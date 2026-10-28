//! The QUIC endpoint.
//!
//! One UDP socket and a small tokio runtime on threads of its own; the
//! protocol threads never block on it. Each pair of nodes keeps one
//! connection: between validators the one with the smaller identifier dials,
//! and a gateway dials every validator it knows. Dialing retries with
//! exponential backoff (0.1 s to 2 s). Right after the TLS handshake (which
//! pins keys, see [`crate::tls`]) both sides exchange a hello naming the
//! protocol version, the chain identity and the node; a mismatch closes the
//! connection before anything is delivered.
//!
//! On a connection every class has one long-lived unidirectional stream per
//! direction, written in order and sent at the class's priority, so a 2 MB
//! certificate never delays a vote. Outbound frames wait in a queue per peer
//! and class, bounded in bytes (the oldest go first when one is full); a
//! frame whose write fails goes back to the queue for the next connection.
//! Inbound frames are checked against their class's limit before the body
//! is read, charged to the peer's quota, and handed to the delivery callback
//! with the sender the connection authenticated.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use borsh::{BorshDeserialize, BorshSerialize};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{Connection, RecvStream, SendStream, VarInt};
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use tokio::runtime::Runtime;
use tokio::sync::Notify;

use crate::class::{ClassLimits, MessageClass};
use crate::error::NetError;
use crate::frame::{frame, FrameHeader, FRAME_HEADER_BYTES};
use crate::id::NodeId;
use crate::key::NetworkKey;
use crate::quota::{Admission, Buckets, Quotas};
use crate::record::Peer;
use crate::tls::{self, Allowed, SERVER_NAME};

/// The version of the mesh protocol a hello names.
pub const PROTOCOL_VERSION: u16 = 1;

/// QUIC keep-alive interval.
pub const KEEP_ALIVE: Duration = Duration::from_secs(5);

/// A connection silent this long is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a dial or a hello may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

const BACKOFF_MIN: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(2);

/// The largest hello accepted.
const MAX_HELLO_BYTES: usize = 1024;

/// Why a connection was closed (QUIC application error codes).
mod code {
    pub const SHUTDOWN: u32 = 0;
    pub const REPLACED: u32 = 1;
    pub const PROTOCOL: u32 = 2;
    pub const QUOTA: u32 = 3;
    pub const UNKNOWN: u32 = 4;
    pub const REMOVED: u32 = 5;
}

/// What each side says first.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct Hello {
    protocol: u16,
    chain: Hash32,
    node: NodeId,
}

/// Where an endpoint hands what arrives: the sender (as its connection
/// authenticated it), the class and the body. It must not block.
pub type Deliver = Arc<dyn Fn(NodeId, MessageClass, Vec<u8>) + Send + Sync>;

/// How to run an endpoint.
#[derive(Clone, Debug)]
pub struct EndpointConfig {
    /// The chain identity both sides of a connection must share.
    pub chain: Hash32,
    /// This node.
    pub local: NodeId,
    /// Its network key.
    pub key: NetworkKey,
    /// Where to accept connections (`None`: dial only, as a gateway does).
    pub listen: Option<SocketAddr>,
    /// The nodes to keep connections with.
    pub peers: Vec<Peer>,
    /// The largest frame of each class, sent or received.
    pub limits: ClassLimits,
    /// Bytes queued per peer and class before the oldest frames go.
    pub queue_limits: ClassLimits,
    /// Inbound quotas per peer.
    pub quotas: Quotas,
    /// Threads of the endpoint's runtime.
    pub worker_threads: usize,
}

impl EndpointConfig {
    /// A configuration with default limits, queues and quotas.
    pub fn new(chain: Hash32, local: NodeId, key: NetworkKey) -> EndpointConfig {
        let limits = ClassLimits::default();
        EndpointConfig {
            chain,
            local,
            key,
            listen: None,
            peers: Vec::new(),
            limits,
            queue_limits: ClassLimits {
                control: 8 * 1024 * 1024,
                consensus: (4 * limits.consensus).max(64 * 1024 * 1024),
                ingress: (16 * limits.ingress).max(32 * 1024 * 1024),
                bulk: (4 * limits.bulk).max(64 * 1024 * 1024),
            },
            quotas: Quotas::default(),
            worker_threads: 2,
        }
    }
}

