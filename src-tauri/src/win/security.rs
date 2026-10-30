//! 内核对象句柄上的安全描述符：一次 `GetSecurityInfo(OWNER | DACL | LABEL)`
//! 得到 L1 属主、DACL 事实（F3′ / S-2）与强制完整性标签（Ellie V5 复跑复核 §2-3、§2-7）。

use windows_sys::Win32::Foundation::{ERROR_INVALID_ACL, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, GetLengthSid, GetSidSubAuthority,
    GetSidSubAuthorityCount, IsValidSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID,
};

use super::identity::Known;
use super::sid::OwnedSid;
use super::util::{last_err, LocalBox};
use crate::mihomo::trust::{
    AceFact, AceKind, DaclFact, FactErr, LabelFact, SecurityFact, MANDATORY_NO_WRITE_UP,
};

/// SYSTEM_MANDATORY_LABEL_ACE_TYPE。
const MANDATORY_LABEL_ACE: u8 = 0x11;

/// 在**已连接的同一个句柄**上一次读出属主、DACL 与完整性标签（不按名字重开，M-2）。
pub fn security(h: HANDLE, known: &Known) -> Result<SecurityFact, FactErr> {
    let mut owner: PSID = std::ptr::null_mut();
    let mut pdacl: *mut ACL = std::ptr::null_mut();
    let mut psacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: 出参有效；`sd` 由 LocalBox 释放，其余指针都指向 `sd` 内部。
    let rc = unsafe {
        GetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut pdacl,
            &mut psacl,
            &mut sd,
        )
    };
    // SAFETY: GetSecurityInfo 的 LocalFree 约定（空指针亦可）。
    let _sd = unsafe { LocalBox::from_raw(sd) };
    if rc != ERROR_SUCCESS {
        return Err(FactErr::new("GetSecurityInfo", rc));
    }
    // SAFETY: 三个指针都指向仍存活的 `_sd`（或为空）。
    let owner = unsafe { OwnedSid::copy_from(owner, "GetSecurityInfo") }?;
    // DACL 不存在（present = false）时 GetSecurityInfo 同样返回空指针：与 NULL DACL 一并拒绝。
    let dacl = if pdacl.is_null() {
        DaclFact::Null
    } else {
        // SAFETY: 非空的 DACL 指向存活的 `_sd`。
        DaclFact::Aces(unsafe { aces(pdacl, known) }?)
    };
    let label = if psacl.is_null() {
        LabelFact::None
    } else {
        // SAFETY: 非空的 SACL 指向存活的 `_sd`。
        unsafe { label(psacl) }?
    };
    Ok(SecurityFact {
        owner: known.classify(&owner),
        dacl,
        label,
    })
}

