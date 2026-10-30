//! `tcp-probe <port>`: the same data collection as plan §2.1 rule 4, without connecting.
//! GetExtendedTcpTable(AF_INET / AF_INET6, TCP_TABLE_OWNER_PID_LISTENER) → PIDs →
//! L2 ①②③, L3, WTS per PID. No socket is ever opened.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use super::context::Context;
use super::process::{identity_steps, WtsSnapshot};
use super::util::{AlignedBuf, WinErr};
use crate::report::{Report, Step};

/// Fetch the raw listener table for one address family.
fn table(af: u16) -> Result<AlignedBuf, WinErr> {
    let mut size = 16 * 1024u32;
    for _ in 0..8 {
        let mut buf = AlignedBuf::new(size as usize);
        size = buf.len_bytes() as u32;
        // SAFETY: buffer of `size` bytes, 8-aligned.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr(),
                &mut size,
                0,
                u32::from(af),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match rc {
            NO_ERROR => return Ok(buf),
            ERROR_INSUFFICIENT_BUFFER => size = size.saturating_add(4096),
            other => return Err(WinErr::code("GetExtendedTcpTable", other)),
        }
    }
    Err(WinErr::code(
        "GetExtendedTcpTable",
        ERROR_INSUFFICIENT_BUFFER,
    ))
}

/// Rows of a `MIB_TCP{,6}TABLE_OWNER_PID` buffer (u32 count, padding to row alignment, rows).
///
/// # Safety
/// `buf` must hold a table of `R` rows as written by GetExtendedTcpTable.
unsafe fn rows<R: Copy>(buf: &AlignedBuf) -> Vec<R> {
    let base = buf.as_ptr() as *const u8;
    let n = *(base as *const u32) as usize;
    // dwNumEntries is followed by `table[ANYSIZE_ARRAY]`, aligned for R.
    let off = std::mem::align_of::<R>().max(std::mem::size_of::<u32>());
    let max = buf.len_bytes().saturating_sub(off) / std::mem::size_of::<R>();
    let n = n.min(max);
    let first = base.add(off) as *const R;
    (0..n).map(|i| *first.add(i)).collect()
}

fn port_of(dw: u32) -> u16 {
    u16::from_be((dw & 0xFFFF) as u16)
}

pub fn run(port: u16, ctx: &Context, report: &mut Report) {
    let mut pids = BTreeSet::new();

    let v4 = table(AF_INET).map(|b| {
        // SAFETY: AF_INET + OWNER_PID class → MIB_TCPTABLE_OWNER_PID.
        let all: Vec<MIB_TCPROW_OWNER_PID> = unsafe { rows(&b) };
        let total = all.len();
        let entries: Vec<Value> = all
            .into_iter()
            .filter(|r| port_of(r.dwLocalPort) == port)
            .map(|r| {
                pids.insert(r.dwOwningPid);
                json!({
                    "local_addr": Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes()).to_string(),
                    "local_port": port_of(r.dwLocalPort),
                    "state": r.dwState,
                    "pid": r.dwOwningPid,
                })
            })
            .collect();
        json!({ "entries": entries, "total_listeners": total })
    });
    report.push(match v4 {
        Ok(v) => Step::ok("tcp_table_v4", v),
        Err(e) => e.step("tcp_table_v4"),
    });

    let v6 = table(AF_INET6).map(|b| {
        // SAFETY: AF_INET6 + OWNER_PID class → MIB_TCP6TABLE_OWNER_PID.
        let all: Vec<MIB_TCP6ROW_OWNER_PID> = unsafe { rows(&b) };
        let total = all.len();
        let entries: Vec<Value> = all
            .into_iter()
            .filter(|r| port_of(r.dwLocalPort) == port)
            .map(|r| {
                pids.insert(r.dwOwningPid);
                json!({
                    "local_addr": Ipv6Addr::from(r.ucLocalAddr).to_string(),
                    "local_scope_id": r.dwLocalScopeId,
                    "local_port": port_of(r.dwLocalPort),
                    "state": r.dwState,
                    "pid": r.dwOwningPid,
                })
            })
            .collect();
        json!({ "entries": entries, "total_listeners": total })
    });
    report.push(match v6 {
        Ok(v) => Step::ok("tcp_table_v6", v),
        Err(e) => e.step("tcp_table_v6"),
    });

    report.push(Step::ok(
        "listener_pids",
        json!(pids.iter().collect::<Vec<_>>()),
    ));
    if pids.is_empty() {
        return;
    }
    let wts = WtsSnapshot::take();
    for pid in pids {
        for s in identity_steps(pid, &ctx.known, &wts) {
            report.push(s);
        }
    }
}