/// Counters of one peer's link.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkStats {
    /// Whether a connection is up.
    pub connected: bool,
    /// Connections established so far.
    pub connections: u64,
    /// Frames written to the wire.
    pub frames_sent: u64,
    /// Bytes written (frame headers included).
    pub bytes_sent: u64,
    /// Frames delivered.
    pub frames_received: u64,
    /// Bytes delivered (frame bodies).
    pub bytes_received: u64,
    /// Frames dropped from a full queue.
    pub queue_drops: u64,
    /// Frames dropped over the peer's quota.
    pub quota_drops: u64,
    /// Bytes waiting in the queues now.
    pub queued_bytes: u64,
}

/// Counters of an endpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointStats {
    /// The socket's address.
    pub local_addr: String,
    /// Dials that did not connect.
    pub dials_failed: u64,
    /// Incoming handshakes that failed (unknown keys among them).
    pub handshakes_failed: u64,
    /// Connections closed for a bad hello (another chain, another node).
    pub hellos_refused: u64,
    /// By peer (`validator:<hex>` or `node:<hex>`).
    pub links: BTreeMap<String, LinkStats>,
}

/// Why a frame was not queued.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The endpoint knows no such peer.
    #[error("unknown peer {0:?}")]
    UnknownPeer(NodeId),
    /// The body is larger than its class allows.
    #[error("{0}")]
    Frame(#[from] crate::frame::FrameError),
}

#[derive(Default)]
struct QueueInner {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
}

/// Frames waiting for one peer, in one class.
#[derive(Default)]
struct Queue {
    inner: Mutex<QueueInner>,
    notify: Notify,
}

impl Queue {
    /// Queues a frame; returns how many old frames made room for it.
    fn push(&self, frame: Vec<u8>, limit: usize) -> u64 {
        let mut dropped = 0;
        if let Ok(mut queue) = self.inner.lock() {
            queue.bytes += frame.len();
            queue.frames.push_back(frame);
            while queue.bytes > limit && queue.frames.len() > 1 {
                if let Some(old) = queue.frames.pop_front() {
                    queue.bytes -= old.len();
                    dropped += 1;
                }
            }
        }
        self.notify.notify_one();
        dropped
    }

    /// Puts back a frame whose write failed, first in line.
    fn push_front(&self, frame: Vec<u8>) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.bytes += frame.len();
            queue.frames.push_front(frame);
        }
        self.notify.notify_one();
    }

    fn try_pop(&self) -> Option<Vec<u8>> {
        let mut queue = self.inner.lock().ok()?;
        let frame = queue.frames.pop_front()?;
        queue.bytes -= frame.len();
        Some(frame)
    }

    async fn pop(&self) -> Vec<u8> {
        loop {
            if let Some(frame) = self.try_pop() {
                return frame;
            }
            self.notify.notified().await;
        }
    }

    fn bytes(&self) -> usize {
        self.inner.lock().map(|q| q.bytes).unwrap_or(0)
    }
}

/// Everything about one peer.
struct Slot {
    node: NodeId,
    peer: RwLock<Peer>,
    queues: [Queue; 4],
    link: Mutex<Option<(u64, Connection)>>,
    stats: Mutex<LinkStats>,
    buckets: Mutex<Buckets>,
    removed: AtomicBool,
}

impl Slot {
    fn new(peer: Peer, quotas: Quotas) -> Slot {
        Slot {
            node: peer.node,
            peer: RwLock::new(peer),
            queues: Default::default(),
            link: Mutex::new(None),
            stats: Mutex::new(LinkStats::default()),
            buckets: Mutex::new(Buckets::new(quotas, Instant::now())),
            removed: AtomicBool::new(false),
        }
    }

    fn connection(&self) -> Option<Connection> {
        self.link.lock().ok()?.as_ref().map(|(_, c)| c.clone())
    }

    fn peer(&self) -> Option<Peer> {
        self.peer.read().ok().map(|p| p.clone())
    }

    fn count(&self, f: impl FnOnce(&mut LinkStats)) {
        if let Ok(mut stats) = self.stats.lock() {
            f(&mut stats);
        }
    }

