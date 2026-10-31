//! Windows 系统 DNS 发现的 FFI（windows-plan §2.3，M2）。
//!
//! 只采集事实（网卡及其 DNS、默认路由网卡、Verge 的 `tun.device`），不做判定；
//! 选择逻辑在 `dns::select_windows_resolvers`（纯函数，全平台单测）。
//! FFI 约定沿用 M1 的 `win/`：`AlignedBuf` 承载可变长结构体，错误用 `FactErr`，
//! 每个 `unsafe` 写明前置条件；读 yaml 只走 `safe_fs`（S-R1）。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GetBestRoute2, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_IPV4_ENABLED, MIB_IPFORWARD_ROW2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_INET, SOCKET_ADDRESS,
};

use crate::dns::{parse_tun_device, AdapterDns, WinDnsFacts};
use crate::mihomo::trust::FactErr;
use crate::win::util::{from_wide_ptr, AlignedBuf};

/// §2.3：缓冲区先分配 15 KB，遇到 `ERROR_BUFFER_OVERFLOW` 按返回的大小重试。
const INITIAL_BUF: u32 = 15 * 1024;
const MAX_TRIES: usize = 4;

/// 默认路由探测目标：先 IPv4（1.1.1.1），没有 IPv4 默认路由时再试 IPv6。
const ROUTE_PROBE_V4: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
const ROUTE_PROBE_V6: Ipv6Addr = Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111);

/// 采集 `list_dns_resolvers_blocking` 需要的全部事实。
///
/// 只有 `GetAdaptersAddresses` 失败才返回错误；默认路由、yaml 读取失败只记录说明。
pub fn collect() -> Result<WinDnsFacts, FactErr> {
    let adapters = adapters()?;
    let (default_if, default_route_note) = match default_route_if() {
        Ok(i) => (Some(i), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let (tun_hint, tun_hint_from) = match verge_tun_device() {
        Some((d, from)) => (Some(d), Some(from)),
        None => (None, None),
    };
    Ok(WinDnsFacts {
        adapters,
        default_if,
        default_route_note,
        tun_hint,
        tun_hint_from,
    })
}

/// `GetAdaptersAddresses(AF_UNSPEC, SKIP_ANYCAST | SKIP_MULTICAST)`，每张网卡映射为 `AdapterDns`。
///
/// 不设 `GAA_FLAG_SKIP_FRIENDLY_NAME`（需要网卡名识别 TUN）；DNS 服务器地址默认就会返回。
pub fn adapters() -> Result<Vec<AdapterDns>, FactErr> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = INITIAL_BUF;
    for _ in 0..MAX_TRIES {
        let mut buf = AlignedBuf::new(size as usize);
        size = buf.len_bytes() as u32;
        // SAFETY: `buf` 至少 `size` 字节、8 字节对齐（满足 IP_ADAPTER_ADDRESSES_LH 的对齐）；
        // reserved 必须为 NULL。
        let rc = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                flags,
                std::ptr::null(),
                buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };
        match rc {
            // SAFETY: 返回 NO_ERROR 时 `buf` 内是以 Next 串起的 IP_ADAPTER_ADDRESSES_LH 链表，
            // 所有指针都指向 `buf` 内部，`buf` 在 walk 期间存活。
            NO_ERROR => return Ok(unsafe { walk(&buf) }),
            ERROR_NO_DATA => return Ok(Vec::new()),
            ERROR_BUFFER_OVERFLOW => size = size.max(INITIAL_BUF).saturating_add(1024),
            other => return Err(FactErr::new("GetAdaptersAddresses", other)),
        }
    }
    Err(FactErr::new("GetAdaptersAddresses", ERROR_BUFFER_OVERFLOW))
}

