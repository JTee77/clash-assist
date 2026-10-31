//! 路径辅助：`%ProgramData%`、DOS 路径 → NT 路径（QueryDosDeviceW），服务模式内核目录判定。

use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

use super::util::{from_wide, from_wide_ptr, wide};

/// `%ProgramData%`（SHGetKnownFolderPath，不读环境变量）。
pub fn program_data() -> Option<String> {
    let mut p: *mut u16 = std::ptr::null_mut();
    // SAFETY: 出参有效；无论成败都用 CoTaskMemFree 释放。
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, std::ptr::null_mut(), &mut p) };
    // SAFETY: 成功时 `p` 是以 NUL 结尾的字符串。
    let s = if hr >= 0 {
        unsafe { from_wide_ptr(p) }
    } else {
        None
    };
    // SAFETY: SHGetKnownFolderPath 的释放约定（空指针亦可）。
    unsafe { CoTaskMemFree(p as *const _) };
    s
}

/// `C:\a\b` → `\Device\HarddiskVolume3\a\b`。
pub fn dos_to_nt(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return None;
    }
    let drive = wide(&path[..2]);
    let mut buf = vec![0u16; 1024];
    // SAFETY: NUL 结尾的输入与给定长度的输出缓冲区。
    let n = unsafe { QueryDosDeviceW(drive.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 {
        return None;
    }
    // 输出是多个以 NUL 分隔的字符串，取第一个。
    let first = buf.iter().position(|&c| c == 0).unwrap_or(n as usize);
    Some(format!("{}{}", from_wide(&buf[..first]), &path[2..]))
}

/// 服务模式内核是否位于 `%ProgramData%\clash-verge-service\cores\` 下（统一换成 NT 路径后比较，
/// 大小写不敏感）。只用于告警。
pub fn under_service_cores_dir(path: &str, is_nt: bool) -> Option<bool> {
    let dir = format!(
        "{}\\clash-verge-service\\cores\\",
        program_data()?.trim_end_matches('\\')
    );
    let dir_nt = dos_to_nt(&dir)?;
    let path_nt = if is_nt {
        path.to_string()
    } else {
        dos_to_nt(path)?
    };
    Some(path_nt.to_lowercase().starts_with(&dir_nt.to_lowercase()))
}