    fn snapshot(&self) -> LinkStats {
        let mut stats = self.stats.lock().map(|s| s.clone()).unwrap_or_default();
        stats.queued_bytes = self.queues.iter().map(|q| q.bytes() as u64).sum();
        stats
    }

    fn close(&self, reason: u32, why: &[u8]) {
        if let Ok(mut link) = self.link.lock() {
            if let Some((_, connection)) = link.take() {
                connection.close(VarInt::from_u32(reason), why);
            }
        }
        self.count(|s| s.connected = false);
    }
}

#[derive(Default)]
struct Counters {
    dials_failed: u64,
    handshakes_failed: u64,
    hellos_refused: u64,
}

struct Shared {
    chain: Hash32,
    local: NodeId,
    key: NetworkKey,
    limits: ClassLimits,
    queue_limits: ClassLimits,
    quotas: Quotas,
    quic: quinn::Endpoint,
    slots: RwLock<BTreeMap<NodeId, Arc<Slot>>>,
    by_key: RwLock<BTreeMap<Vec<u8>, NodeId>>,
    allowed: Allowed,
    deliver: Deliver,
    counters: Mutex<Counters>,
    next_link: AtomicU64,
    closing: AtomicBool,
    runtime: tokio::runtime::Handle,
}

/// Whether `local` dials `peer` (rather than waiting to be dialed).
pub fn dials(local: &NodeId, peer: &NodeId) -> bool {
    match (local, peer) {
        (NodeId::Validator(a), NodeId::Validator(b)) => a < b,
        (NodeId::Node(_), NodeId::Validator(_)) => true,
        _ => false,
    }
}

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(KEEP_ALIVE));
    config.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(IDLE_TIMEOUT).expect("a valid idle timeout"),
    ));
    config.max_concurrent_uni_streams(VarInt::from_u32(16));
    config.max_concurrent_bidi_streams(VarInt::from_u32(2));
    config.stream_receive_window(VarInt::from_u32(16 * 1024 * 1024));
    config.receive_window(VarInt::from_u32(64 * 1024 * 1024));
    config.send_window(64 * 1024 * 1024);
    Arc::new(config)
}

impl Shared {
    fn closing(&self) -> bool {
        self.closing.load(Ordering::Relaxed)
    }

    /// The hello this node says.
    fn greeting(&self) -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            chain: self.chain,
            node: self.local,
        }
    }

    fn count(&self, f: impl FnOnce(&mut Counters)) {
        if let Ok(mut counters) = self.counters.lock() {
            f(&mut counters);
        }
    }

    fn slot(&self, node: &NodeId) -> Option<Arc<Slot>> {
        self.slots.read().ok()?.get(node).cloned()
    }

    /// The node whose pinned key the connection's peer proved.
    fn identify(&self, connection: &Connection) -> Option<NodeId> {
        let identity = connection.peer_identity()?;
        let certificates = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
        let spki = certificates.first()?.as_ref().to_vec();
        self.by_key.read().ok()?.get(&spki).copied()
    }

    fn set_peers(self: &Arc<Self>, peers: Vec<Peer>) {
        let wanted: BTreeMap<NodeId, Peer> = peers
            .into_iter()
            .filter(|peer| peer.node != self.local)
            .map(|peer| (peer.node, peer))
            .collect();
        let Ok(mut slots) = self.slots.write() else {
            return;
        };
        for (node, slot) in slots.iter() {
            if !wanted.contains_key(node) {
                slot.removed.store(true, Ordering::Relaxed);
                slot.close(code::REMOVED, b"removed");
            }
        }
        slots.retain(|node, _| wanted.contains_key(node));
        let mut by_key = BTreeMap::new();
        let mut allowed = BTreeSet::new();
        for (node, peer) in wanted {
            let spki = peer.key.spki();
            by_key.insert(spki.clone(), node);
            allowed.insert(spki);
            match slots.get(&node) {
                Some(slot) => {
                    let rekeyed = slot.peer().map(|p| p.key != peer.key).unwrap_or(true);
                    if let Ok(mut current) = slot.peer.write() {
                        *current = peer;
                    }
                    if rekeyed {
                        // The connection authenticated the old key.
                        slot.close(code::REMOVED, b"new key");
                    }
                }
                None => {
                    let slot = Arc::new(Slot::new(peer, self.quotas));
                    slots.insert(node, slot.clone());
                    if dials(&self.local, &node) {
                        self.runtime.spawn(dial_loop(self.clone(), slot));
                    }
                }
            }
        }
        drop(slots);
        if let Ok(mut current) = self.by_key.write() {
            *current = by_key;
        }
        if let Ok(mut current) = self.allowed.write() {
            *current = allowed;
        }
    }

    /// Makes `connection` the link to `node`, replacing any older one, and
    /// starts its writers and readers.
    fn install(self: &Arc<Self>, node: NodeId, connection: Connection) {
        let Some(slot) = self.slot(&node) else {
            connection.close(VarInt::from_u32(code::REMOVED), b"removed");
            return;
        };
        let id = self.next_link.fetch_add(1, Ordering::Relaxed);
        let old = slot
            .link
            .lock()
            .ok()
            .and_then(|mut link| link.replace((id, connection.clone())));
        if let Some((_, old)) = old {
            old.close(VarInt::from_u32(code::REPLACED), b"replaced");
        }
        slot.count(|s| {
            s.connected = true;
            s.connections += 1;
        });
        for class in MessageClass::ALL {
            self.runtime
                .spawn(write_class(slot.clone(), connection.clone(), class));
        }
        self.runtime
            .spawn(read_streams(self.clone(), slot.clone(), connection.clone()));
        self.runtime.spawn(async move {
            connection.closed().await;
            if let Ok(mut link) = slot.link.lock() {
                if link.as_ref().map(|(current, _)| *current) == Some(id) {
                    *link = None;
                    slot.count(|s| s.connected = false);
                }
            }
        });
    }
}

