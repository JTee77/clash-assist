//! GetExtendedTcpTable：端口 P 的监听者（规则 4）与已建立连接的服务端 PID（Ellie S1）。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_CLASS,
    TCP_TABLE_OWNER_PID_CONNECTIONS, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use super::util::AlignedBuf;
use crate::mihomo::trust::{FactErr, ListenerRow};

fn table(af: u16, class: TCP_TABLE_CLASS) -> Result<AlignedBuf, FactErr> {
    let mut size = 16 * 1024u32;
    for _ in 0..8 {
        let mut buf = AlignedBuf::new(size as usize);
        size = buf.len_bytes() as u32;
        // SAFETY: `size` 字节、8 字节对齐的缓冲区。
        let rc =
            unsafe { GetExtendedTcpTable(buf.as_mut_ptr(), &mut size, 0, u32::from(af), class, 0) };
        match rc {
            NO_ERROR => return Ok(buf),
            ERROR_INSUFFICIENT_BUFFER => size = size.saturating_add(4096),
            other => return Err(FactErr::new("GetExtendedTcpTable", other)),
        }
    }
    Err(FactErr::new(
        "GetExtendedTcpTable",
        ERROR_INSUFFICIENT_BUFFER,
    ))
}

/// `MIB_TCP{,6}TABLE_OWNER_PID` 的行（u32 计数，按行对齐，然后是行数组）。
///
/// # Safety
/// `buf` 必须是 GetExtendedTcpTable 写出的 `R` 行表。
unsafe fn rows<R: Copy>(buf: &AlignedBuf) -> Vec<R> {
    let base = buf.as_ptr() as *const u8;
    let n = *(base as *const u32) as usize;
    let off = std::mem::align_of::<R>().max(std::mem::size_of::<u32>());
    let max = buf.len_bytes().saturating_sub(off) / std::mem::size_of::<R>();
    let first = base.add(off) as *const R;
    (0..n.min(max)).map(|i| *first.add(i)).collect()
}

fn port_of(dw: u32) -> u16 {
    u16::from_be((dw & 0xFFFF) as u16)
}

fn v4(dw: u32) -> Ipv4Addr {
    // dwLocalAddr 按网络字节序存放在 DWORD 中。
    Ipv4Addr::from(dw.to_ne_bytes())
}

/// 端口 `port` 上的 IPv4 监听者。
pub fn listeners_v4(port: u16) -> Result<Vec<ListenerRow>, FactErr> {
    let b = table(AF_INET, TCP_TABLE_OWNER_PID_LISTENER)?;
    // SAFETY: AF_INET + OWNER_PID → MIB_TCPTABLE_OWNER_PID。
    let all: Vec<MIB_TCPROW_OWNER_PID> = unsafe { rows(&b) };
    Ok(all
        .into_iter()
        .filter(|r| port_of(r.dwLocalPort) == port)
        .map(|r| ListenerRow {
            ip: IpAddr::V4(v4(r.dwLocalAddr)),
            pid: r.dwOwningPid,
        })
        .collect())
}

/// 端口 `port` 上的 IPv6 监听者。
pub fn listeners_v6(port: u16) -> Result<Vec<ListenerRow>, FactErr> {
    let b = table(AF_INET6, TCP_TABLE_OWNER_PID_LISTENER)?;
    // SAFETY: AF_INET6 + OWNER_PID → MIB_TCP6TABLE_OWNER_PID。
    let all: Vec<MIB_TCP6ROW_OWNER_PID> = unsafe { rows(&b) };
    Ok(all
        .into_iter()
        .filter(|r| port_of(r.dwLocalPort) == port)
        .map(|r| ListenerRow {
            ip: IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)),
            pid: r.dwOwningPid,
        })
        .collect())
}

/// 已建立连接中「本端 = server、对端 = client」那一行的 PID（即接受这条连接的进程）。
pub fn connection_server_pid(
    server: SocketAddr,
    client: SocketAddr,
) -> Result<Option<u32>, FactErr> {
    match (server, client) {
        (SocketAddr::V4(s), SocketAddr::V4(c)) => {
            let b = table(AF_INET, TCP_TABLE_OWNER_PID_CONNECTIONS)?;
            // SAFETY: AF_INET + OWNER_PID → MIB_TCPTABLE_OWNER_PID。
            let all: Vec<MIB_TCPROW_OWNER_PID> = unsafe { rows(&b) };
            Ok(all
                .into_iter()
                .find(|r| {
                    v4(r.dwLocalAddr) == *s.ip()
                        && port_of(r.dwLocalPort) == s.port()
                        && v4(r.dwRemoteAddr) == *c.ip()
                        && port_of(r.dwRemotePort) == c.port()
                })
                .map(|r| r.dwOwningPid))
        }
        (SocketAddr::V6(s), SocketAddr::V6(c)) => {
            let b = table(AF_INET6, TCP_TABLE_OWNER_PID_CONNECTIONS)?;
            // SAFETY: AF_INET6 + OWNER_PID → MIB_TCP6TABLE_OWNER_PID。
            let all: Vec<MIB_TCP6ROW_OWNER_PID> = unsafe { rows(&b) };
            Ok(all
                .into_iter()
                .find(|r| {
                    Ipv6Addr::from(r.ucLocalAddr) == *s.ip()
                        && port_of(r.dwLocalPort) == s.port()
                        && Ipv6Addr::from(r.ucRemoteAddr) == *c.ip()
                        && port_of(r.dwRemotePort) == c.port()
                })
                .map(|r| r.dwOwningPid))
        }
        _ => Ok(None),
    }
}
