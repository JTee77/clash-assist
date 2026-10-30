//! 服务端 / 监听进程的身份：L2 ①②③、L3、WTS（§2.2、§2.1 规则 4）。

use std::os::windows::io::{AsRawHandle, OwnedHandle};

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, ERROR_INVALID_LEVEL, HANDLE, NTSTATUS, UNICODE_STRING,
};
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

use super::identity::Known;
use super::sid::OwnedSid;
use super::util::{from_wide, last_err, owned_handle_invalid, owned_handle_nullable};
use crate::mihomo::trust::{FactErr, Ident, WtsUser};

/// `SystemProcessIdInformation`：未公开，windows-sys 中没有（§2.2 L2 ②）。
const SYSTEM_PROCESS_ID_INFORMATION_CLASS: SYSTEM_INFORMATION_CLASS = 0x58;
/// `WTS_ANY_SESSION`：windows-sys 0.61 未导出。
const WTS_ANY_SESSION: u32 = 0xFFFF_FFFE;
/// UNICODE_STRING 能容纳的最长 NT 路径（UTF-16 单元）。
const MAX_UNICODE_STRING_CHARS: usize = (u16::MAX as usize) / 2;

/// `SYSTEM_PROCESS_ID_INFORMATION` 的布局（与 phnt / ProcessHacker 一致）。
#[repr(C)]
struct SystemProcessIdInformation {
    process_id: HANDLE,
    image_name: UNICODE_STRING,
}

/// 一个 PID 的进程句柄：L2 ① 与 L3 共用**同一个**句柄（Ellie S1），缩小 PID 复用窗口。
pub struct ProcessProbe {
    pid: u32,
    handle: Option<Result<OwnedHandle, FactErr>>,
}

impl ProcessProbe {
    pub fn new(pid: u32) -> Self {
        ProcessProbe { pid, handle: None }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    fn handle(&mut self) -> Result<HANDLE, FactErr> {
        let pid = self.pid;
        let h = self.handle.get_or_insert_with(|| {
            // SAFETY: 普通调用；结果交给 OwnedHandle 关闭。
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            owned_handle_nullable(h, "OpenProcess")
        });
        match h {
            Ok(h) => Ok(h.as_raw_handle()),
            Err(e) => Err(*e),
        }
    }

    /// L2 ①：QueryFullProcessImageNameW（Win32 路径）。
    pub fn win32_path(&mut self) -> Result<String, FactErr> {
        let h = self.handle()?;
        let mut buf = vec![0u16; 32_768];
        let mut size = buf.len() as u32;
        // SAFETY: `size` 个 UTF-16 单元的缓冲区；句柄在调用期间有效。
        let ok = unsafe {
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size)
        };
        if ok == 0 {
            return Err(last_err("QueryFullProcessImageNameW"));
        }
        Ok(from_wide(&buf[..(size as usize).min(buf.len())]))
    }

    /// L3：OpenProcessToken(TOKEN_QUERY) + TokenUser。
    pub fn token_user(&mut self, known: &Known) -> Result<Ident, FactErr> {
        let h = self.handle()?;
        let mut tok: HANDLE = std::ptr::null_mut();
        // SAFETY: 有效进程句柄；令牌句柄交给 OwnedHandle 关闭。
        if unsafe { OpenProcessToken(h, TOKEN_QUERY, &mut tok) } == 0 {
            return Err(last_err("OpenProcessToken"));
        }
        let tok = owned_handle_nullable(tok, "OpenProcessToken")?;
        Ok(known.classify(&OwnedSid::token_user(tok.as_raw_handle())?))
    }
}

/// L2 ②：NtQuerySystemInformation(SystemProcessIdInformation) → NT 映像路径。
pub fn nt_path(pid: u32) -> Result<String, FactErr> {
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
    // SAFETY: `info` 布局正确，ImageName 指向存活的 `buf`（MaximumLength 字节）。
    let status: NTSTATUS = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_ID_INFORMATION_CLASS,
            (&mut info as *mut SystemProcessIdInformation).cast(),
            std::mem::size_of::<SystemProcessIdInformation>() as u32,
            &mut ret_len,
        )
    };
    if status < 0 {
        // SAFETY: 纯查表。
        return Err(FactErr::new("NtQuerySystemInformation", unsafe {
            RtlNtStatusToDosError(status)
        }));
    }
    // 内核只应写入我们的缓冲区；指针被换掉就不信任。
    if !info.image_name.Buffer.is_null() && info.image_name.Buffer != buf.as_mut_ptr() {
        return Err(FactErr::new(
            "NtQuerySystemInformation",
            ERROR_INVALID_LEVEL,
        ));
    }
    let chars = (info.image_name.Length as usize / 2).min(buf.len());
    Ok(from_wide(&buf[..chars]))
}

/// L2 ③：Toolhelp32 快照里的 szExeFile。
pub fn snapshot_name(pid: u32) -> Result<String, FactErr> {
    // SAFETY: 普通调用；失败返回 INVALID_HANDLE_VALUE。
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let snap = owned_handle_invalid(snap, "CreateToolhelp32Snapshot")?;
    // SAFETY: 清零的 POD，按要求设置 dwSize。
    let mut pe: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: 有效快照句柄与大小正确的结构体。
    let mut ok = unsafe { Process32FirstW(snap.as_raw_handle(), &mut pe) };
    if ok == 0 {
        return Err(last_err("Process32FirstW"));
    }
    while ok != 0 {
        if pe.th32ProcessID == pid {
            let n = pe
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(pe.szExeFile.len());
            return Ok(from_wide(&pe.szExeFile[..n]));
        }
        // SAFETY: 同上。
        ok = unsafe { Process32NextW(snap.as_raw_handle(), &mut pe) };
    }
    // ERROR_NOT_FOUND
    Err(FactErr::new("Process32NextW", 1168))
}

struct WtsMem {
    p: *mut WTS_PROCESS_INFO_EXW,
    count: u32,
}

impl Drop for WtsMem {
    fn drop(&mut self) {
        if !self.p.is_null() {
            // SAFETY: WTSEnumerateProcessesExW level 1 返回的内存，只释放一次。
            unsafe { WTSFreeMemoryExW(WTSTypeProcessInfoLevel1, self.p.cast(), self.count) };
        }
    }
}

/// `WTSEnumerateProcessesExW(level 1)` 中某 PID 的 pUserSid（只作快速拒绝，F4：空 SID 与复制失败分开）。
pub fn wts_user(pid: u32, known: &Known) -> WtsUser {
    let mut level = 1u32;
    let mut p: *mut u16 = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: 出参有效；成功后由 WtsMem 释放。
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
        return WtsUser::Failed(last_err("WTSEnumerateProcessesExW"));
    }
    let mem = WtsMem { p: p.cast(), count };
    if level != 1 {
        return WtsUser::Failed(FactErr::new(
            "WTSEnumerateProcessesExW",
            ERROR_INVALID_LEVEL,
        ));
    }
    for i in 0..count as usize {
        // SAFETY: level 1 输出是 `count` 个 WTS_PROCESS_INFO_EXW。
        let e = unsafe { &*mem.p.add(i) };
        if e.ProcessId != pid {
            continue;
        }
        if e.pUserSid.is_null() {
            return WtsUser::NullSid;
        }
        // SAFETY: WTS 分配内的非空 SID，`mem` 仍存活。
        return match unsafe { OwnedSid::copy_from(e.pUserSid, "WTSEnumerateProcessesExW") } {
            Ok(s) => WtsUser::User(known.classify(&s)),
            Err(_) => WtsUser::SidCopyFailed,
        };
    }
    WtsUser::NotInSnapshot
}