/// Exchanges hellos on a fresh connection: `ours` goes out, and the other
/// side must speak this protocol version on the same chain and be
/// `expected`.
async fn hello(
    ours: &Hello,
    limits: &ClassLimits,
    connection: &Connection,
    expected: NodeId,
    dialer: bool,
) -> Result<(), NetError> {
    let exchange = async {
        let (mut send, mut recv) = if dialer {
            connection.open_bi().await
        } else {
            connection.accept_bi().await
        }
        .map_err(|e| NetError::Handshake(e.to_string()))?;
        let encoded = borsh::to_vec(ours).map_err(|e| NetError::Handshake(e.to_string()))?;
        send.write_all(&frame(MessageClass::Control, &encoded, limits)?)
            .await
            .map_err(|e| NetError::Handshake(e.to_string()))?;
        let _ = send.finish();
        let mut head = [0u8; FRAME_HEADER_BYTES];
        recv.read_exact(&mut head)
            .await
            .map_err(|e| NetError::Handshake(e.to_string()))?;
        let header = FrameHeader::parse(head, limits)?;
        if header.class != MessageClass::Control || header.len > MAX_HELLO_BYTES {
            return Err(NetError::Handshake("malformed hello".into()));
        }
        let mut body = vec![0u8; header.len];
        recv.read_exact(&mut body)
            .await
            .map_err(|e| NetError::Handshake(e.to_string()))?;
        let theirs = Hello::try_from_slice(&body)
            .map_err(|_| NetError::Handshake("malformed hello".into()))?;
        if theirs.protocol != PROTOCOL_VERSION {
            return Err(NetError::Handshake(format!(
                "protocol {} (this node speaks {PROTOCOL_VERSION})",
                theirs.protocol
            )));
        }
        if theirs.chain != ours.chain {
            return Err(NetError::Handshake(format!(
                "the peer is on chain {}",
                theirs.chain.short()
            )));
        }
        if theirs.node != expected {
            return Err(NetError::Handshake(format!(
                "the peer says it is {}, its key is {}'s",
                theirs.node.short(),
                expected.short()
            )));
        }
        Ok(())
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, exchange)
        .await
        .map_err(|_| NetError::Handshake("timed out".into()))?
}

async fn connect(
    shared: &Shared,
    peer: &Peer,
    address: SocketAddr,
) -> Result<Connection, NetError> {
    let tls = tls::client_config(&shared.key, &peer.key)?;
    let quic = QuicClientConfig::try_from(tls).map_err(|e| NetError::Tls(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic));
    config.transport_config(transport_config());
    let connecting = shared
        .quic
        .connect_with(config, address, SERVER_NAME)
        .map_err(|e| NetError::Handshake(e.to_string()))?;
    tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
        .await
        .map_err(|_| NetError::Handshake("timed out".into()))?
        .map_err(|e| NetError::Handshake(e.to_string()))
}

