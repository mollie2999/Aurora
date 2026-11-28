//! "View server info": ask a game server about itself with Valve's server
//! query protocol (A2S_INFO over UDP), like Steam's server info window.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub name: String,
    pub map: String,
    pub game: String,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    pub vac: bool,
    pub password: bool,
    pub ping_ms: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerPlayer {
    pub name: String,
    pub score: i32,
    pub duration_secs: f32,
}

const INFO: &[u8] = b"\xFF\xFF\xFF\xFFTSource Engine Query\0";
const PLAYERS_REQ: &[u8] = b"\xFF\xFF\xFF\xFFU";
const RULES_REQ: &[u8] = b"\xFF\xFF\xFF\xFFV";

/// Query `addr` ("ip:port"). Blocking: call it off the UI thread.
pub fn query(addr: &str) -> Result<ServerInfo, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket.set_read_timeout(Some(Duration::from_secs(3))).map_err(|e| e.to_string())?;
    socket.connect(addr).map_err(|e| e.to_string())?;
    let start = Instant::now();
    socket.send(INFO).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 1400];
    let mut n = socket.recv(&mut buf).map_err(|_| "The server didn't answer (it may block queries).".to_string())?;
    // Newer servers answer with a challenge first; repeat with it appended.
    if n >= 9 && buf[4] == b'A' {
        let mut req = INFO.to_vec();
        req.extend_from_slice(&buf[5..9]);
        socket.send(&req).map_err(|e| e.to_string())?;
        n = socket.recv(&mut buf).map_err(|_| "The server didn't answer (it may block queries).".to_string())?;
    }
    let ping_ms = start.elapsed().as_millis() as u32;
    let mut info = parse(&buf[..n])?;
    info.ping_ms = ping_ms;
    Ok(info)
}

/// Parse an A2S_INFO reply (header 0xFFFFFFFF 'I').
pub fn parse(b: &[u8]) -> Result<ServerInfo, String> {
    if b.len() < 6 || b[..4] != [0xFF; 4] || b[4] != b'I' {
        return Err("Unexpected answer from the server".into());
    }
    let mut i = 6; // header + protocol byte
    let mut string = || -> String {
        let end = b[i..].iter().position(|c| *c == 0).map(|p| i + p).unwrap_or(b.len());
        let s = String::from_utf8_lossy(&b[i..end]).into_owned();
        i = (end + 1).min(b.len());
        s
    };
    let name = string();
    let map = string();
    let _folder = string();
    let game = string();
    let byte = |at: usize| b.get(at).copied().unwrap_or(0);
    // id (2 bytes), players, max, bots, type, os, visibility, vac
    let at = i + 2;
    Ok(ServerInfo { name, map, game, players: byte(at), max_players: byte(at + 1), bots: byte(at + 2), password: byte(at + 5) == 1, vac: byte(at + 6) == 1, ping_ms: 0 })
}

fn cstr(d: &[u8], at: &mut usize) -> String {
    let start = (*at).min(d.len());
    let end = d[start..]
        .iter()
        .position(|b| *b == 0)
        .map(|p| start + p)
        .unwrap_or(d.len());
    *at = (end + 1).min(d.len());
    String::from_utf8_lossy(&d[start..end]).into_owned()
}

fn recv_a2s_packet(socket: &UdpSocket) -> Result<Vec<u8>, String> {
    let mut buf = [0u8; 4096];
    let n = socket
        .recv(&mut buf)
        .map_err(|_| "The server didn't answer (it may block queries).".to_string())?;
    let pkt = &buf[..n];
    // Handle multi-packet uncompressed (-2) responses if present.
    if pkt.len() >= 12 && pkt[..4] == [0xFE, 0xFF, 0xFF, 0xFF] {
        let req_id = [pkt[4], pkt[5], pkt[6], pkt[7]];
        let total = pkt[8];
        let number = pkt[9];
        if total == 0 || total > 32 {
            return Err("Invalid split A2S packet count".into());
        }
        let mut parts = std::collections::BTreeMap::new();
        parts.insert(number, pkt[12..].to_vec());
        while parts.len() < total as usize {
            let m = socket
                .recv(&mut buf)
                .map_err(|_| "Incomplete multi-packet A2S reply.".to_string())?;
            let p = &buf[..m];
            if p.len() >= 12 && p[..4] == [0xFE, 0xFF, 0xFF, 0xFF] && p[4..8] == req_id {
                parts.insert(p[9], p[12..].to_vec());
            } else {
                break;
            }
        }
        let whole: Vec<u8> = (0..total)
            .flat_map(|idx| parts.remove(&idx).unwrap_or_default())
            .collect();
        return Ok(whole);
    }
    Ok(pkt.to_vec())
}

fn query_challenged(addr: &str, prefix: &[u8]) -> Result<Vec<u8>, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    socket.connect(addr).map_err(|e| e.to_string())?;
    let mut initial = Vec::with_capacity(prefix.len() + 4);
    initial.extend_from_slice(prefix);
    initial.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    socket.send(&initial).map_err(|e| e.to_string())?;
    let mut reply = recv_a2s_packet(&socket)?;
    if reply.len() >= 9 && reply[..4] == [0xFF; 4] && reply[4] == b'A' {
        let mut retry = Vec::with_capacity(prefix.len() + 4);
        retry.extend_from_slice(prefix);
        retry.extend_from_slice(&reply[5..9]);
        socket.send(&retry).map_err(|e| e.to_string())?;
        reply = recv_a2s_packet(&socket)?;
    }
    Ok(reply)
}

