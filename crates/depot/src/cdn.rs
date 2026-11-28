//! Content servers (Steam's CDN). Servers are ranked by the download speed
//! measured on real chunks, and a failing server is skipped automatically.
//!
//! [`Cdn::rank`] only gives a starting order (quickest to answer). While
//! downloading, every chunk's speed updates its server's average, the
//! fastest server takes the requests, and one in eight goes to one of the
//! next ones so a server that got faster (or the first one, slower) is noticed.

use crate::{Error, Result};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Server {
    pub host: String,
    pub https: bool,
    /// Steam's reported load (lower is better).
    pub load: f32,
}

pub struct Cdn {
    http: reqwest::Client,
    /// Servers best-first, with their average download speed in bytes/s
    /// (0 until measured); reordered as we learn which are fast.
    servers: Mutex<Vec<(Server, f64)>>,
    requests: AtomicUsize,
}

/// Servers that share the requests while being measured.
const POOL: usize = 4;
/// Maximum number of servers to probe when the initial pool's fastest server is slow.
const MAX_POOL: usize = 12;
/// Speed threshold (2 MB/s) below which we expand probing beyond [`POOL`].
const SLOW_LEADER_BPS: f64 = 2.0 * 1_048_576.0;
/// Downloads smaller than this say more about latency than speed.
const MIN_SAMPLE: usize = 64 << 10;

pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

impl Cdn {
    pub fn new(servers: Vec<Server>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("Valve/Steam HTTP Client 1.0")
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(32)
            .https_only(true)
            // Downloads spread over many connections; HTTP/2 would put them
            // all on one.
            .http1_only()
            .build()
            .expect("http client");
        let mut s: Vec<(Server, f64)> = servers
            .into_iter()
            .filter(|s| s.https && valid_host(&s.host))
            .map(|s| (s, 0.0))
            .collect();
        s.sort_by(|a, b| a.0.load.total_cmp(&b.0.load));
        Self { http, servers: Mutex::new(s), requests: AtomicUsize::new(0) }
    }

    pub fn server_count(&self) -> usize {
        self.servers.lock().unwrap().len()
    }

    /// Starting order: the servers that answer a tiny request quickest first
    /// (Steam just takes the directory's order). Download speed takes over
    /// once chunks arrive (see [`Cdn::record`]).
    pub async fn rank(&self) {
        let list: Vec<Server> = self.servers.lock().unwrap().iter().take(8).map(|s| s.0.clone()).collect();
        let mut timed: Vec<(Server, f64)> = futures_util::future::join_all(list.into_iter().map(|s| {
            let http = self.http.clone();
            async move {
                let url = format!("https://{}/", s.host);
                let start = Instant::now();
                let ok = tokio::time::timeout(Duration::from_secs(3), http.head(&url).send()).await.is_ok();
                let ms = if ok { start.elapsed().as_secs_f64() * 1000.0 } else { 10_000.0 };
                (s, ms)
            }
        }))
        .await;
        if !timed.is_empty() {
            timed.sort_by(|a, b| a.1.total_cmp(&b.1));
            tracing::info!("fastest content server: {} ({:.0} ms)", timed[0].0.host, timed[0].1);
            let mut servers = self.servers.lock().unwrap();
            let rest: Vec<(Server, f64)> = servers.drain(..).filter(|s| !timed.iter().any(|t| t.0.host == s.0.host)).collect();
            // Only an order: speeds start unmeasured.
            servers.extend(timed.into_iter().map(|(s, _)| (s, 0.0)));
            servers.extend(rest);
        }
    }

    fn pick(&self, attempt: usize) -> Result<Server> {
        let n = self.requests.fetch_add(1, Ordering::Relaxed);
        let servers = self.servers.lock().unwrap();
        if servers.is_empty() {
            return Err(Error::NoServers);
        }
        if attempt > 0 {
            return Ok(servers[attempt % servers.len()].0.clone());
        }
        let base_pool = servers.len().min(POOL);
        let pool = if servers[..base_pool].iter().all(|s| s.1 > 0.0) && servers[0].1 < SLOW_LEADER_BPS {
            servers.len().min(MAX_POOL)
        } else {
            base_pool
        };
        // Spread requests over unmeasured servers in the active pool so already-measured
        // slow servers aren't re-hit while unmeasured candidates remain.
        let unmeasured: Vec<usize> =
            servers[..pool].iter().enumerate().filter_map(|(i, s)| (s.1 == 0.0).then_some(i)).collect();
        if !unmeasured.is_empty() {
            return Ok(servers[unmeasured[n % unmeasured.len()]].0.clone());
        }
        // All servers in the active pool are measured: send 1 in 8 requests to a
        // non-leader server so speed changes are noticed.
        if pool > 1 && n % 8 == 0 {
            return Ok(servers[1 + ((n / 8) % (pool - 1))].0.clone());
        }
        Ok(servers[0].0.clone())
    }

