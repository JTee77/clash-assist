//! Who is running the probe (to tell `ca_a` / `ca_b` / runneradmin runs apart, and to
//! check P7's precondition that `ca_b` really is a non-admin).

use std::os::windows::io::AsRawHandle;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{
    CheckTokenMembership, GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
};

use super::sid::{Known, OwnedSid};
use super::util::{owned_handle_nullable, WinErr};

pub struct Context {
    pub known: Known,
    pub json: Value,
}

pub fn collect() -> Context {
    // SAFETY: trivial queries on the current process.
    let pid = unsafe { GetCurrentProcessId() };
    let mut session = 0u32;
    // SAFETY: out-param valid.
    let session = if unsafe { ProcessIdToSessionId(pid, &mut session) } != 0 {
        json!(session)
    } else {
        json!({ "error": WinErr::last("ProcessIdToSessionId").to_json() })
    };

    let token = (|| {
        let mut t: HANDLE = std::ptr::null_mut();
        // SAFETY: pseudo-handle for the current process; token wrapped for RAII close.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) } == 0 {
            return Err(WinErr::last("OpenProcessToken"));
        }
        owned_handle_nullable(t, "OpenProcessToken")
    })();

    let (me, elevated) = match &token {
        Ok(t) => {
            let me = OwnedSid::token_user(t.as_raw_handle());
            let mut el = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut ret = 0u32;
            // SAFETY: correctly sized out struct.
            let elevated = if unsafe {
                GetTokenInformation(
                    t.as_raw_handle(),
                    TokenElevation,
                    (&mut el as *mut TOKEN_ELEVATION).cast(),
                    std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                    &mut ret,
                )
            } != 0
            {
                json!(el.TokenIsElevated != 0)
            } else {
                json!({ "error": WinErr::last("GetTokenInformation").to_json() })
            };
            (me, elevated)
        }
        Err(e) => (Err(*e), json!({ "error": e.to_json() })),
    };

    let known = Known {
        me: me.as_ref().ok().cloned(),
        system: OwnedSid::system().ok(),
        admins: OwnedSid::builtin_admins().ok(),
    };

    // Is BUILTIN\Administrators *enabled* in our token (false for a filtered/standard token)?
    let ba_enabled = match &known.admins {
        Some(ba) => {
            let mut is = 0i32;
            // SAFETY: null token = current thread/process token; valid SID.
            if unsafe { CheckTokenMembership(std::ptr::null_mut(), ba.as_psid(), &mut is) } != 0 {
                json!(is != 0)
            } else {
                json!({ "error": WinErr::last("CheckTokenMembership").to_json() })
            }
        }
        None => Value::Null,
    };

    let self_json = match &me {
        Ok(s) => known.describe(s),
        Err(e) => json!({ "error": e.to_json() }),
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let json = json!({
        "pid": pid,
        "session_id": session,
        "self": self_json,
        "elevated": elevated,
        "builtin_admins_enabled": ba_enabled,
        "timestamp_unix_ms": ts,
    });
    Context { known, json }
}
