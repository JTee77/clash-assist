//! Win32 小工具：错误捕获、UTF-16 转换、LocalFree / 内核句柄的 RAII 持有者。

use std::ffi::c_void;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};

use crate::mihomo::trust::FactErr;

/// 失败调用之后立即读取 `GetLastError()`。
pub fn last_err(api: &'static str) -> FactErr {
    // SAFETY: GetLastError 无前置条件。
    FactErr::new(api, unsafe { GetLastError() })
}

/// 以 NUL 结尾的 UTF-16 副本。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
}

/// 读取以 NUL 结尾的宽字符串（最多 64K 个字符）。
///
/// # Safety
/// `p` 为空，或指向可读的、以 NUL 结尾的 UTF-16 字符串。
pub unsafe fn from_wide_ptr(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while len < 65_536 && *p.add(len) != 0 {
        len += 1;
    }
    Some(from_wide(std::slice::from_raw_parts(p, len)))
}

/// 必须用 `LocalFree` 释放的内存（安全描述符、SDDL 字符串、ConvertStringSidToSidW 的 SID）。
pub struct LocalBox(*mut c_void);

impl LocalBox {
    /// # Safety
    /// `p` 为空，或是文档要求用 `LocalFree` 释放、且不会在别处释放的指针。
    pub unsafe fn from_raw(p: *mut c_void) -> Self {
        LocalBox(p)
    }
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for LocalBox {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: from_raw 的约定；只释放一次。
            unsafe { LocalFree(self.0) };
        }
    }
}

/// 失败时返回 NULL 的句柄。
pub fn owned_handle_nullable(h: HANDLE, api: &'static str) -> Result<OwnedHandle, FactErr> {
    if h.is_null() {
        return Err(last_err(api));
    }
    // SAFETY: API 刚返回的非空句柄，由我们独占。
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

/// 失败时返回 INVALID_HANDLE_VALUE 的句柄。
pub fn owned_handle_invalid(h: HANDLE, api: &'static str) -> Result<OwnedHandle, FactErr> {
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(last_err(api));
    }
    // SAFETY: API 刚返回的有效句柄，由我们独占。
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

/// 8 字节对齐、清零的缓冲区（可变长 Win32 结构体）。
pub struct AlignedBuf(Vec<u64>);

impl AlignedBuf {
    pub fn new(bytes: usize) -> Self {
        AlignedBuf(vec![0u64; bytes.div_ceil(8).max(1)])
    }
    pub fn len_bytes(&self) -> usize {
        self.0.len() * 8
    }
    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }
    pub fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr().cast()
    }
}