    /// A download of `bytes` from `host` took `took`: update its average
    /// speed and keep the list fastest first.
    fn record(&self, host: &str, bytes: usize, took: Duration) {
        if bytes < MIN_SAMPLE {
            return;
        }
        let speed = bytes as f64 / took.as_secs_f64().max(0.001);
        let mut servers = self.servers.lock().unwrap();
        let Some(s) = servers.iter_mut().find(|s| s.0.host == host) else { return };
        s.1 = if s.1 == 0.0 { speed } else { s.1 * 0.8 + speed * 0.2 };
        let leader = servers[0].0.host.clone();
        // Stable: unmeasured servers keep their place after the measured ones.
        servers.sort_by(|a, b| b.1.total_cmp(&a.1));
        if servers[0].0.host != leader {
            tracing::info!("fastest content server now {} ({:.1} MB/s)", servers[0].0.host, servers[0].1 / 1_048_576.0);
        }
    }

    fn demote(&self, host: &str) {
        let mut servers = self.servers.lock().unwrap();
        if let Some(i) = servers.iter().position(|s| s.0.host == host) {
            let mut s = servers.remove(i);
            // Measured as hopeless, so speed sorting keeps it last.
            s.1 = 1.0;
            servers.push(s);
        }
    }

const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 32 * 1024 * 1024;

    async fn get(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let mut last = None;
        for attempt in 0..5 {
            let s = self.pick(attempt)?;
            let url = format!("https://{}{}", s.host, path);
            let start = Instant::now();
            match self.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
                Ok(mut r) => {
                    if r.content_length().is_some_and(|n| n > max_bytes as u64) {
                        return Err(Error::Format(format!("CDN response exceeds {} byte limit", max_bytes)));
                    }
                    let cap = r.content_length().map(|n| (n as usize).min(max_bytes)).unwrap_or(0);
                    let mut buf = Vec::with_capacity(cap);
                    let mut stream_err = None;
                    loop {
                        match r.chunk().await {
                            Ok(Some(chunk)) => {
                                if buf.len().saturating_add(chunk.len()) > max_bytes {
                                    return Err(Error::Format(format!("CDN response exceeds {} byte limit", max_bytes)));
                                }
                                buf.extend_from_slice(&chunk);
                            }
                            Ok(None) => break,
                            Err(e) => {
                                stream_err = Some(e);
                                break;
                            }
                        }
                    }
                    if let Some(e) = stream_err {
                        last = Some(e);
                    } else {
                        self.record(&s.host, buf.len(), start.elapsed());
                        return Ok(buf);
                    }
                }
                Err(e) => {
                    if e.status().map(|s| s.as_u16()) == Some(401) || e.status().map(|s| s.as_u16()) == Some(403) {
                        return Err(Error::Http(e));
                    }
                    last = Some(e);
                }
            }
            self.demote(&s.host);
            tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        Err(last.map(Error::Http).unwrap_or(Error::NoServers))
    }

    pub async fn manifest(&self, depot: u32, gid: u64, request_code: u64) -> Result<Vec<u8>> {
        let path = if request_code > 0 {
            format!("/depot/{depot}/manifest/{gid}/5/{request_code}")
        } else {
            format!("/depot/{depot}/manifest/{gid}/5")
        };
        self.get(&path, Self::MAX_MANIFEST_BYTES).await
    }

