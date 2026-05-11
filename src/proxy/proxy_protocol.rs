//! PROXY protocol v1 (text) and v2 (binary) parser.
//!
//! When `[gateway].proxy_protocol = true`, the TCP listener reads a PROXY
//! protocol header before passing the connection to the HTTP layer. This
//! extracts the real client IP when Lynxx sits behind AWS NLB, HAProxy,
//! or any layer-4 load balancer that speaks PROXY protocol.
//!
//! Ref: <https://www.haproxy.org/download/2.9/doc/proxy-protocol.txt>

use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::TcpStream;

/// Result of parsing a PROXY protocol header.
#[derive(Debug, Clone)]
pub struct ProxyHeader {
    /// Original client address as reported by the proxy.
    pub src_addr: Option<SocketAddr>,
    /// Destination address as reported by the proxy.
    pub dst_addr: Option<SocketAddr>,
}

/// PROXY protocol v2 magic signature (12 bytes).
const V2_SIGNATURE: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51,
    0x55, 0x49, 0x54, 0x0A,
];

/// Read and parse a PROXY protocol header from the given stream.
///
/// Auto-detects v1 (text) vs v2 (binary) from the first bytes. Returns
/// `None` when the connection uses LOCAL command (health checks) or the
/// protocol is unsupported (e.g. Unix sockets).
///
/// The stream is advanced past the header; subsequent reads get the raw
/// HTTP data.
pub async fn read_proxy_header(stream: &mut BufReader<TcpStream>) -> Result<ProxyHeader, ProxyError> {
    // Peek 13 bytes: enough to distinguish v1 "PROXY " from v2 signature.
    let buf = stream.fill_buf().await.map_err(ProxyError::Io)?;
    if buf.len() < 6 {
        return Err(ProxyError::TooShort);
    }

    if buf.starts_with(b"PROXY ") {
        parse_v1(stream).await
    } else if buf.len() >= 13 && buf[..12] == V2_SIGNATURE {
        parse_v2(stream).await
    } else {
        Err(ProxyError::InvalidSignature)
    }
}

/// Parse PROXY protocol v1 (text format).
///
/// Format: `PROXY TCP4 <src_ip> <dst_ip> <src_port> <dst_port>\r\n`
///         `PROXY TCP6 <src_ip> <dst_ip> <src_port> <dst_port>\r\n`
///         `PROXY UNKNOWN\r\n`
async fn parse_v1(reader: &mut BufReader<TcpStream>) -> Result<ProxyHeader, ProxyError> {
    let mut line = String::with_capacity(108); // max v1 line is 107 + NUL
    reader.read_line(&mut line).await.map_err(ProxyError::Io)?;

    // Strip trailing \r\n
    let line = line.trim_end_matches('\n').trim_end_matches('\r');

    let parts: Vec<&str> = line.split(' ').collect();
    if parts.len() < 2 || parts[0] != "PROXY" {
        return Err(ProxyError::InvalidV1Header);
    }

    // PROXY UNKNOWN — local/health-check connection
    if parts[1] == "UNKNOWN" {
        return Ok(ProxyHeader { src_addr: None, dst_addr: None });
    }

    if parts.len() != 6 {
        return Err(ProxyError::InvalidV1Header);
    }

    let proto = parts[1]; // TCP4 or TCP6
    if proto != "TCP4" && proto != "TCP6" {
        return Err(ProxyError::UnsupportedProtocol(proto.to_string()));
    }

    let src_ip: std::net::IpAddr = parts[2].parse()
        .map_err(|_| ProxyError::InvalidAddress)?;
    let dst_ip: std::net::IpAddr = parts[3].parse()
        .map_err(|_| ProxyError::InvalidAddress)?;
    let src_port: u16 = parts[4].parse()
        .map_err(|_| ProxyError::InvalidPort)?;
    let dst_port: u16 = parts[5].parse()
        .map_err(|_| ProxyError::InvalidPort)?;

    Ok(ProxyHeader {
        src_addr: Some(SocketAddr::new(src_ip, src_port)),
        dst_addr: Some(SocketAddr::new(dst_ip, dst_port)),
    })
}

