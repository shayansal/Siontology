//! Counting what a transport carries: messages and bytes per message class,
//! per message kind and per peer.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{MessageClass, NodeId};

/// Messages and their encoded bytes, in one direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counter {
    /// Messages.
    pub messages: u64,
    /// Bytes of their canonical (Borsh) encoding.
    pub bytes: u64,
}

impl Counter {
    fn add(&mut self, bytes: usize) {
        self.messages += 1;
        self.bytes += bytes as u64;
    }
}

/// The traffic of one class, kind or peer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traffic {
    /// Sent by this node.
    pub sent: Counter,
    /// Delivered to this node.
    pub received: Counter,
    /// Messages this node sent that its transport did not carry: over the
    /// class's size limit, for a node it cannot reach, over a full queue,
    /// or lost by a fault injector.
    pub dropped: u64,
}

/// What a transport carried for one node.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficStats {
    /// The carrier: `memory`, `memory+faults` or `quic`.
    pub transport: String,
    /// By class: `control`, `consensus`, `ingress`, `bulk`.
    pub classes: BTreeMap<String, Traffic>,
    /// By message kind (`header`, `vote`, `certificate`, …).
    pub kinds: BTreeMap<String, Traffic>,
    /// By peer (`validator:<hex>` or `node:<hex>`).
    pub peers: BTreeMap<String, Traffic>,
    /// Between machines: the QUIC endpoint's links (connections, queues,
    /// quota drops) and refused handshakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<sion_net::EndpointStats>,
}

impl TrafficStats {
    /// Everything this node sent, all classes together.
    pub fn sent(&self) -> Counter {
        let mut total = Counter::default();
        for traffic in self.classes.values() {
            total.messages += traffic.sent.messages;
            total.bytes += traffic.sent.bytes;
        }
        total
    }

    /// Everything delivered to this node, all classes together.
    pub fn received(&self) -> Counter {
        let mut total = Counter::default();
        for traffic in self.classes.values() {
            total.messages += traffic.received.messages;
            total.bytes += traffic.received.bytes;
        }
        total
    }

    /// Messages dropped, all classes together.
    pub fn dropped(&self) -> u64 {
        self.classes.values().map(|t| t.dropped).sum()
    }
}

#[derive(Debug, Default)]
struct Counts {
    classes: [Traffic; 4],
    kinds: BTreeMap<&'static str, Traffic>,
    peers: BTreeMap<NodeId, Traffic>,
}

impl Counts {
    fn each(
        &mut self,
        peer: NodeId,
        class: MessageClass,
        kind: &'static str,
        mut f: impl FnMut(&mut Traffic),
    ) {
        f(&mut self.classes[class.index()]);
        f(self.kinds.entry(kind).or_default());
        f(self.peers.entry(peer).or_default());
    }
}

/// A node's traffic counters, shared by the threads that send and deliver.
#[derive(Debug, Default)]
pub struct TrafficCounter {
    counts: Mutex<Counts>,
}

impl TrafficCounter {
    /// Counts a message sent to `to`.
    pub fn sent(&self, to: NodeId, class: MessageClass, kind: &'static str, bytes: usize) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.each(to, class, kind, |t| t.sent.add(bytes));
        }
    }

    /// Counts a message delivered from `from`.
    pub fn received(&self, from: NodeId, class: MessageClass, kind: &'static str, bytes: usize) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.each(from, class, kind, |t| t.received.add(bytes));
        }
    }

    /// Counts a message for `to` that was not carried.
    pub fn dropped(&self, to: NodeId, class: MessageClass, kind: &'static str) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.each(to, class, kind, |t| t.dropped += 1);
        }
    }

    /// The counts so far, labeled with the carrier's name.
    pub fn snapshot(&self, transport: &str) -> TrafficStats {
        let Ok(counts) = self.counts.lock() else {
            return TrafficStats::default();
        };
        TrafficStats {
            transport: transport.to_string(),
            classes: MessageClass::ALL
                .iter()
                .map(|class| (class.name().to_string(), counts.classes[class.index()]))
                .collect(),
            kinds: counts
                .kinds
                .iter()
                .map(|(kind, traffic)| (kind.to_string(), *traffic))
                .collect(),
            peers: counts
                .peers
                .iter()
                .map(|(peer, traffic)| (peer.to_string(), *traffic))
                .collect(),
            endpoint: None,
        }
    }
}