    pub async fn chunk(&self, depot: u32, sha: &[u8]) -> Result<Vec<u8>> {
        self.get(&format!("/depot/{depot}/chunk/{}", crate::chunk::hex(sha)), Self::MAX_CHUNK_BYTES).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cdn(hosts: &[&str]) -> Cdn {
        Cdn::new(hosts.iter().map(|h| Server { host: h.to_string(), https: true, load: 0.0 }).collect())
    }

    #[test]
    fn fastest_measured_server_takes_the_requests() {
        let c = cdn(&["a", "b", "c"]);
        // Until all are measured, requests spread over the pool.
        let first: Vec<String> = (0..3).map(|_| c.pick(0).unwrap().host).collect();
        assert_eq!(first, ["a", "b", "c"]);
        c.record("a", 1 << 20, Duration::from_millis(500)); // 2 MB/s
        c.record("b", 1 << 20, Duration::from_millis(50)); // 20 MB/s
        c.record("c", 1 << 20, Duration::from_millis(200)); // 5 MB/s
        let picks: Vec<String> = (0..16).map(|_| c.pick(0).unwrap().host).collect();
        assert!(picks.iter().filter(|h| *h == "b").count() >= 13, "{picks:?}");
        // Too small to say anything about speed.
        c.record("a", 1000, Duration::from_micros(1));
        assert_eq!(c.servers.lock().unwrap()[0].0.host, "b");
        // A failing server goes last even though it was fast.
        c.demote("b");
        assert_eq!(c.servers.lock().unwrap().last().unwrap().0.host, "b");
        c.record("c", 1 << 20, Duration::from_millis(200));
        assert_ne!(c.servers.lock().unwrap()[0].0.host, "b");
    }

    #[test]
    fn filters_out_plaintext_http_servers() {
        let c = Cdn::new(vec![
            Server { host: "plain.example.com".into(), https: false, load: 0.0 },
            Server { host: "tls.example.com".into(), https: true, load: 50.0 },
        ]);
        assert_eq!(c.server_count(), 1);
        assert_eq!(c.pick(0).unwrap().host, "tls.example.com");

        let only_http = Cdn::new(vec![Server { host: "plain.example.com".into(), https: false, load: 0.0 }]);
        assert_eq!(only_http.server_count(), 0);
        assert!(matches!(only_http.pick(0), Err(Error::NoServers)));
    }

    #[test]
    fn unmeasured_only_probing_skips_already_measured_server() {
        let c = cdn(&["a", "b", "c", "d"]);
        c.record("a", 1 << 20, Duration::from_secs(2)); // 0.5 MB/s
        let picks: Vec<String> = (0..6).map(|_| c.pick(0).unwrap().host).collect();
        assert!(!picks.iter().any(|h| h == "a"), "{picks:?}");
        assert_eq!(picks, ["b", "c", "d", "b", "c", "d"]);
    }

    #[test]
    fn reprobe_hits_non_leader_servers() {
        let c = cdn(&["a", "b", "c", "d"]);
        c.record("a", 10 << 20, Duration::from_secs(1));
        c.record("b", 5 << 20, Duration::from_secs(1));
        c.record("c", 4 << 20, Duration::from_secs(1));
        c.record("d", 3 << 20, Duration::from_secs(1));
        let picks: Vec<String> = (0..24).map(|_| c.pick(0).unwrap().host).collect();
        assert_eq!(picks[0], "b");
        assert_eq!(picks[8], "c");
        assert_eq!(picks[16], "d");
        assert_eq!(picks.iter().filter(|h| *h == "a").count(), 21);
    }

    #[test]
    fn slow_leader_expands_pool_beyond_initial_four() {
        let c = cdn(&["a", "b", "c", "d", "e", "f"]);
        c.record("a", 1 << 20, Duration::from_secs(1));
        c.record("b", 800 << 10, Duration::from_secs(1));
        c.record("c", 600 << 10, Duration::from_secs(1));
        c.record("d", 400 << 10, Duration::from_secs(1));
        let picks: Vec<String> = (0..4).map(|_| c.pick(0).unwrap().host).collect();
        assert_eq!(picks, ["e", "f", "e", "f"]);

        // Once a fast server is found, pool collapses back to the initial 4.
        c.record("e", 10 << 20, Duration::from_secs(1));
        let after: Vec<String> = (0..16).map(|_| c.pick(0).unwrap().host).collect();
        assert!(!after.iter().any(|h| h == "f"), "{after:?}");
        assert!(after.iter().filter(|h| *h == "e").count() >= 13, "{after:?}");
    }

    #[test]
    fn fast_leader_stays_within_initial_pool() {
        let c = cdn(&["a", "b", "c", "d", "e", "f"]);
        c.record("a", 4 << 20, Duration::from_secs(1));
        c.record("b", 1 << 20, Duration::from_secs(1));
        c.record("c", 1 << 20, Duration::from_secs(1));
        c.record("d", 1 << 20, Duration::from_secs(1));
        let picks: Vec<String> = (0..32).map(|_| c.pick(0).unwrap().host).collect();
        assert!(!picks.iter().any(|h| h == "e" || h == "f"), "{picks:?}");
        assert_eq!(picks.iter().filter(|h| *h == "a").count(), 28);
    }

    #[test]
    fn retry_attempts_reach_beyond_initial_pool() {
        let c = cdn(&["a", "b", "c", "d", "e", "f"]);
        assert_eq!(c.pick(1).unwrap().host, "b");
        assert_eq!(c.pick(4).unwrap().host, "e");
        assert_eq!(c.pick(5).unwrap().host, "f");
        assert_eq!(c.pick(6).unwrap().host, "a");
    }
}