/// ACL 的 ACE 数。
///
/// # Safety
/// `acl` 指向有效的 ACL。
unsafe fn ace_count(acl: *mut ACL) -> Result<u32, FactErr> {
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    if GetAclInformation(
        acl,
        (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return Err(last_err("GetAclInformation"));
    }
    Ok(info.AceCount)
}

/// SACL 中的强制标签：取最低的 RID（多于一个时保守）。
///
/// # Safety
/// `sacl` 指向有效的 ACL。
unsafe fn label(sacl: *mut ACL) -> Result<LabelFact, FactErr> {
    let mut out = LabelFact::None;
    for i in 0..ace_count(sacl)? {
        let mut pace: *mut core::ffi::c_void = std::ptr::null_mut();
        if GetAce(sacl, i, &mut pace) == 0 || pace.is_null() {
            return Ok(LabelFact::Unparsed);
        }
        let hdr = &*(pace as *const ACE_HEADER);
        if hdr.AceType != MANDATORY_LABEL_ACE {
            continue;
        }
        // SYSTEM_MANDATORY_LABEL_ACE 与 ACCESS_ALLOWED_ACE 布局相同。
        let a = &*(pace as *const ACCESS_ALLOWED_ACE);
        let sid = (&a.SidStart as *const u32) as PSID;
        let size = hdr.AceSize as usize;
        if size < SID_OFFSET + 8
            || IsValidSid(sid) == 0
            || GetLengthSid(sid) as usize > size - SID_OFFSET
        {
            return Ok(LabelFact::Unparsed);
        }
        let n = *GetSidSubAuthorityCount(sid);
        if n == 0 {
            return Ok(LabelFact::Unparsed);
        }
        let rid = *GetSidSubAuthority(sid, u32::from(n) - 1);
        // 任一标签 ACE 缺 NO_WRITE_UP ⇒ WithoutNoWriteUp（拒绝，Ellie PR50 Q4）；RID 取最低。
        let nw = a.Mask & MANDATORY_NO_WRITE_UP != 0;
        out = match out {
            LabelFact::None => {
                if nw {
                    LabelFact::Rid(rid)
                } else {
                    LabelFact::WithoutNoWriteUp(rid)
                }
            }
            LabelFact::Rid(r) if nw => LabelFact::Rid(r.min(rid)),
            LabelFact::Rid(r) | LabelFact::WithoutNoWriteUp(r) => {
                LabelFact::WithoutNoWriteUp(r.min(rid))
            }
            LabelFact::Unparsed => LabelFact::Unparsed,
        };
    }
    Ok(out)
}

const INHERIT_ONLY_ACE: u8 = 0x08;
/// ACE 头（4 字节）+ Mask（4 字节）之后是 SID。
const SID_OFFSET: usize = 8;

fn ace_kind(t: u8) -> AceKind {
    match t {
        0 => AceKind::Allow,                     // ACCESS_ALLOWED_ACE_TYPE
        1 | 6 | 0x0A | 0x0C => AceKind::Deny, // DENIED / DENIED_OBJECT / DENIED_CALLBACK(_OBJECT)
        4 | 5 | 9 | 0x0B => AceKind::OtherAllow, // COMPOUND / ALLOWED_OBJECT / ALLOWED_CALLBACK(_OBJECT)
        _ => AceKind::Other,                     // 审计、强制标签等
    }
}

/// DACL 的逐条 ACE（主体按本人 / SYSTEM / BA / 其他分类）。
///
/// # Safety
/// `pdacl` 指向有效的 ACL，且在调用期间存活。
unsafe fn aces(pdacl: *mut ACL, known: &Known) -> Result<Vec<AceFact>, FactErr> {
    let count = ace_count(pdacl)?;
    let mut aces = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut pace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: i < AceCount；指针指向 DACL 内部。
        if unsafe { GetAce(pdacl, i, &mut pace) } == 0 || pace.is_null() {
            // 读不出的 ACE：保守地当作无法判定的 ACE（F3′ 拒绝）。
            aces.push(AceFact {
                kind: AceKind::OtherAllow,
                mask: 0,
                inherit_only: false,
                trustee: None,
            });
            continue;
        }
        // SAFETY: 每个 ACE 都以 ACE_HEADER 开头。
        let hdr = unsafe { &*(pace as *const ACE_HEADER) };
        let kind = ace_kind(hdr.AceType);
        let inherit_only = hdr.AceFlags & INHERIT_ONLY_ACE != 0;
        let (mask, trustee) = if matches!(kind, AceKind::Allow | AceKind::Deny) && hdr.AceType <= 1
        {
            // ACCESS_ALLOWED_ACE / ACCESS_DENIED_ACE 布局相同：{Header, Mask, SidStart}。
            // SAFETY: 类型已检查。
            let a = unsafe { &*(pace as *const ACCESS_ALLOWED_ACE) };
            let sid_ptr = (&a.SidStart as *const u32) as PSID;
            let ace_size = hdr.AceSize as usize;
            // SID 必须完整落在 ACE 内（防止越界读）。
            // SAFETY: IsValidSid / GetLengthSid 只读 SID 头；长度在复制前再与 AceSize 比较。
            let fits = ace_size >= SID_OFFSET + 8
                && unsafe { IsValidSid(sid_ptr) } != 0
                && (unsafe { GetLengthSid(sid_ptr) } as usize) <= ace_size - SID_OFFSET;
            let trustee = if fits {
                // SAFETY: 已确认 SID 完整位于 ACE 内，DACL 仍存活。
                unsafe { OwnedSid::copy_from(sid_ptr, "GetAce") }
                    .ok()
                    .map(|s| known.classify(&s))
            } else {
                None
            };
            (a.Mask, trustee)
        } else {
            (0, None)
        };
        aces.push(AceFact {
            kind,
            mask,
            inherit_only,
            trustee,
        });
    }
    if aces.len() != count as usize {
        return Err(FactErr::new("GetAce", ERROR_INVALID_ACL));
    }
    Ok(aces)
}