/// Keeps a connection to a peer this node dials.
async fn dial_loop(shared: Arc<Shared>, slot: Arc<Slot>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.closing() || slot.removed.load(Ordering::Relaxed) {
            return;
        }
        if let Some(connection) = slot.connection() {
            connection.closed().await;
            backoff = BACKOFF_MIN;
            continue;
        }
        let Some(peer) = slot.peer() else {
            return;
        };
        let mut connected = false;
        for address in &peer.addresses {
            match connect(&shared, &peer, *address).await {
                Ok(connection) => match hello(
                    &shared.greeting(),
                    &shared.limits,
                    &connection,
                    peer.node,
                    true,
                )
                .await
                {
                    Ok(()) => {
                        shared.install(peer.node, connection);
                        connected = true;
                        break;
                    }
                    Err(_) => {
                        connection.close(VarInt::from_u32(code::PROTOCOL), b"hello");
                        shared.count(|c| c.hellos_refused += 1);
                    }
                },
                Err(_) => shared.count(|c| c.dials_failed += 1),
            }
        }
        if !connected {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

/// Accepts connections from known keys.
async fn accept_loop(shared: Arc<Shared>) {
    while let Some(incoming) = shared.quic.accept().await {
        if shared.closing() {
            return;
        }
        let shared = shared.clone();
        shared.runtime.clone().spawn(async move {
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(_) => {
                    shared.count(|c| c.handshakes_failed += 1);
                    return;
                }
            };
            let Some(node) = shared.identify(&connection) else {
                connection.close(VarInt::from_u32(code::UNKNOWN), b"unknown key");
                shared.count(|c| c.handshakes_failed += 1);
                return;
            };
            match hello(&shared.greeting(), &shared.limits, &connection, node, false).await {
                Ok(()) => shared.install(node, connection),
                Err(_) => {
                    connection.close(VarInt::from_u32(code::PROTOCOL), b"hello");
                    shared.count(|c| c.hellos_refused += 1);
                }
            }
        });
    }
}

async fn open(connection: &Connection, class: MessageClass) -> Option<SendStream> {
    let mut stream = connection.open_uni().await.ok()?;
    let _ = stream.set_priority(class.priority());
    stream.write_all(&[class.tag()]).await.ok()?;
    Some(stream)
}

/// Writes one class's queue to its stream on `connection` until the
/// connection closes.
async fn write_class(slot: Arc<Slot>, connection: Connection, class: MessageClass) {
    let queue = &slot.queues[class.index()];
    let mut stream: Option<SendStream> = None;
    loop {
        let frame = tokio::select! {
            biased;
            _ = connection.closed() => {
                // Hand the turn to the next connection's writer.
                queue.notify.notify_one();
                return;
            }
            frame = queue.pop() => frame,
        };
        if stream.is_none() {
            stream = open(&connection, class).await;
        }
        let written = match stream.as_mut() {
            Some(stream) => stream.write_all(&frame).await.is_ok(),
            None => false,
        };
        if !written {
            queue.push_front(frame);
            return;
        }
        slot.count(|s| {
            s.frames_sent += 1;
            s.bytes_sent += frame.len() as u64;
        });
    }
}

/// Accepts the peer's streams on `connection`.
async fn read_streams(shared: Arc<Shared>, slot: Arc<Slot>, connection: Connection) {
    while let Ok(recv) = connection.accept_uni().await {
        shared.runtime.spawn(read_stream(
            shared.clone(),
            slot.clone(),
            connection.clone(),
            recv,
        ));
    }
}

/// Reads one class's frames: header, limit, body, quota, delivery.
async fn read_stream(
    shared: Arc<Shared>,
    slot: Arc<Slot>,
    connection: Connection,
    mut recv: RecvStream,
) {
    let mut tag = [0u8; 1];
    if recv.read_exact(&mut tag).await.is_err() {
        return;
    }
    let Some(class) = MessageClass::from_tag(tag[0]) else {
        connection.close(VarInt::from_u32(code::PROTOCOL), b"unknown class");
        return;
    };
    loop {
        let mut head = [0u8; FRAME_HEADER_BYTES];
        if recv.read_exact(&mut head).await.is_err() {
            return;
        }
        let header = match FrameHeader::parse(head, &shared.limits) {
            Ok(header) if header.class == class => header,
            _ => {
                connection.close(VarInt::from_u32(code::PROTOCOL), b"frame");
                return;
            }
        };
        let mut body = vec![0u8; header.len];
        if recv.read_exact(&mut body).await.is_err() {
            return;
        }
        let admission = slot
            .buckets
            .lock()
            .map(|mut b| b.admit(class, header.len, Instant::now()))
            .unwrap_or(Admission::Admit);
        match admission {
            Admission::Admit => {}
            Admission::Drop => {
                slot.count(|s| s.quota_drops += 1);
                continue;
            }
            Admission::Disconnect => {
                slot.count(|s| s.quota_drops += 1);
                connection.close(VarInt::from_u32(code::QUOTA), b"quota");
                return;
            }
        }
        slot.count(|s| {
            s.frames_received += 1;
            s.bytes_received += header.len as u64;
        });
        (shared.deliver)(slot.node, class, body);
    }
}

/// The key exchange a [`probe`] offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyExchange {
    /// X25519MLKEM768 only, as every node offers.
    PostQuantum,
    /// X25519 and P-256 only, as a peer without post-quantum TLS would:
    /// nodes refuse it.
    ClassicalOnly,
}

