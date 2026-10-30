//! Server/listener process identity: L2 ①②③, L3 and WTS (windows-plan §2.2, §2.1 rule 4).
//!
//! Each collector returns a report `Step`; nothing here decides trust.

use std::os::windows::io::{AsRawHandle, OwnedHandle};

use serde_json::{json, Value};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS,
};
use windows_sys::Win32::Foundation::{HANDLE, NTSTATUS, UNICODE_STRING};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::RemoteDesktop::{
    WTSEnumerateProcessesExW, WTSFreeMemoryExW, WTSTypeProcessInfoLevel1,
    WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFO_EXW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::sid::{Known, OwnedSid};
use super::util::{
    file_name, from_wide, from_wide_ptr, owned_handle_invalid, owned_handle_nullable, WinErr,
};
use crate::report::Step;

/// `SystemProcessIdInformation` — undocumented, not in windows-sys (plan §2.2 L2 ②).
const SYSTEM_PROCESS_ID_INFORMATION_CLASS: SYSTEM_INFORMATION_CLASS = 0x58;
/// `WTS_ANY_SESSION` — not exported by windows-sys 0.61.
const WTS_ANY_SESSION: u32 = 0xFFFF_FFFE;
/// Longest NT path a UNICODE_STRING can carry, in UTF-16 units.
const MAX_UNICODE_STRING_CHARS: usize = (u16::MAX as usize) / 2;

/// Layout of `SYSTEM_PROCESS_ID_INFORMATION` (undocumented; matches phnt / ProcessHacker).
#[repr(C)]
struct SystemProcessIdInformation {
    process_id: HANDLE,
    image_name: UNICODE_STRING,
}

fn open_process(pid: u32) -> Result<OwnedHandle, WinErr> {
    // SAFETY: plain call; result checked and wrapped for RAII close.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    owned_handle_nullable(h, "OpenProcess")
}

/// L2 ①: OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) + QueryFullProcessImageNameW.
pub fn l2_1(pid: u32) -> Step {
    let r = (|| -> Result<Value, WinErr> {
        let h = open_process(pid)?;
        let mut buf = vec![0u16; 32_768];
        let mut size = buf.len() as u32;
        // SAFETY: buffer of `size` UTF-16 units; handle alive for the call.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                h.as_raw_handle(),
                PROCESS_NAME_WIN32,
                buf.as_mut_ptr(),
                &mut size,
            )
        };
        if ok == 0 {
            return Err(WinErr::last("QueryFullProcessImageNameW"));
        }
        let path = from_wide(&buf[..(size as usize).min(buf.len())]);
        Ok(json!({ "path": path, "file_name": file_name(&path) }))
    })();
    match r {
        Ok(v) => Step::ok("l2_1", v),
        Err(e) => e.step("l2_1"),
    }
    .with_pid(pid)
}

/// L2 ②: NtQuerySystemInformation(SystemProcessIdInformation = 0x58) → NT image path.
pub fn l2_2(pid: u32) -> Step {
    let mut buf = vec![0u16; MAX_UNICODE_STRING_CHARS];
    let mut info = SystemProcessIdInformation {
        process_id: pid as usize as HANDLE,
        image_name: UNICODE_STRING {
            Length: 0,
            MaximumLength: (buf.len() * 2) as u16,
            Buffer: buf.as_mut_ptr(),
        },
    };
    let mut ret_len = 0u32;
    // SAFETY: `info` is a correctly laid out SYSTEM_PROCESS_ID_INFORMATION whose ImageName
    // buffer points at `buf` (MaximumLength bytes), both alive for the call.
    let status: NTSTATUS = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_ID_INFORMATION_CLASS,
            (&mut info as *mut SystemProcessIdInformation).cast(),
            std::mem::size_of::<SystemProcessIdInformation>() as u32,
            &mut ret_len,
        )
    };
    if status < 0 {
        return WinErr::nt("NtQuerySystemInformation", status)
            .step("l2_2")
            .with_pid(pid);
    }
    let chars = (info.image_name.Length as usize / 2).min(buf.len());
    // The kernel writes into our buffer; never trust a different Buffer pointer.
    if !info.image_name.Buffer.is_null() && info.image_name.Buffer != buf.as_mut_ptr() {
        return Step::failed("l2_2", "kernel returned an unexpected ImageName buffer")
            .with_pid(pid);
    }
    let path = from_wide(&buf[..chars]);
    Step::ok(
        "l2_2",
        json!({ "nt_path": path, "file_name": file_name(&path) }),
    )
    .with_pid(pid)
}

/// L2 ③: CreateToolhelp32Snapshot + Process32FirstW/NextW → szExeFile.
pub fn l2_3(pid: u32) -> Step {
    let r = (|| -> Result<Option<Value>, WinErr> {
        // SAFETY: plain call; INVALID_HANDLE_VALUE on failure.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        let snap = owned_handle_invalid(snap, "CreateToolhelp32Snapshot")?;
        // SAFETY: zeroed POD struct with dwSize set as the API requires.
        let mut pe: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: valid snapshot handle and correctly sized entry.
        let mut ok = unsafe { Process32FirstW(snap.as_raw_handle(), &mut pe) };
        if ok == 0 {
            return Err(WinErr::last("Process32FirstW"));
        }
        while ok != 0 {
            if pe.th32ProcessID == pid {
                let n = pe
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(pe.szExeFile.len());
                return Ok(Some(json!({
                    "exe_file": from_wide(&pe.szExeFile[..n]),
                    "parent_pid": pe.th32ParentProcessID,
                })));
            }
            // SAFETY: as above.
            ok = unsafe { Process32NextW(snap.as_raw_handle(), &mut pe) };
        }
        Ok(None)
    })();
    match r {
        Ok(Some(v)) => Step::ok("l2_3", v),
        Ok(None) => Step::failed("l2_3", "pid-not-in-snapshot"),
        Err(e) => e.step("l2_3"),
    }
    .with_pid(pid)
}

