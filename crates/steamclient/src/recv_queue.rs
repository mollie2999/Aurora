//! Shared accounting for unread networking messages across all transports.
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

const PEER_LIMIT: usize = 8 << 20;
const PROCESS_LIMIT: usize = 64 << 20;
pub const CHANNEL_LIMIT: usize = 64;
static USAGE: LazyLock<Mutex<Usage>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Usage {
    total: usize,
    peers: HashMap<u64, usize>,
}

impl Usage {
    fn reserve(&mut self, peer: u64, bytes: usize) -> bool {
        if bytes > PROCESS_LIMIT - self.total || bytes > PEER_LIMIT - self.peers.get(&peer).copied().unwrap_or(0) {
            return false;
        }
        self.total += bytes;
        *self.peers.entry(peer).or_default() += bytes;
        true
    }
}

struct Charge { peer: u64, bytes: usize }

impl Drop for Charge {
    fn drop(&mut self) {
        let mut usage = USAGE.lock().unwrap();
        usage.total -= self.bytes;
        let bytes = usage.peers.get_mut(&self.peer).unwrap();
        *bytes -= self.bytes;
        if *bytes == 0 { usage.peers.remove(&self.peer); }
    }
}

pub struct Queue<T>(VecDeque<(T, Charge)>);
impl<T> Default for Queue<T> {
    fn default() -> Self { Self(VecDeque::new()) }
}
impl<T> Queue<T> {
    pub fn push(&mut self, peer: u64, capacity: usize, value: T) -> bool {
        // Include the queue entry itself so empty datagrams are bounded too.
        let bytes = capacity.saturating_add(std::mem::size_of::<(T, Charge)>());
        if !USAGE.lock().unwrap().reserve(peer, bytes) { return false; }
        self.0.push_back((value, Charge { peer, bytes }));
        true
    }
    pub fn front(&self) -> Option<&T> { self.0.front().map(|(v, _)| v) }
    pub fn pop_front(&mut self) -> Option<T> { self.0.pop_front().map(|(v, _)| v) }
    pub fn drain<R: std::ops::RangeBounds<usize>>(&mut self, range: R) -> impl Iterator<Item=T> {
        self.0.drain(range).map(|(v, _)| v)
    }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
    pub fn len(&self) -> usize { self.0.len() }
}

#[test]
fn enforces_peer_and_process_limits_independently() {
    let mut usage = Usage::default();
    for peer in 0..PROCESS_LIMIT / PEER_LIMIT {
        assert!(usage.reserve(peer as u64, PEER_LIMIT));
        assert!(!usage.reserve(peer as u64, 1));
    }
    assert!(!usage.reserve(100, 1));
}
