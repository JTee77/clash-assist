//! `trust::PipeFacts` / `trust::ProcessFacts` 的真实实现。

use std::os::windows::io::RawHandle;

use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

use super::identity::Known;
use super::process::{nt_path, snapshot_name, wts_user, ProcessProbe};
use super::util::last_err;
use super::{paths, security};
use crate::mihomo::trust::{FactErr, Ident, PipeFacts, ProcessFacts, SecurityFact, WtsUser};

/// 一个已打开管道句柄上的事实。所有查询都针对这个句柄（L1 与连接绑定）。
pub struct OsPipeFacts<'a> {
    h: RawHandle,
    known: &'a Known,
    proc_: Option<ProcessProbe>,
}

impl<'a> OsPipeFacts<'a> {
    pub fn new(h: RawHandle, known: &'a Known) -> Self {
        OsPipeFacts {
            h,
            known,
            proc_: None,
        }
    }
    fn probe(&mut self, pid: u32) -> &mut ProcessProbe {
        if self.proc_.as_ref().map(|p| p.pid()) != Some(pid) {
            self.proc_ = Some(ProcessProbe::new(pid));
        }
        self.proc_.as_mut().expect("just set")
    }
}

impl PipeFacts for OsPipeFacts<'_> {
    fn security(&mut self) -> Result<SecurityFact, FactErr> {
        security::security(self.h, self.known)
    }
    fn server_pid(&mut self) -> Result<u32, FactErr> {
        let mut pid = 0u32;
        // SAFETY: 有效管道句柄；出参有效。
        if unsafe { GetNamedPipeServerProcessId(self.h, &mut pid) } == 0 {
            return Err(last_err("GetNamedPipeServerProcessId"));
        }
        Ok(pid)
    }
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.probe(pid).win32_path()
    }
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr> {
        nt_path(pid)
    }
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr> {
        snapshot_name(pid)
    }
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr> {
        let known = self.known;
        self.probe(pid).token_user(known)
    }
    fn under_service_cores_dir(&mut self, path: &str, nt: bool) -> Option<bool> {
        paths::under_service_cores_dir(path, nt)
    }
}

/// TCP 监听进程的事实；L2 ① 与 L3 共用同一进程句柄（S1）。
pub struct OsProcessFacts<'a> {
    known: &'a Known,
    proc_: Option<ProcessProbe>,
}

impl<'a> OsProcessFacts<'a> {
    pub fn new(known: &'a Known) -> Self {
        OsProcessFacts { known, proc_: None }
    }
    fn probe(&mut self, pid: u32) -> &mut ProcessProbe {
        if self.proc_.as_ref().map(|p| p.pid()) != Some(pid) {
            self.proc_ = Some(ProcessProbe::new(pid));
        }
        self.proc_.as_mut().expect("just set")
    }
}

impl ProcessFacts for OsProcessFacts<'_> {
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.probe(pid).win32_path()
    }
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr> {
        nt_path(pid)
    }
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr> {
        snapshot_name(pid)
    }
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr> {
        let known = self.known;
        self.probe(pid).token_user(known)
    }
    fn wts_user(&mut self, pid: u32) -> WtsUser {
        wts_user(pid, self.known)
    }
}