/// Dials `peer` once at its first address, as node `local` of chain
/// `chain` authenticating with `key` and offering `exchange`, and exchanges
/// hellos. Succeeds only if the peer accepted the connection: what an
/// operator checks a node's reachability with, and what tests check that
/// nodes refuse with. Nothing is sent after the hellos. A successful probe
/// briefly takes the place of the connection `peer` keeps to `local`, if
/// it has one.
pub fn probe(
    chain: Hash32,
    local: NodeId,
    key: &NetworkKey,
    peer: &Peer,
    exchange: KeyExchange,
) -> Result<(), NetError> {
    let provider = match exchange {
        KeyExchange::PostQuantum => tls::provider(),
        KeyExchange::ClassicalOnly => tls::provider_with(vec![
            rustls::crypto::aws_lc_rs::kx_group::X25519,
            rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
        ]),
    };
    let tls = tls::client_config_with(key, &peer.key, provider)?;
    let quic = QuicClientConfig::try_from(tls).map_err(|e| NetError::Tls(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic));
    config.transport_config(transport_config());
    let address = *peer
        .addresses
        .first()
        .ok_or_else(|| NetError::Handshake("the peer has no address".into()))?;
    let bind = if address.is_ipv6() {
        SocketAddr::from(([0u16; 8], 0))
    } else {
        SocketAddr::from(([0, 0, 0, 0], 0))
    };
    let greeting = Hello {
        protocol: PROTOCOL_VERSION,
        chain,
        node: local,
    };
    let expected = peer.node;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = quinn::Endpoint::client(bind)?;
        let connecting = client
            .connect_with(config, address, SERVER_NAME)
            .map_err(|e| NetError::Handshake(e.to_string()))?;
        let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting)
            .await
            .map_err(|_| NetError::Handshake("timed out".into()))?
            .map_err(|e| NetError::Handshake(e.to_string()))?;
        let outcome = hello(
            &greeting,
            &ClassLimits::default(),
            &connection,
            expected,
            true,
        )
        .await;
        connection.close(VarInt::from_u32(code::SHUTDOWN), b"probe");
        let _ = tokio::time::timeout(Duration::from_millis(300), client.wait_idle()).await;
        outcome
    })
}

/// A running endpoint.
pub struct Endpoint {
    shared: Arc<Shared>,
    runtime: Mutex<Option<Runtime>>,
    local_addr: SocketAddr,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("local", &self.shared.local)
            .field("address", &self.local_addr)
            .finish()
    }
}