/// Query A2S_PLAYER list for `addr` (`"ip:port"`). Blocking: call off UI thread.
pub fn query_players(addr: &str) -> Result<Vec<ServerPlayer>, String> {
    let raw = query_challenged(addr, PLAYERS_REQ)?;
    parse_players(&raw)
}

/// Parse an A2S_PLAYER reply (header `0xFFFFFFFF 'D'`).
pub fn parse_players(b: &[u8]) -> Result<Vec<ServerPlayer>, String> {
    if b.len() < 6 || b[..4] != [0xFF; 4] || b[4] != b'D' {
        return Err("Unexpected A2S_PLAYER reply".into());
    }
    let count = b[5] as usize;
    let mut at = 6;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if at >= b.len() {
            break;
        }
        at += 1; // index byte
        let name = cstr(b, &mut at);
        let Some(tail) = b.get(at..at + 8) else { break };
        let score = i32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        let duration_secs = f32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]);
        at += 8;
        out.push(ServerPlayer {
            name,
            score,
            duration_secs: if duration_secs.is_finite() { duration_secs.max(0.0) } else { 0.0 },
        });
    }
    Ok(out)
}

/// Query A2S_RULES key/value pairs for `addr` (`"ip:port"`). Blocking.
pub fn query_rules(addr: &str) -> Result<Vec<(String, String)>, String> {
    let raw = query_challenged(addr, RULES_REQ)?;
    parse_rules(&raw)
}

/// Parse an A2S_RULES reply (header `0xFFFFFFFF 'E'`).
pub fn parse_rules(b: &[u8]) -> Result<Vec<(String, String)>, String> {
    if b.len() < 7 || b[..4] != [0xFF; 4] || b[4] != b'E' {
        return Err("Unexpected A2S_RULES reply".into());
    }
    let count = u16::from_le_bytes([b[5], b[6]]) as usize;
    let mut at = 7;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count.min(1024) {
        if at >= b.len() {
            break;
        }
        let k = cstr(b, &mut at);
        let v = cstr(b, &mut at);
        out.push((k, v));
    }
    Ok(out)
}

/// Discover Source servers on the LAN via UDP broadcast on standard ports.
pub fn discover_lan(timeout: Duration) -> Vec<(String, ServerInfo)> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = socket.set_broadcast(true);
    let _ = socket.set_read_timeout(Some(Duration::from_millis(80)));
    for port in (27015u16..=27020).chain(26900u16..=26905) {
        let _ = socket.send_to(INFO, ("255.255.255.255", port));
    }
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut buf = [0u8; 1400];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                if n >= 9 && buf[..4] == [0xFF; 4] && buf[4] == b'A' {
                    let mut req = INFO.to_vec();
                    req.extend_from_slice(&buf[5..9]);
                    let _ = socket.send_to(&req, src);
                    continue;
                }
                if let Ok(mut info) = parse(&buf[..n]) {
                    let addr = src.to_string();
                    if seen.insert(addr.clone()) {
                        info.ping_ms = 1;
                        found.push((addr, info));
                    }
                }
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_a2s_info() {
        let mut b = vec![0xFF, 0xFF, 0xFF, 0xFF, b'I', 17];
        for s in ["My Server", "rp_bangclaw", "garrysmod", "DarkRP"] {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b.extend_from_slice(&[0x04, 0x00, 37, 64, 2, b'd', b'w', 0, 1]);
        let i = super::parse(&b).unwrap();
        assert_eq!((i.name.as_str(), i.map.as_str(), i.game.as_str()), ("My Server", "rp_bangclaw", "DarkRP"));
        assert_eq!((i.players, i.max_players, i.bots, i.password, i.vac), (37, 64, 2, false, true));
    }

    #[test]
    fn parses_a2s_players_and_rules() {
        let mut p = vec![0xFF, 0xFF, 0xFF, 0xFF, b'D', 2, 0];
        p.extend_from_slice(b"Alice\0");
        p.extend_from_slice(&12i32.to_le_bytes());
        p.extend_from_slice(&45.5f32.to_le_bytes());
        p.push(1);
        p.extend_from_slice(b"Bob\0");
        p.extend_from_slice(&(-3i32).to_le_bytes());
        p.extend_from_slice(&12.0f32.to_le_bytes());

        let players = super::parse_players(&p).unwrap();
        assert_eq!(players.len(), 2);
        assert_eq!(players[0].name, "Alice");
        assert_eq!(players[0].score, 12);
        assert_eq!(players[1].name, "Bob");
        assert_eq!(players[1].score, -3);

        let mut r = vec![0xFF, 0xFF, 0xFF, 0xFF, b'E', 2, 0];
        r.extend_from_slice(b"sv_gravity\0800\0mp_timelimit\030\0");
        let rules = super::parse_rules(&r).unwrap();
        assert_eq!(
            rules,
            vec![
                ("sv_gravity".to_string(), "800".to_string()),
                ("mp_timelimit".to_string(), "30".to_string()),
            ]
        );
    }
}
