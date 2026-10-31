//! UDP IP discovery: learn our public address/port as seen by the voice server.
//!
//! Request and response are 74 bytes: `type u16 | length u16 (=70) | ssrc u32 |
//! address [u8; 64] (NUL-terminated) | port u16`, type 1 = request, 2 = response.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

pub const DISCOVERY_LEN: usize = 74;

pub fn discovery_request(ssrc: u32) -> [u8; DISCOVERY_LEN] {
    let mut b = [0u8; DISCOVERY_LEN];
    b[0..2].copy_from_slice(&1u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    b
}

pub fn discovery_response(ssrc: u32, address: &str, port: u16) -> [u8; DISCOVERY_LEN] {
    let mut b = [0u8; DISCOVERY_LEN];
    b[0..2].copy_from_slice(&2u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    let a = address.as_bytes();
    b[8..8 + a.len().min(63)].copy_from_slice(&a[..a.len().min(63)]);
    b[72..74].copy_from_slice(&port.to_be_bytes());
    b
}

pub fn parse_discovery_response(b: &[u8]) -> Option<(String, u16)> {
    if b.len() < DISCOVERY_LEN || u16::from_be_bytes([b[0], b[1]]) != 2 {
        return None;
    }
    let addr = &b[8..72];
    let end = addr.iter().position(|c| *c == 0).unwrap_or(addr.len());
    let address = std::str::from_utf8(&addr[..end]).ok()?.to_string();
    Some((address, u16::from_be_bytes([b[72], b[73]])))
}

/// Binds a socket, connects it to the media server and performs IP discovery.
pub async fn connect_and_discover(server: SocketAddr, ssrc: u32) -> std::io::Result<(UdpSocket, String, u16)> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().expect("static") } else { "[::]:0".parse().expect("static") };
    let sock = UdpSocket::bind(bind).await?;
    sock.connect(server).await?;
    let req = discovery_request(ssrc);
    let mut buf = [0u8; 1500];
    for _ in 0..5 {
        sock.send(&req).await?;
        match tokio::time::timeout(Duration::from_millis(800), sock.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                if let Some((ip, port)) = parse_discovery_response(&buf[..n]) {
                    return Ok((sock, ip, port));
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => continue,
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "IP discovery timed out"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_roundtrip() {
        let r = discovery_response(5, "203.0.113.7", 50000);
        assert_eq!(parse_discovery_response(&r), Some(("203.0.113.7".into(), 50000)));
        assert!(parse_discovery_response(&discovery_request(5)).is_none());
    }

    #[tokio::test]
    async fn discovery_against_local_echo() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 100];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            assert_eq!(n, DISCOVERY_LEN);
            let ssrc = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
            server.send_to(&discovery_response(ssrc, &from.ip().to_string(), from.port()), from).await.unwrap();
        });
        let (_sock, ip, port) = connect_and_discover(addr, 77).await.unwrap();
        assert_eq!(ip, "127.0.0.1");
        assert!(port > 0);
    }
}