impl Endpoint {
    /// Starts an endpoint that delivers what arrives through `deliver`.
    pub fn start(config: EndpointConfig, deliver: Deliver) -> Result<Endpoint, NetError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.worker_threads.max(1))
            .thread_name("sion-net")
            .enable_all()
            .build()?;
        let allowed: Allowed = Arc::default();
        let server = match config.listen {
            Some(_) => {
                let tls = tls::server_config(&config.key, allowed.clone())?;
                let quic =
                    QuicServerConfig::try_from(tls).map_err(|e| NetError::Tls(e.to_string()))?;
                let mut server = quinn::ServerConfig::with_crypto(Arc::new(quic));
                server.transport_config(transport_config());
                Some(server)
            }
            None => None,
        };
        let bind = config
            .listen
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
        let socket = std::net::UdpSocket::bind(bind)?;
        let quic = {
            let _context = runtime.enter();
            quinn::Endpoint::new(
                quinn::EndpointConfig::default(),
                server,
                socket,
                Arc::new(quinn::TokioRuntime),
            )?
        };
        let local_addr = quic.local_addr()?;
        let shared = Arc::new(Shared {
            chain: config.chain,
            local: config.local,
            key: config.key,
            limits: config.limits,
            queue_limits: config.queue_limits,
            quotas: config.quotas,
            quic,
            slots: RwLock::new(BTreeMap::new()),
            by_key: RwLock::new(BTreeMap::new()),
            allowed,
            deliver,
            counters: Mutex::new(Counters::default()),
            next_link: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            runtime: runtime.handle().clone(),
        });
        shared.set_peers(config.peers);
        if config.listen.is_some() {
            runtime.spawn(accept_loop(shared.clone()));
        }
        Ok(Endpoint {
            shared,
            runtime: Mutex::new(Some(runtime)),
            local_addr,
        })
    }

    /// This node.
    pub fn local(&self) -> NodeId {
        self.shared.local
    }

    /// The socket's address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Queues `body` for `to` in `class`. Returns at once; the frame goes out
    /// when a connection to `to` is up.
    pub fn send(&self, to: &NodeId, class: MessageClass, body: &[u8]) -> Result<(), SendError> {
        let slot = self.shared.slot(to).ok_or(SendError::UnknownPeer(*to))?;
        let frame = frame(class, body, &self.shared.limits)?;
        let dropped = slot.queues[class.index()].push(frame, self.shared.queue_limits.get(class));
        if dropped > 0 {
            slot.count(|s| s.queue_drops += dropped);
        }
        Ok(())
    }

    /// Whether `node` is one of this endpoint's peers.
    pub fn knows(&self, node: &NodeId) -> bool {
        self.shared.slot(node).is_some()
    }

    /// Whether a connection to `node` is up.
    pub fn connected(&self, node: &NodeId) -> bool {
        self.shared
            .slot(node)
            .is_some_and(|slot| slot.connection().is_some())
    }

    /// Replaces the peers: new ones are dialed (or awaited), removed ones
    /// disconnected, and only the remaining keys are accepted.
    pub fn set_peers(&self, peers: Vec<Peer>) {
        self.shared.set_peers(peers);
    }

    /// The endpoint's counters.
    pub fn stats(&self) -> EndpointStats {
        let counters = self
            .shared
            .counters
            .lock()
            .map(|c| (c.dials_failed, c.handshakes_failed, c.hellos_refused))
            .unwrap_or_default();
        let links = self
            .shared
            .slots
            .read()
            .map(|slots| {
                slots
                    .iter()
                    .map(|(node, slot)| (node.to_string(), slot.snapshot()))
                    .collect()
            })
            .unwrap_or_default();
        EndpointStats {
            local_addr: self.local_addr.to_string(),
            dials_failed: counters.0,
            handshakes_failed: counters.1,
            hellos_refused: counters.2,
            links,
        }
    }

    /// Closes every connection and stops the runtime.
    pub fn shutdown(&self) {
        self.shared.closing.store(true, Ordering::Relaxed);
        let Some(runtime) = self.runtime.lock().ok().and_then(|mut r| r.take()) else {
            return;
        };
        self.shared
            .quic
            .close(VarInt::from_u32(code::SHUTDOWN), b"shutdown");
        let quic = self.shared.quic.clone();
        runtime.block_on(async move {
            let _ = tokio::time::timeout(Duration::from_millis(300), quic.wait_idle()).await;
        });
        runtime.shutdown_timeout(Duration::from_millis(500));
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests;
