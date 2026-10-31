//! Minimal raw UDP DNS query: encode a TXT question, send via UdpSocket,
//! decode the response extracting TXT character-strings. No external deps.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

const DNS_HEADER_LEN: usize = 12;
const DNS_PORT: u16 = 53;
const TYPE_TXT: u16 = 16;
const CLASS_IN: u16 = 1;

/// Query ID counter (arbitrary start, monotonically incrementing).
static QID: AtomicU16 = AtomicU16::new(0x1234);

#[derive(Debug)]
pub enum DnsError {
    Io(io::Error),
    MalformedPacket(&'static str),
    CompressionLoop,
    NoAnswer,
    Cancelled,
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO: {e}"),
            Self::MalformedPacket(msg) => write!(f, "malformed: {msg}"),
            Self::CompressionLoop => write!(f, "compression pointer loop"),
            Self::NoAnswer => write!(f, "no answer records"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl From<io::Error> for DnsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Encode a DNS query for TXT records of `name` (e.g. "whoami.ds.akahelp.net").
/// Returns (query_id, encoded_packet).
fn encode_txt_query(name: &str) -> (u16, Vec<u8>) {
    let id = QID.fetch_add(1, Ordering::Relaxed);
    let mut buf = Vec::with_capacity(64);

    // Header
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // RD=1, rest=0
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
    buf.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    // Question: QNAME
    for label in name.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    buf.push(0); // root label

    // QTYPE=TXT, QCLASS=IN
    buf.extend_from_slice(&TYPE_TXT.to_be_bytes());
    buf.extend_from_slice(&CLASS_IN.to_be_bytes());

    (id, buf)
}

/// Decode a DNS response, returning all TXT strings from the answer RRs.
/// Handles name compression pointers (RFC 1035 §4.1.4) and multi-chunk TXT.
fn decode_txt_response(packet: &[u8], expected_id: u16) -> Result<Vec<String>, DnsError> {
    if packet.len() < DNS_HEADER_LEN {
        return Err(DnsError::MalformedPacket("too short for header"));
    }

    let resp_id = u16::from_be_bytes([packet[0], packet[1]]);
    if resp_id != expected_id {
        return Err(DnsError::MalformedPacket("id mismatch"));
    }

    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    let rcode = (flags & 0x000F) as u8;
    if rcode != 0 {
        return Err(DnsError::MalformedPacket("non-zero RCODE"));
    }

    let ancount = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    if ancount == 0 {
        return Err(DnsError::NoAnswer);
    }

    // Skip question section(s)
    let mut pos = DNS_HEADER_LEN;
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    for _ in 0..qdcount {
        pos = skip_name(packet, pos)?;
        pos += 4; // QTYPE + QCLASS
    }

    // Parse answer RRs; collect TXT strings from first matching record.
    // (In practice the server returns only one TXT answer.)
    let mut all_strings: Vec<String> = Vec::new();
    for _ in 0..ancount {
        // NAME (possibly compressed)
        pos = skip_name(packet, pos)?;

        if pos + 10 > packet.len() {
            return Err(DnsError::MalformedPacket("truncated RR"));
        }
        let rtype = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let rdlength = u16::from_be_bytes([packet[pos + 8], packet[pos + 9]]) as usize;
        pos += 10; // TYPE(2) + CLASS(2) + TTL(4) + RDLENGTH(2)

        if pos + rdlength > packet.len() {
            return Err(DnsError::MalformedPacket("rdlength exceeds packet"));
        }

        if rtype == TYPE_TXT {
            let strings = parse_txt_rdata(&packet[pos..pos + rdlength])?;
            all_strings.extend(strings);
        }

        pos += rdlength;
    }

    if all_strings.is_empty() {
        return Err(DnsError::NoAnswer);
    }
    Ok(all_strings)
}

/// Skip a DNS name (handling compression pointers), return next position.
fn skip_name(packet: &[u8], mut pos: usize) -> Result<usize, DnsError> {
    let mut depth = 0u32;
    loop {
        if pos >= packet.len() {
            return Err(DnsError::MalformedPacket("name extends past packet"));
        }
        let len = packet[pos];
        if len == 0 {
            return Ok(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            // compression pointer: 2 bytes
            if pos + 1 >= packet.len() {
                return Err(DnsError::MalformedPacket("truncated pointer"));
            }
            // In skip mode, pointer terminates the name
            return Ok(pos + 2);
        }
        if len & 0xC0 != 0 {
            return Err(DnsError::MalformedPacket("reserved label bits"));
        }
        // normal label
        pos += 1 + len as usize;
        depth += 1;
        if depth > 128 {
            return Err(DnsError::CompressionLoop);
        }
    }
}

/// Parse TXT RDATA: a sequence of length-prefixed character-strings.
fn parse_txt_rdata(rdata: &[u8]) -> Result<Vec<String>, DnsError> {
    let mut strings = Vec::new();
    let mut pos = 0;
    while pos < rdata.len() {
        let len = rdata[pos] as usize;
        pos += 1;
        if pos + len > rdata.len() {
            return Err(DnsError::MalformedPacket("truncated char-string"));
        }
        strings.push(String::from_utf8_lossy(&rdata[pos..pos + len]).into_owned());
        pos += len;
    }
    Ok(strings)
}

/// `server_ip`（IPv4、IPv6 或 `[IPv6]`）→ `ip:53`。
///
/// 先按 `IpAddr` 解析再拼端口：原来的 `format!("{bare}:53").parse::<SocketAddr>()`
/// 对裸 IPv6（如 `2001:4860:4860::8888`）会得到 `…::8888:53`，解析失败，
/// 导致 IPv6 解析器永远查不了（Windows 网卡常同时配有 IPv6 DNS，M2）。
fn resolver_socket_addr(server_ip: &str) -> io::Result<SocketAddr> {
    // Strip brackets if caller passes "[::1]" form
    let bare = server_ip
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let ip: std::net::IpAddr = bare.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bad resolver addr: {e}"),
        )
    })?;
    Ok(SocketAddr::new(ip, DNS_PORT))
}

/// Send a DNS query over UDP to `server_ip:53`, wait for response with `timeout`.
/// Returns the raw response bytes. Auto-selects IPv4/IPv6 socket family.
fn send_udp_query(
    server_ip: &str,
    query: &[u8],
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, DnsError> {
    if cancelled() {
        return Err(DnsError::Cancelled);
    }
    let addr = resolver_socket_addr(server_ip)?;

    let bind_addr: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };

    let socket = UdpSocket::bind(bind_addr)?;
    socket.connect(addr)?;
    let slice = Duration::from_millis(200).min(timeout);
    socket.set_read_timeout(Some(slice))?;
    socket.set_write_timeout(Some(timeout))?;

    socket.send(query)?;

    // 4096 is well above any typical UDP DNS response (512 without EDNS,
    // up to 4096 with EDNS). whoami TXT is tiny, but be generous.
    // Read in short slices so a card deadline can close this wait early.
    let mut buf = vec![0u8; 4096];
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if cancelled() {
            return Err(DnsError::Cancelled);
        }
        if std::time::Instant::now() >= deadline {
            return Err(DnsError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "dns timeout",
            )));
        }
        match socket.recv(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                return Ok(buf);
            }
            Err(e)
                if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(e) => return Err(DnsError::Io(e)),
        }
    }
}