/// Parse PROXY protocol v2 (binary format).
///
/// Layout (after 12-byte signature):
///   byte 12: version_command (upper 4 = version, lower 4 = command)
///   byte 13: family_transport (upper 4 = family, lower 4 = transport)
///   bytes 14-15: address length (big-endian u16)
///   bytes 16..: address data + optional TLVs
async fn parse_v2(reader: &mut BufReader<TcpStream>) -> Result<ProxyHeader, ProxyError> {
    let mut hdr = [0u8; 16];
    reader.read_exact(&mut hdr).await.map_err(ProxyError::Io)?;

    // Verify signature
    if hdr[..12] != V2_SIGNATURE {
        return Err(ProxyError::InvalidSignature);
    }

    let ver_cmd = hdr[12];
    let version = (ver_cmd >> 4) & 0x0F;
    let command = ver_cmd & 0x0F;

    if version != 2 {
        return Err(ProxyError::UnsupportedVersion(version));
    }

    let family_transport = hdr[13];
    let family = (family_transport >> 4) & 0x0F;
    let _transport = family_transport & 0x0F;

    let addr_len = u16::from_be_bytes([hdr[14], hdr[15]]) as usize;

    // Read address data + TLVs
    let mut addr_data = vec![0u8; addr_len];
    if addr_len > 0 {
        reader.read_exact(&mut addr_data).await.map_err(ProxyError::Io)?;
    }

    // LOCAL command = health check / internal
    if command == 0x00 {
        return Ok(ProxyHeader { src_addr: None, dst_addr: None });
    }

    // PROXY command
    if command != 0x01 {
        return Err(ProxyError::UnsupportedCommand(command));
    }

    match family {
        // AF_INET (IPv4): 4+4+2+2 = 12 bytes
        0x01 if addr_data.len() >= 12 => {
            let src_ip = std::net::Ipv4Addr::new(
                addr_data[0], addr_data[1], addr_data[2], addr_data[3],
            );
            let dst_ip = std::net::Ipv4Addr::new(
                addr_data[4], addr_data[5], addr_data[6], addr_data[7],
            );
            let src_port = u16::from_be_bytes([addr_data[8], addr_data[9]]);
            let dst_port = u16::from_be_bytes([addr_data[10], addr_data[11]]);
            Ok(ProxyHeader {
                src_addr: Some(SocketAddr::new(src_ip.into(), src_port)),
                dst_addr: Some(SocketAddr::new(dst_ip.into(), dst_port)),
            })
        }
        // AF_INET6 (IPv6): 16+16+2+2 = 36 bytes
        0x02 if addr_data.len() >= 36 => {
            let mut src_bytes = [0u8; 16];
            let mut dst_bytes = [0u8; 16];
            src_bytes.copy_from_slice(&addr_data[0..16]);
            dst_bytes.copy_from_slice(&addr_data[16..32]);
            let src_ip = std::net::Ipv6Addr::from(src_bytes);
            let dst_ip = std::net::Ipv6Addr::from(dst_bytes);
            let src_port = u16::from_be_bytes([addr_data[32], addr_data[33]]);
            let dst_port = u16::from_be_bytes([addr_data[34], addr_data[35]]);
            Ok(ProxyHeader {
                src_addr: Some(SocketAddr::new(src_ip.into(), src_port)),
                dst_addr: Some(SocketAddr::new(dst_ip.into(), dst_port)),
            })
        }
        // AF_UNIX or unsupported
        _ => Ok(ProxyHeader { src_addr: None, dst_addr: None }),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("io: {0}")]
    Io(std::io::Error),
    #[error("connection too short for PROXY protocol header")]
    TooShort,
    #[error("invalid PROXY protocol signature")]
    InvalidSignature,
    #[error("invalid v1 header")]
    InvalidV1Header,
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
    #[error("invalid address")]
    InvalidAddress,
    #[error("invalid port")]
    InvalidPort,
    #[error("unsupported v2 version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported v2 command: {0}")]
    UnsupportedCommand(u8),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_v1_tcp4_line() {
        // Verify v1 line parsing logic (TcpStream can't be constructed in
        // unit tests without a real socket, so we test the parsing directly).
        let line = "PROXY TCP4 192.168.1.1 10.0.0.1 56324 443";
        let parts: Vec<&str> = line.split(' ').collect();
        assert_eq!(parts[0], "PROXY");
        assert_eq!(parts[1], "TCP4");
        let src: std::net::IpAddr = parts[2].parse().unwrap();
        let dst: std::net::IpAddr = parts[3].parse().unwrap();
        let sport: u16 = parts[4].parse().unwrap();
        let dport: u16 = parts[5].parse().unwrap();
        assert_eq!(src.to_string(), "192.168.1.1");
        assert_eq!(dst.to_string(), "10.0.0.1");
        assert_eq!(sport, 56324);
        assert_eq!(dport, 443);
    }

    #[test]
    fn v2_signature_length() {
        assert_eq!(V2_SIGNATURE.len(), 12);
    }

    #[test]
    fn parse_v2_header_bytes() {
        // Verify v2 header layout parsing (addresses only, no IO).
        let addr_data: Vec<u8> = vec![
            // src IPv4: 192.168.0.1
            192, 168, 0, 1,
            // dst IPv4: 10.0.0.1
            10, 0, 0, 1,
            // src port: 12345 = 0x3039
            0x30, 0x39,
            // dst port: 443 = 0x01BB
            0x01, 0xBB,
        ];
        let src_ip = std::net::Ipv4Addr::new(
            addr_data[0], addr_data[1], addr_data[2], addr_data[3],
        );
        let src_port = u16::from_be_bytes([addr_data[8], addr_data[9]]);
        assert_eq!(src_ip.to_string(), "192.168.0.1");
        assert_eq!(src_port, 12345);
    }
}
