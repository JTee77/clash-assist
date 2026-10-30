//! Small safe building blocks over Win32: error capture, UTF-16 helpers and
//! RAII owners for `LocalAlloc`-ed memory and kernel handles.

use std::ffi::c_void;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS,
};

use crate::report::Step;

/// A failed Win32/NT call: which API, and the Win32 error (plus NTSTATUS for Nt* calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinErr {
    pub api: &'static str,
    pub code: u32,
    pub ntstatus: Option<NTSTATUS>,
}

impl WinErr {
    /// Capture `GetLastError()` right after a failed call.
    pub fn last(api: &'static str) -> Self {
        // SAFETY: GetLastError has no preconditions.
        let code = unsafe { GetLastError() };
        WinErr {
            api,
            code,
            ntstatus: None,
        }
    }

    /// For APIs that return the error code directly (GetSecurityInfo, GetExtendedTcpTable, ...).
    pub fn code(api: &'static str, code: u32) -> Self {
        WinErr {
            api,
            code,
            ntstatus: None,
        }
    }

    pub fn nt(api: &'static str, status: NTSTATUS) -> Self {
        // SAFETY: RtlNtStatusToDosError is a pure table lookup.
        let code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
        WinErr {
            api,
            code,
            ntstatus: Some(status),
        }
    }

    pub fn step(self, step: &'static str) -> Step {
        let s = Step::win32(step, self.api, self.code);
        match self.ntstatus {
            Some(st) => s.with_ntstatus(st),
            None => s,
        }
    }

    pub fn to_json(self) -> serde_json::Value {
        // Reuse the step serialisation so field names stay identical everywhere.
        let mut v = self.step("_").to_json();
        if let Some(m) = v.as_object_mut() {
            m.remove("step");
            m.remove("ok");
        }
        v
    }
}

/// NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 slice (no NUL) to String, lossy.
pub fn from_wide(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
}

/// Read a NUL-terminated wide string.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string valid for reads.
pub unsafe fn from_wide_ptr(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    // Bounded scan: no Win32 string we read here is anywhere near 64K chars.
    while len < 65_536 && *p.add(len) != 0 {
        len += 1;
    }
    Some(from_wide(std::slice::from_raw_parts(p, len)))
}

/// Last path component (handles both `\` and `/`, and NT paths such as `\Device\HarddiskVolume3\x.exe`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Owner of memory that must be released with `LocalFree`
/// (security descriptors, SDDL strings, SIDs from ConvertStringSidToSidW).
pub struct LocalBox(*mut c_void);

impl LocalBox {
    /// # Safety
    /// `p` must be null or a pointer returned by an API documented to require `LocalFree`,
    /// and must not be freed elsewhere.
    pub unsafe fn from_raw(p: *mut c_void) -> Self {
        LocalBox(p)
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for LocalBox {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: invariant of `from_raw`; freed exactly once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// Wrap a handle that the API reports as NULL on failure.
pub fn owned_handle_nullable(h: HANDLE, api: &'static str) -> Result<OwnedHandle, WinErr> {
    if h.is_null() {
        return Err(WinErr::last(api));
    }
    // SAFETY: non-null handle freshly returned to us by the API; we take sole ownership.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

/// Wrap a handle that the API reports as INVALID_HANDLE_VALUE on failure.
pub fn owned_handle_invalid(h: HANDLE, api: &'static str) -> Result<OwnedHandle, WinErr> {
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(WinErr::last(api));
    }
    // SAFETY: valid handle freshly returned to us by the API; we take sole ownership.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

/// Pointer-aligned zeroed buffer of at least `bytes` bytes (for variable-length Win32 structs).
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