/// Convenience: encode + send + decode, returning joined TXT strings.
pub fn dns_txt_lookup(
    name: &str,
    server: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>, DnsError> {
    if cancelled() {
        return Err(DnsError::Cancelled);
    }
    let (id, query) = encode_txt_query(name);
    let response = send_udp_query(server, &query, timeout, cancelled)?;
    decode_txt_response(&response, id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn resolver_socket_addr_accepts_v4_v6_and_brackets() {
        assert_eq!(
            resolver_socket_addr("192.168.1.1").unwrap().to_string(),
            "192.168.1.1:53"
        );
        assert_eq!(
            resolver_socket_addr("2001:4860:4860::8888")
                .unwrap()
                .to_string(),
            "[2001:4860:4860::8888]:53"
        );
        assert_eq!(
            resolver_socket_addr("[::1]").unwrap().to_string(),
            "[::1]:53"
        );
        assert!(resolver_socket_addr("not-an-ip").is_err());
        assert!(resolver_socket_addr("").is_err());
    }

    use super::*;

    const MAX_NAME_EXPANSION_DEPTH: u32 = 10;

    /// Test-only: expand a DNS name resolving compression pointers, returning
    /// (name, next_pos_after_local). Production decode uses `skip_name`.
    fn expand_name(packet: &[u8], pos: usize) -> Result<(String, usize), DnsError> {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = pos;
        let mut next_after_local = pos;
        let mut jumped_via_pointer = false;
        let mut depth = 0u32;

        loop {
            if cur >= packet.len() {
                return Err(DnsError::MalformedPacket("name past end"));
            }
            let len = packet[cur];
            if len == 0 {
                if !jumped_via_pointer {
                    next_after_local = cur + 1;
                }
                break;
            }
            if len & 0xC0 == 0xC0 {
                if cur + 1 >= packet.len() {
                    return Err(DnsError::MalformedPacket("truncated pointer"));
                }
                let ptr = ((len as usize & 0x3F) << 8) | packet[cur + 1] as usize;
                if !jumped_via_pointer {
                    next_after_local = cur + 2;
                    jumped_via_pointer = true;
                }
                depth += 1;
                if depth > MAX_NAME_EXPANSION_DEPTH {
                    return Err(DnsError::CompressionLoop);
                }
                cur = ptr;
                continue;
            }
            if len & 0xC0 != 0 {
                return Err(DnsError::MalformedPacket("reserved label bits"));
            }
            let label_start = cur + 1;
            let label_end = label_start + len as usize;
            if label_end > packet.len() {
                return Err(DnsError::MalformedPacket("label past end"));
            }
            parts.push(String::from_utf8_lossy(&packet[label_start..label_end]).into_owned());
            cur = label_end;
        }

        Ok((parts.join("."), next_after_local))
    }

    // ---------- encode tests ----------

    #[test]
    fn lookup_stops_when_already_cancelled() {
        let err = dns_txt_lookup("example.com", "127.0.0.1", Duration::from_secs(5), &|| true);
        assert!(matches!(err, Err(DnsError::Cancelled)));
    }

    #[test]
    fn encode_known_query_bytes() {
        // Manually crafted expected packet for "example.com" TXT IN
        let (id, packet) = encode_txt_query("example.com");
        assert_eq!(&packet[0..2], &id.to_be_bytes());
        assert_eq!(&packet[2..4], &[0x01, 0x00]); // RD=1
        assert_eq!(&packet[4..6], &[0x00, 0x01]); // QDCOUNT=1
                                                  // Question section starts at 12
        let expected_qname: &[u8] = &[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ];
        assert_eq!(&packet[12..12 + expected_qname.len()], expected_qname);
        let qtype_class_offset = 12 + expected_qname.len();
        assert_eq!(
            &packet[qtype_class_offset..qtype_class_offset + 2],
            &[0x00, 0x10]
        ); // TXT=16
        assert_eq!(
            &packet[qtype_class_offset + 2..qtype_class_offset + 4],
            &[0x00, 0x01]
        ); // IN=1
    }

    #[test]
    fn encode_trailing_dot_ignored() {
        let (_, p1) = encode_txt_query("test.example.com.");
        let (_, p2) = encode_txt_query("test.example.com");
        // Compare payload after the 2-byte query ID (which increments).
        assert_eq!(&p1[2..], &p2[2..]);
    }

    // ---------- skip_name tests ----------

    #[test]
    fn skip_name_plain() {
        let pkt: Vec<u8> = vec![
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0, 0xFF,
        ];
        let next = skip_name(&pkt, 0).unwrap();
        assert_eq!(next, 13);
    }

    #[test]
    fn skip_name_compressed_pointer() {
        // At offset 0: 0xC0 0x0C → pointer to 12, next = 2
        let pkt: Vec<u8> = vec![0xC0, 0x0C, 0xAA, 0xBB];
        let next = skip_name(&pkt, 0).unwrap();
        assert_eq!(next, 2);
    }

    // ---------- expand_name tests ----------

    #[test]
    fn expand_name_simple() {
        // "www.example.com" encoded: 3 www 7 example 3 com 0
        let pkt: Vec<u8> = vec![
            3, b'w', b'w', b'w', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm',
            0, 0xFF,
        ];
        let (name, next) = expand_name(&pkt, 0).unwrap();
        assert_eq!(name, "www.example.com");
        assert_eq!(next, 17);
    }

    #[test]
    fn expand_name_with_pointer() {
        // "example\0" at offsets 0..8; pointer 0xC000 at offset 9-10 pointing back to 0
        let pkt: Vec<u8> = vec![
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0, 0xC0, 0x00, 0xAA,
        ];
        let (name, next) = expand_name(&pkt, 9).unwrap();
        assert_eq!(name, "example");
        assert_eq!(next, 11); // past the 2-byte pointer
    }

    #[test]
    fn expand_name_nested_pointer() {
        // At offset 0: "sub" label, at offset 4: pointer to offset 8, at offset 8: "net" root
        // 3 s u b 0 0xC0 0x06 ...
        // offset 6: 3 n e t 0
        let pkt: Vec<u8> = vec![3, b's', b'u', b'b', 0xC0, 0x06, 3, b'n', b'e', b't', 0];
        let (name, next) = expand_name(&pkt, 0).unwrap();
        assert_eq!(name, "sub.net");
        assert_eq!(next, 6); // past the pointer (2 bytes at offset 4)
    }

    #[test]
    fn expand_name_pointer_loop_detected() {
        // 0xC0 0x00 at offset 0 → pointer to self → loop
        let pkt: Vec<u8> = vec![0xC0, 0x00];
        let res = expand_name(&pkt, 0);
        assert!(matches!(res, Err(DnsError::CompressionLoop)));
    }

    // ---------- decode_txt_response tests ----------

    /// Helper: build a minimal DNS response with one TXT answer.
    fn build_response(id: u16, txt_chunks: &[&[u8]]) -> Vec<u8> {
        let mut pkt = Vec::new();
        // Header
        pkt.extend_from_slice(&id.to_be_bytes());
        pkt.extend_from_slice(&0x8180u16.to_be_bytes()); // QR=1, RD=1, RA=1
        pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        pkt.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
        pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
        pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
                                                    // Question: "test.example" (compressed not needed here)
        pkt.extend_from_slice(&[
            4, b't', b'e', b's', b't', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0,
        ]);
        pkt.extend_from_slice(&[0x00, 0x10]); // TXT
        pkt.extend_from_slice(&[0x00, 0x01]); // IN
                                              // Answer: NAME pointer to offset 12 (start of question)
        pkt.extend_from_slice(&[0xC0, 0x0C]);
        pkt.extend_from_slice(&[0x00, 0x10]); // TYPE=TXT
        pkt.extend_from_slice(&[0x00, 0x01]); // CLASS=IN
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL=60
        let rdlength: u16 = txt_chunks.iter().map(|c| 1 + c.len()).sum::<usize>() as u16;
        pkt.extend_from_slice(&rdlength.to_be_bytes());
        for chunk in txt_chunks {
            pkt.push(chunk.len() as u8);
            pkt.extend_from_slice(chunk);
        }
        pkt
    }

    #[test]
    fn decode_single_txt_chunk() {
        let pkt = build_response(0x1234, &[b"hello world"]);
        let strings = decode_txt_response(&pkt, 0x1234).unwrap();
        assert_eq!(strings, vec!["hello world"]);
    }

    #[test]
    fn decode_multi_txt_chunks() {
        let pkt = build_response(0xABCD, &[b"\"ip\" \"1.2.3.4\"", b" \"ns\" \"8.8.8.8\""]);
        let strings = decode_txt_response(&pkt, 0xABCD).unwrap();
        assert_eq!(strings.len(), 2);
        assert_eq!(strings[0], "\"ip\" \"1.2.3.4\"");
    }

    #[test]
    fn decode_wrong_id_fails() {
        let pkt = build_response(0x1111, &[b"data"]);
        let res = decode_txt_response(&pkt, 0x2222);
        assert!(matches!(res, Err(DnsError::MalformedPacket("id mismatch"))));
    }

    #[test]
    fn decode_rcode_nxdomain_fails() {
        let mut pkt = build_response(0x1234, &[b"data"]);
        // Set RCODE=3 (NXDOMAIN) in flags: 0x8183
        pkt[3] = 0x83;
        let res = decode_txt_response(&pkt, 0x1234);
        assert!(matches!(
            res,
            Err(DnsError::MalformedPacket("non-zero RCODE"))
        ));
    }

    #[test]
    fn decode_truncated_packet_fails() {
        let res = decode_txt_response(&[0x12, 0x34], 0x1234);
        assert!(matches!(res, Err(DnsError::MalformedPacket(_))));
    }

    // ---------- parse_txt_rdata edge cases ----------

    #[test]
    fn parse_txt_rdata_truncated_chunk() {
        // Length says 10 but only 5 bytes remain
        let rdata: &[u8] = &[10, b'a', b'b', b'c', b'd', b'e'];
        let res = parse_txt_rdata(rdata);
        assert!(matches!(res, Err(DnsError::MalformedPacket(_))));
    }

    #[test]
    fn parse_txt_rdata_empty() {
        let rdata: &[u8] = &[];
        let res = parse_txt_rdata(rdata).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn parse_txt_rdata_zero_len_string() {
        let rdata: &[u8] = &[0]; // one char-string of length 0
        let res = parse_txt_rdata(rdata).unwrap();
        assert_eq!(res, vec![""]);
    }
}