/// # Safety
/// `buf` 必须是 `GetAdaptersAddresses` 成功写出的缓冲区。
unsafe fn walk(buf: &AlignedBuf) -> Vec<AdapterDns> {
    let mut out = Vec::new();
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // 防御：链表异常时最多走 1024 张网卡。
    let mut guard = 0usize;
    while !p.is_null() && guard < 1024 {
        guard += 1;
        let a = &*p;
        let v4_index = a.Anonymous1.Anonymous.IfIndex;
        let if_index = if v4_index != 0 {
            v4_index
        } else {
            a.Ipv6IfIndex
        };
        let mut dns = Vec::new();
        let mut d = a.FirstDnsServerAddress;
        let mut dguard = 0usize;
        while !d.is_null() && dguard < 64 {
            dguard += 1;
            if let Some(ip) = sockaddr_ip(&(*d).Address) {
                dns.push(ip);
            }
            d = (*d).Next;
        }
        out.push(AdapterDns {
            if_index,
            friendly_name: from_wide_ptr(a.FriendlyName).unwrap_or_default(),
            description: from_wide_ptr(a.Description).unwrap_or_default(),
            if_type: a.IfType,
            oper_up: a.OperStatus == IfOperStatusUp,
            dns,
            ipv4_enabled: a.Anonymous2.Flags & IP_ADAPTER_IPV4_ENABLED != 0,
            ipv4_metric: a.Ipv4Metric,
            ipv6_metric: a.Ipv6Metric,
        });
        p = a.Next;
    }
    out
}

/// # Safety
/// `sa.lpSockaddr` 为空，或指向至少 `iSockaddrLength` 字节的有效 sockaddr。
unsafe fn sockaddr_ip(sa: &SOCKET_ADDRESS) -> Option<IpAddr> {
    let p = sa.lpSockaddr;
    if p.is_null() || sa.iSockaddrLength <= 0 {
        return None;
    }
    let len = sa.iSockaddrLength as usize;
    let family = std::ptr::read_unaligned(std::ptr::addr_of!((*p).sa_family));
    if family == AF_INET && len >= std::mem::size_of::<SOCKADDR_IN>() {
        let sin = std::ptr::read_unaligned(p.cast::<SOCKADDR_IN>());
        // S_addr 按网络字节序存放。
        Some(IpAddr::V4(Ipv4Addr::from(
            sin.sin_addr.S_un.S_addr.to_ne_bytes(),
        )))
    } else if family == AF_INET6 && len >= std::mem::size_of::<SOCKADDR_IN6>() {
        let sin6 = std::ptr::read_unaligned(p.cast::<SOCKADDR_IN6>());
        Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.u.Byte)))
    } else {
        None
    }
}

fn sockaddr_inet(ip: IpAddr) -> SOCKADDR_INET {
    let mut sa = SOCKADDR_INET::default();
    match ip {
        IpAddr::V4(v4) => {
            let mut sin = SOCKADDR_IN {
                sin_family: AF_INET,
                ..Default::default()
            };
            sin.sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.octets());
            sa.Ipv4 = sin;
        }
        IpAddr::V6(v6) => {
            let mut sin6 = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                ..Default::default()
            };
            sin6.sin6_addr.u.Byte = v6.octets();
            sa.Ipv6 = sin6;
        }
    }
    sa
}

fn best_route_if(dest: IpAddr) -> Result<u32, FactErr> {
    let dst = sockaddr_inet(dest);
    let mut row = MIB_IPFORWARD_ROW2::default();
    let mut src = SOCKADDR_INET::default();
    // SAFETY: 目标地址是本地构造的 SOCKADDR_INET；接口 LUID / 源地址可为 NULL；
    // 输出参数指向本地变量。
    let rc = unsafe {
        GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &dst,
            0,
            &mut row,
            &mut src,
        )
    };
    if rc == NO_ERROR {
        Ok(row.InterfaceIndex)
    } else {
        Err(FactErr::new("GetBestRoute2", rc))
    }
}

/// 默认路由网卡（§2.3）：`GetBestRoute2` 到 1.1.1.1 的路由所在网卡；没有 IPv4 默认路由时试 IPv6。
pub fn default_route_if() -> Result<u32, FactErr> {
    best_route_if(IpAddr::V4(ROUTE_PROBE_V4))
        .or_else(|e4| best_route_if(IpAddr::V6(ROUTE_PROBE_V6)).map_err(|_| e4))
}

/// Verge yaml 中的 `tun.device`：先读 `clash-verge.yaml`（较新），再读 `config.yaml`。
/// 返回（网卡名，来源文件名）。只走 `safe_fs`。
fn verge_tun_device() -> Option<(String, String)> {
    for path in [
        crate::platform::verge_clash_verge_yaml_path(),
        crate::platform::verge_config_path(),
    ] {
        let Ok(content) = crate::safe_fs::read_to_string(&path) else {
            continue;
        };
        if let Some(d) = parse_tun_device(&content) {
            let from = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Some((d, from));
        }
    }
    None
}
