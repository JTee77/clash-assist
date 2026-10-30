//! Security-descriptor reads/writes on a kernel handle (L1, S-2 ACE walk, S-E owner forging).

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
    ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};

use super::sid::{Known, OwnedSid};
use super::util::{from_wide_ptr, wide, LocalBox, WinErr};

/// L1: `GetSecurityInfo(h, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION)` on the given handle.
pub fn owner(h: HANDLE) -> Result<OwnedSid, WinErr> {
    let mut owner: PSID = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: out-params valid; `sd` must be LocalFree'd (LocalBox) and `owner` points into it.
    let rc = unsafe {
        GetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    // SAFETY: freed with LocalFree per the GetSecurityInfo contract (null is fine).
    let _sd = unsafe { LocalBox::from_raw(sd) };
    if rc != ERROR_SUCCESS {
        return Err(WinErr::code("GetSecurityInfo", rc));
    }
    // SAFETY: `owner` points into `_sd`, which is still alive.
    unsafe { OwnedSid::copy_from(owner, "GetSecurityInfo") }
}

/// S-2: owner/group/DACL as an SDDL string plus a raw ACE walk (facts only).
pub fn dacl(h: HANDLE, known: &Known) -> Result<Value, WinErr> {
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut pdacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: out-params valid; `sd` freed via LocalBox, `pdacl` points into it.
    let rc = unsafe {
        GetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            info,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut pdacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    // SAFETY: LocalFree contract of GetSecurityInfo.
    let sd = unsafe { LocalBox::from_raw(sd) };
    if rc != ERROR_SUCCESS {
        return Err(WinErr::code("GetSecurityInfo", rc));
    }

    let mut s: *mut u16 = std::ptr::null_mut();
    // SAFETY: valid SD; output string freed via LocalBox.
    let sddl = if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd.as_ptr(),
            SDDL_REVISION_1,
            info,
            &mut s,
            std::ptr::null_mut(),
        )
    } != 0
    {
        // SAFETY: LocalFree contract of the conversion API.
        let s = unsafe { LocalBox::from_raw(s.cast()) };
        // SAFETY: NUL-terminated string alive while `s` lives.
        json!(unsafe { from_wide_ptr(s.as_ptr() as *const u16) })
    } else {
        json!({ "error": WinErr::last("ConvertSecurityDescriptorToStringSecurityDescriptorW").to_json() })
    };

    if pdacl.is_null() {
        return Ok(json!({ "sddl": sddl, "null_dacl": true, "aces": [] }));
    }
    let mut size_info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `pdacl` valid while `sd` lives; correctly sized out struct.
    if unsafe {
        GetAclInformation(
            pdacl,
            (&mut size_info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(WinErr::last("GetAclInformation"));
    }
    let mut aces = Vec::new();
    for i in 0..size_info.AceCount {
        let mut pace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: index < AceCount; pointer points into the DACL.
        if unsafe { GetAce(pdacl, i, &mut pace) } == 0 || pace.is_null() {
            aces.push(json!({ "index": i, "error": WinErr::last("GetAce").to_json() }));
            continue;
        }
        // SAFETY: every ACE starts with ACE_HEADER.
        let hdr = unsafe { &*(pace as *const ACE_HEADER) };
        let mut ace = json!({
            "index": i,
            "ace_type": hdr.AceType,
            "ace_flags": hdr.AceFlags,
        });
        // 0 = ACCESS_ALLOWED_ACE_TYPE, 1 = ACCESS_DENIED_ACE_TYPE: identical layout {Header, Mask, SidStart}.
        if hdr.AceType == 0 || hdr.AceType == 1 {
            // SAFETY: type checked above; SID starts at SidStart inside the ACE.
            let a = unsafe { &*(pace as *const ACCESS_ALLOWED_ACE) };
            // SAFETY: the SID is embedded in the ACE starting at SidStart, inside the live DACL.
            let sid = unsafe { OwnedSid::copy_from((&a.SidStart as *const u32) as PSID, "GetAce") };
            let mask = a.Mask;
            ace["kind"] = json!(if hdr.AceType == 0 { "allow" } else { "deny" });
            ace["mask"] = json!(format!("0x{mask:08X}"));
            // FILE_WRITE_DATA (0x2) | GENERIC_WRITE (0x40000000) | GENERIC_ALL (0x10000000)
            ace["has_write_bits"] = json!(mask & (0x2 | 0x4000_0000 | 0x1000_0000) != 0);
            ace["trustee"] = match sid {
                Ok(s) => known.describe(&s),
                Err(e) => json!({ "error": e.to_json() }),
            };
        }
        aces.push(ace);
    }
    Ok(json!({ "sddl": sddl, "null_dacl": false, "ace_count": size_info.AceCount, "aces": aces }))
}

/// `SetSecurityInfo(h, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, sid)`. Returns the raw Win32 code.
pub fn set_owner(h: HANDLE, sid: &OwnedSid) -> Result<(), WinErr> {
    // SAFETY: valid handle and SID; other pointers null as allowed for OWNER-only updates.
    let rc = unsafe {
        SetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            sid.as_psid(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(WinErr::code("SetSecurityInfo", rc));
    }
    Ok(())
}

/// Security descriptor parsed from SDDL, freed with LocalFree on drop.
pub struct ParsedSd(LocalBox);

impl ParsedSd {
    pub fn parse(sddl: &str) -> Result<Self, WinErr> {
        let w = wide(sddl);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: NUL-terminated input; output freed via LocalBox.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(WinErr::last(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            ));
        }
        // SAFETY: LocalFree contract.
        let b = unsafe { LocalBox::from_raw(sd) };
        if b.is_null() {
            return Err(WinErr::code(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
                windows_sys::Win32::Foundation::ERROR_INVALID_SECURITY_DESCR,
            ));
        }
        Ok(ParsedSd(b))
    }

    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0.as_ptr()
    }
}
