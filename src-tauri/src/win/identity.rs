//! 本进程身份（「本人」）与参考 SID：SYSTEM、BUILTIN\Administrators；owner key。

use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{WinBuiltinAdministratorsSid, WinLocalSystemSid, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::sid::OwnedSid;
use super::util::{last_err, owned_handle_nullable};
use crate::mihomo::trust::{FactErr, Ident, Who};

pub struct Known {
    pub me: OwnedSid,
    pub me_str: String,
    pub system: OwnedSid,
    pub admins: OwnedSid,
}

impl Known {
    pub fn classify(&self, sid: &OwnedSid) -> Ident {
        let who = if sid.equals(&self.me) {
            Who::Me
        } else if sid.equals(&self.system) {
            Who::System
        } else if sid.equals(&self.admins) {
            Who::Admins
        } else {
            Who::Other
        };
        Ident::new(who, sid.to_string_sid())
    }
}

fn collect() -> Result<Known, FactErr> {
    let mut t: HANDLE = std::ptr::null_mut();
    // SAFETY: 当前进程伪句柄；令牌句柄交给 OwnedHandle 关闭。
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) } == 0 {
        return Err(last_err("OpenProcessToken"));
    }
    let t = owned_handle_nullable(t, "OpenProcessToken")?;
    let me = OwnedSid::token_user(t.as_raw_handle())?;
    let me_str = me.to_string_sid();
    if !me_str.starts_with("S-1-") {
        return Err(FactErr::new(
            "ConvertSidToStringSidW",
            windows_sys::Win32::Foundation::ERROR_INVALID_SID,
        ));
    }
    Ok(Known {
        me,
        me_str,
        system: OwnedSid::well_known(WinLocalSystemSid)?,
        admins: OwnedSid::well_known(WinBuiltinAdministratorsSid)?,
    })
}

/// 进程生命周期内不变，缓存一次。
pub fn known() -> Result<&'static Known, FactErr> {
    static K: OnceLock<Result<Known, FactErr>> = OnceLock::new();
    K.get_or_init(collect).as_ref().map_err(|e| *e)
}

/// 命名契约的 owner key：hex(sha256(当前用户 SID 字符串))。
pub fn owner_key() -> Result<String, FactErr> {
    Ok(crate::mihomo::guard::owner_key_from_sid(&known()?.me_str))
}
