//! Owned SIDs and the "who is this" description used throughout the report.

use std::ffi::c_void;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{
    CopySid, CreateWellKnownSid, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid,
    LookupAccountSidW, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid, PSID,
    SECURITY_MAX_SID_SIZE, SID_NAME_USE, TOKEN_USER, WELL_KNOWN_SID_TYPE,
};

use super::util::{from_wide, from_wide_ptr, wide, AlignedBuf, LocalBox, WinErr};

/// A SID copied into memory we own (u32-aligned, as SID requires).
#[derive(Clone)]
pub struct OwnedSid(Vec<u32>);

impl OwnedSid {
    /// Copy a SID out of memory owned by someone else.
    ///
    /// # Safety
    /// `p` must be null or point to a SID readable for `GetLengthSid(p)` bytes.
    pub unsafe fn copy_from(p: PSID, api: &'static str) -> Result<Self, WinErr> {
        if p.is_null() || IsValidSid(p) == 0 {
            return Err(WinErr::code(
                api,
                windows_sys::Win32::Foundation::ERROR_INVALID_SID,
            ));
        }
        let len = GetLengthSid(p);
        let mut buf = vec![0u32; (len as usize).div_ceil(4)];
        if CopySid(len, buf.as_mut_ptr().cast(), p) == 0 {
            return Err(WinErr::last("CopySid"));
        }
        Ok(OwnedSid(buf))
    }

    pub fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Self, WinErr> {
        let mut buf = vec![0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
        let mut size = SECURITY_MAX_SID_SIZE;
        // SAFETY: buffer is SECURITY_MAX_SID_SIZE bytes and u32-aligned.
        let ok = unsafe {
            CreateWellKnownSid(
                kind,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if ok == 0 {
            return Err(WinErr::last("CreateWellKnownSid"));
        }
        Ok(OwnedSid(buf))
    }

    pub fn system() -> Result<Self, WinErr> {
        Self::well_known(WinLocalSystemSid)
    }

    pub fn builtin_admins() -> Result<Self, WinErr> {
        Self::well_known(WinBuiltinAdministratorsSid)
    }

    /// Parse `S-1-...` or an SDDL alias such as `SY` / `BA`.
    pub fn parse(s: &str) -> Result<Self, WinErr> {
        let w = wide(s);
        let mut p: PSID = std::ptr::null_mut();
        // SAFETY: `w` is NUL-terminated; on success `p` must be LocalFree'd (LocalBox).
        let ok = unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut p) };
        if ok == 0 {
            return Err(WinErr::last("ConvertStringSidToSidW"));
        }
        // SAFETY: documented to be freed with LocalFree.
        let owner = unsafe { LocalBox::from_raw(p) };
        // SAFETY: valid SID returned by the API, alive while `owner` lives.
        unsafe { Self::copy_from(owner.as_ptr(), "ConvertStringSidToSidW") }
    }

    /// TokenUser of an open token handle (needs TOKEN_QUERY).
    pub fn token_user(token: HANDLE) -> Result<Self, WinErr> {
        let mut needed = 0u32;
        // SAFETY: size query with a null buffer.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(WinErr::last("GetTokenInformation"));
        }
        let mut buf = AlignedBuf::new(needed as usize);
        // SAFETY: buffer is at least `needed` bytes and 8-aligned.
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
            return Err(WinErr::last("GetTokenInformation"));
        }
        // SAFETY: on success the buffer holds a TOKEN_USER whose Sid points inside the buffer.
        unsafe {
            let tu = &*(buf.as_ptr() as *const TOKEN_USER);
            Self::copy_from(tu.User.Sid, "GetTokenInformation")
        }
    }

    pub fn as_psid(&self) -> PSID {
        self.0.as_ptr() as *mut c_void
    }

    pub fn equals(&self, other: &OwnedSid) -> bool {
        // SAFETY: both are valid SIDs we own.
        unsafe { EqualSid(self.as_psid(), other.as_psid()) != 0 }
    }

    pub fn to_string_sid(&self) -> String {
        let mut p: *mut u16 = std::ptr::null_mut();
        // SAFETY: valid SID; output freed via LocalBox.
        let ok = unsafe { ConvertSidToStringSidW(self.as_psid(), &mut p) };
        if ok == 0 {
            return format!(
                "<ConvertSidToStringSidW failed: {}>",
                WinErr::last("ConvertSidToStringSidW").code
            );
        }
        // SAFETY: documented to be freed with LocalFree.
        let owner = unsafe { LocalBox::from_raw(p.cast()) };
        // SAFETY: NUL-terminated string alive while `owner` lives.
        unsafe { from_wide_ptr(owner.as_ptr() as *const u16) }.unwrap_or_default()
    }

    /// `DOMAIN\name`, or None when the SID does not map (e.g. ERROR_NONE_MAPPED).
    pub fn account(&self) -> Option<String> {
        let (mut n, mut d) = (0u32, 0u32);
        let mut use_: SID_NAME_USE = 0;
        // SAFETY: size query.
        unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                self.as_psid(),
                std::ptr::null_mut(),
                &mut n,
                std::ptr::null_mut(),
                &mut d,
                &mut use_,
            )
        };
        if WinErr::last("LookupAccountSidW").code != ERROR_INSUFFICIENT_BUFFER || n == 0 {
            return None;
        }
        let mut name = vec![0u16; n as usize];
        let mut dom = vec![0u16; d.max(1) as usize];
        // SAFETY: buffers sized per the previous call.
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                self.as_psid(),
                name.as_mut_ptr(),
                &mut n,
                dom.as_mut_ptr(),
                &mut d,
                &mut use_,
            )
        };
        if ok == 0 {
            return None;
        }
        let name = from_wide(&name[..n as usize]);
        let dom = from_wide(&dom[..d as usize]);
        Some(if dom.is_empty() {
            name
        } else {
            format!("{dom}\\{name}")
        })
    }
}

/// Reference SIDs used to annotate every SID in the report (facts only, no decisions).
pub struct Known {
    pub me: Option<OwnedSid>,
    pub system: Option<OwnedSid>,
    pub admins: Option<OwnedSid>,
}

impl Known {
    pub fn describe(&self, sid: &OwnedSid) -> Value {
        let eq = |o: &Option<OwnedSid>| o.as_ref().map(|k| k.equals(sid));
        json!({
            "sid": sid.to_string_sid(),
            "account": sid.account(),
            "is_self": eq(&self.me),
            "is_system": eq(&self.system),
            "is_builtin_admins": eq(&self.admins),
        })
    }
}
