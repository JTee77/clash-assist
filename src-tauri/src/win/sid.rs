//! 自有内存里的 SID。

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{ERROR_INVALID_SID, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    CopySid, CreateWellKnownSid, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid,
    TokenUser, PSID, SECURITY_MAX_SID_SIZE, TOKEN_USER, WELL_KNOWN_SID_TYPE,
};

use super::util::{from_wide_ptr, last_err, AlignedBuf, LocalBox};
use crate::mihomo::trust::FactErr;

/// 复制到我们自己内存中的 SID（按 u32 对齐）。
#[derive(Clone)]
pub struct OwnedSid(Vec<u32>);

impl OwnedSid {
    /// 从他人持有的内存复制 SID。
    ///
    /// # Safety
    /// `p` 为空，或指向可读 `GetLengthSid(p)` 字节的 SID。
    pub unsafe fn copy_from(p: PSID, api: &'static str) -> Result<Self, FactErr> {
        if p.is_null() || IsValidSid(p) == 0 {
            return Err(FactErr::new(api, ERROR_INVALID_SID));
        }
        let len = GetLengthSid(p);
        let mut buf = vec![0u32; (len as usize).div_ceil(4)];
        if CopySid(len, buf.as_mut_ptr().cast(), p) == 0 {
            return Err(last_err("CopySid"));
        }
        Ok(OwnedSid(buf))
    }

    pub fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Self, FactErr> {
        let mut buf = vec![0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
        let mut size = SECURITY_MAX_SID_SIZE;
        // SAFETY: 缓冲区为 SECURITY_MAX_SID_SIZE 字节且 u32 对齐。
        let ok = unsafe {
            CreateWellKnownSid(
                kind,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if ok == 0 {
            return Err(last_err("CreateWellKnownSid"));
        }
        Ok(OwnedSid(buf))
    }

    /// 已打开令牌（需 TOKEN_QUERY）的 TokenUser。
    pub fn token_user(token: HANDLE) -> Result<Self, FactErr> {
        let mut needed = 0u32;
        // SAFETY: 空缓冲区的长度查询。
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(last_err("GetTokenInformation"));
        }
        let mut buf = AlignedBuf::new(needed as usize);
        // SAFETY: 缓冲区不小于 `needed` 字节且 8 字节对齐。
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr(),
                buf.len_bytes() as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(last_err("GetTokenInformation"));
        }
        // SAFETY: 成功时缓冲区里是 TOKEN_USER，其 Sid 指向缓冲区内部。
        unsafe {
            let tu = &*(buf.as_ptr() as *const TOKEN_USER);
            Self::copy_from(tu.User.Sid, "GetTokenInformation")
        }
    }

    pub fn as_psid(&self) -> PSID {
        self.0.as_ptr() as *mut c_void
    }

    pub fn equals(&self, other: &OwnedSid) -> bool {
        // SAFETY: 两个都是我们持有的有效 SID。
        unsafe { EqualSid(self.as_psid(), other.as_psid()) != 0 }
    }

    pub fn to_string_sid(&self) -> String {
        let mut p: *mut u16 = std::ptr::null_mut();
        // SAFETY: 有效 SID；输出经 LocalBox 释放。
        let ok = unsafe { ConvertSidToStringSidW(self.as_psid(), &mut p) };
        if ok == 0 {
            return format!(
                "<ConvertSidToStringSidW 失败：{}>",
                last_err("ConvertSidToStringSidW").code
            );
        }
        // SAFETY: 文档要求 LocalFree。
        let owner = unsafe { LocalBox::from_raw(p.cast()) };
        // SAFETY: `owner` 存活期间字符串有效。
        unsafe { from_wide_ptr(owner.as_ptr() as *const u16) }.unwrap_or_default()
    }
}
