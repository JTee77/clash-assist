//! FlClash 官方 Helper 服务（Session 0）拉起的 `FlClashCore.exe`。
//!
//! 中完整性进程对这个内核 `OpenProcess` 会得到拒绝（Win32 5），WTS 的 pUserSid 也是空的。
//! 这里用不必打开进程的事实把内核钉到「当前用户安装的那一份」：NT 映像、快照文件名、
//! 父进程是已注册的 `FlClashHelperService`，且服务命令行的 `--owner-sid` 是当前用户。

use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFinalPathNameByHandleW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    VOLUME_NAME_NT,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfigW, QUERY_SERVICE_CONFIGW,
    SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_QUERY_CONFIG,
};

use super::identity;
use super::process::{nt_path, parent_pid, snapshot_name};
use super::util::{from_wide_ptr, last_err, owned_handle_invalid, wide};
use crate::mihomo::trust::{
    accept_flclash_helper_core, parse_helper_service_command, FactErr, HelperCoreFacts, Who,
    WtsUser,
};

const SERVICE_NAME: &str = "FlClashHelperService";

/// 证明 `pid` 是当前用户的 FlClash Helper 拉起的内核。
/// 失败原因写入监听日志；调用方保持未验证，不建连、不发送 secret。
pub fn prove_flclash_helper_core(pid: u32) -> Result<String, String> {
    let known = identity::known().map_err(|e| e.to_string())?;
    let wts = super::process::wts_user(pid, known);
    let wts_foreign = matches!(wts, WtsUser::User(user) if user.who == Who::Other);
    let parent = parent_pid(pid).map_err(|e| format!("parent-pid: {e}"))?;
    let (service_nt, owner_sid) = registered_helper().map_err(|e| format!("service: {e}"))?;
    let facts = HelperCoreFacts {
        nt_image: nt_path(pid).ok(),
        snapshot_name: snapshot_name(pid).ok(),
        parent_snapshot_name: snapshot_name(parent).ok(),
        parent_nt_image: nt_path(parent).ok(),
        service_nt_image: Some(service_nt),
        owner_matches: owner_sid.eq_ignore_ascii_case(&known.me_str),
        wts_foreign,
        token_other_user: false,
    };
    accept_flclash_helper_core(&facts).map_err(str::to_string)
}

fn registered_helper() -> Result<(String, String), FactErr> {
    let binary = service_binary_path()?;
    let (exe, owner) = parse_helper_service_command(&binary)
        .ok_or_else(|| FactErr::new("QueryServiceConfigW", ERROR_INSUFFICIENT_BUFFER))?;
    let nt = file_nt_path(&service_exe_path(&exe))?;
    Ok((nt, owner))
}

/// SCM 有时在 ImagePath 前加上 `\??\` 或 `\\?\`。CreateFileW 要的是 Win32 路径。
fn service_exe_path(exe: &str) -> String {
    let exe = exe.trim();
    exe.strip_prefix(r"\\?\")
        .or_else(|| exe.strip_prefix(r"\??\"))
        .unwrap_or(exe)
        .to_string()
}

fn service_binary_path() -> Result<String, FactErr> {
    // SAFETY: 本地 SCM；句柄由 CloseServiceHandle 关闭。
    let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if scm.is_null() {
        return Err(last_err("OpenSCManagerW"));
    }
    let name = wide(SERVICE_NAME);
    // SAFETY: 已打开的 SCM；服务名是 NUL 结尾的 UTF-16。
    let svc = unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_QUERY_CONFIG) };
    unsafe { CloseServiceHandle(scm) };
    if svc.is_null() {
        return Err(last_err("OpenServiceW"));
    }
    let path = query_binary_path(svc);
    unsafe { CloseServiceHandle(svc) };
    path
}

fn query_binary_path(svc: SC_HANDLE) -> Result<String, FactErr> {
    let mut needed = 0u32;
    // SAFETY: 用空缓冲询问所需字节数。这次失败是预期的（ERROR_INSUFFICIENT_BUFFER）。
    unsafe { QueryServiceConfigW(svc, std::ptr::null_mut(), 0, &mut needed) };
    let min = std::mem::size_of::<QUERY_SERVICE_CONFIGW>() as u32;
    if needed < min || needed > 64 * 1024 {
        return Err(last_err("QueryServiceConfigW"));
    }
    // 结构体含指针，缓冲必须按指针对齐；字节 Vec 只有 1 字节对齐。
    let mut buf = super::util::AlignedBuf::new(needed as usize);
    let ok = unsafe {
        QueryServiceConfigW(
            svc,
            buf.as_mut_ptr().cast(),
            buf.len_bytes() as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(last_err("QueryServiceConfigW"));
    }
    // SAFETY: 成功时缓冲区开头是 QUERY_SERVICE_CONFIGW，lpBinaryPathName 指向同一块缓冲。
    let cfg = unsafe { &*buf.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    unsafe { from_wide_ptr(cfg.lpBinaryPathName) }
        .ok_or_else(|| FactErr::new("QueryServiceConfigW", ERROR_INSUFFICIENT_BUFFER))
}

fn file_nt_path(path: &str) -> Result<String, FactErr> {
    let wide_path = wide(path);
    // SAFETY: 只读属性打开磁盘文件；句柄交给 OwnedHandle。
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    let file = owned_handle_invalid(handle, "CreateFileW")?;
    let mut buf = vec![0u16; 32_768];
    // SAFETY: 有效文件句柄与足够大的 UTF-16 缓冲。
    let n = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            VOLUME_NAME_NT,
        )
    };
    if n == 0 || n as usize >= buf.len() {
        return Err(last_err("GetFinalPathNameByHandleW"));
    }
    Ok(super::util::from_wide(&buf[..n as usize]))
}