/// L3: OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) + OpenProcessToken(TOKEN_QUERY)
/// + GetTokenInformation(TokenUser). `api` tells which of the three failed.
pub fn l3(pid: u32, known: &Known) -> Step {
    let r = (|| -> Result<OwnedSid, WinErr> {
        let proc_ = open_process(pid)?;
        let mut tok: HANDLE = std::ptr::null_mut();
        // SAFETY: valid process handle; token handle wrapped for RAII close.
        if unsafe { OpenProcessToken(proc_.as_raw_handle(), TOKEN_QUERY, &mut tok) } == 0 {
            return Err(WinErr::last("OpenProcessToken"));
        }
        let tok = owned_handle_nullable(tok, "OpenProcessToken")?;
        OwnedSid::token_user(tok.as_raw_handle())
    })();
    match r {
        Ok(sid) => Step::ok("l3", json!({ "token_user": known.describe(&sid) })),
        Err(e) => e.step("l3"),
    }
    .with_pid(pid)
}

/// One row of `WTSEnumerateProcessesExW(level 1)`.
pub struct WtsEntry {
    pub pid: u32,
    pub session_id: u32,
    pub process_name: Option<String>,
    pub user_sid: Option<OwnedSid>,
}

/// Snapshot of all processes visible via WTS (WTS_CURRENT_SERVER_HANDLE, level 1, any session).
pub struct WtsSnapshot(Result<Vec<WtsEntry>, WinErr>);

impl WtsSnapshot {
    pub fn take() -> Self {
        let mut level = 1u32;
        let mut p: *mut u16 = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: out-params are valid; on success memory is freed below with WTSFreeMemoryExW.
        let ok = unsafe {
            WTSEnumerateProcessesExW(
                WTS_CURRENT_SERVER_HANDLE,
                &mut level,
                WTS_ANY_SESSION,
                &mut p,
                &mut count,
            )
        };
        if ok == 0 {
            return WtsSnapshot(Err(WinErr::last("WTSEnumerateProcessesExW")));
        }
        let guard = WtsMem { p: p.cast(), count };
        if level != 1 {
            return WtsSnapshot(Err(WinErr::code(
                "WTSEnumerateProcessesExW",
                windows_sys::Win32::Foundation::ERROR_INVALID_LEVEL,
            )));
        }
        let mut v = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: level-1 output is an array of `count` WTS_PROCESS_INFO_EXW.
            let e = unsafe { &*guard.p.add(i) };
            v.push(WtsEntry {
                pid: e.ProcessId,
                session_id: e.SessionId,
                // SAFETY: NUL-terminated strings / SIDs inside the WTS allocation, still alive.
                process_name: unsafe { from_wide_ptr(e.pProcessName) },
                user_sid: if e.pUserSid.is_null() {
                    None
                } else {
                    // SAFETY: non-null SID inside the WTS allocation, still alive.
                    unsafe { OwnedSid::copy_from(e.pUserSid, "WTSEnumerateProcessesExW") }.ok()
                },
            });
        }
        drop(guard);
        WtsSnapshot(Ok(v))
    }

    /// Per-PID `wts` step: pUserSid + SessionId (diagnostic only, C-4).
    pub fn step_for(&self, pid: u32, known: &Known) -> Step {
        match &self.0 {
            Err(e) => e.step("wts"),
            Ok(rows) => match rows.iter().find(|r| r.pid == pid) {
                None => Step::failed("wts", "pid-not-in-wts-snapshot")
                    .with_value(json!({ "snapshot_size": rows.len() })),
                Some(r) => Step::ok(
                    "wts",
                    json!({
                        "session_id": r.session_id,
                        "process_name": r.process_name,
                        "user_sid": r.user_sid.as_ref().map(|s| known.describe(s)),
                        "snapshot_size": rows.len(),
                    }),
                ),
            },
        }
        .with_pid(pid)
    }
}

struct WtsMem {
    p: *mut WTS_PROCESS_INFO_EXW,
    count: u32,
}

impl Drop for WtsMem {
    fn drop(&mut self) {
        if !self.p.is_null() {
            // SAFETY: memory returned by WTSEnumerateProcessesExW level 1, freed exactly once.
            unsafe { WTSFreeMemoryExW(WTSTypeProcessInfoLevel1, self.p.cast(), self.count) };
        }
    }
}

/// L2 ①②③, L3, WTS for one PID, in that order (all always run; the probe records every result).
pub fn identity_steps(pid: u32, known: &Known, wts: &WtsSnapshot) -> Vec<Step> {
    vec![
        l2_1(pid),
        l2_2(pid),
        l2_3(pid),
        l3(pid, known),
        wts.step_for(pid, known),
    ]
}
