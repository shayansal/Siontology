//! The in-process carrier: what the devnet's channels did, behind the
//! transport seam.
//!
//! Every endpoint registers a [`Delivery`] (its inbox) with a shared
//! [`MemoryNetwork`]. Sending calls the receiver's delivery on the sender's
//! thread, so messages between two nodes arrive in the order they were sent,
//! exactly once, as they did over the channels. The network stamps each
//! message with the sending endpoint's node: a node cannot send as another.
//!
//! Messages larger than their class allows are dropped and counted, as a
//! network peer would refuse them. With `encode` set, every message is
//! encoded and decoded on its way, so a devnet run checks that what the
//! protocol sends has one canonical encoding that fits its class.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use super::{ClassLimits, Delivery, NodeId, PeerMessage, TrafficCounter, TrafficStats, Transport};

#[derive(Clone)]
struct Registered {
    deliver: Delivery,
    traffic: Arc<TrafficCounter>,
}

/// The nodes of one in-process network.
pub struct MemoryNetwork {
    endpoints: RwLock<BTreeMap<NodeId, Registered>>,
    limits: ClassLimits,
    encode: bool,
}

impl fmt::Debug for MemoryNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes = self.endpoints.read().map(|e| e.len()).unwrap_or(0);
        f.debug_struct("MemoryNetwork")
            .field("nodes", &nodes)
            .field("encode", &self.encode)
            .finish()
    }
}

impl MemoryNetwork {
    /// A network whose messages must fit `limits`. With `encode`, every
    /// message is encoded and decoded on its way (slower; it checks that
    /// the encoding round-trips).
    pub fn new(limits: ClassLimits, encode: bool) -> Arc<MemoryNetwork> {
        Arc::new(MemoryNetwork {
            endpoints: RwLock::new(BTreeMap::new()),
            limits,
            encode,
        })
    }

    /// Adds `node`, which receives through `deliver`, and returns its
    /// transport. Registering a node again replaces its delivery.
    pub fn endpoint(self: &Arc<Self>, node: NodeId, deliver: Delivery) -> MemoryTransport {
        let traffic = Arc::new(TrafficCounter::default());
        if let Ok(mut endpoints) = self.endpoints.write() {
            endpoints.insert(
                node,
                Registered {
                    deliver,
                    traffic: traffic.clone(),
                },
            );
        }
        MemoryTransport {
            local: node,
            network: self.clone(),
            traffic,
        }
    }

    fn carry(
        &self,
        from: NodeId,
        to: NodeId,
        message: PeerMessage,
        size: usize,
        sender: &TrafficCounter,
    ) {
        let (class, kind) = (message.class(), message.kind());
        let limit = self.limits.get(class);
        if size > limit {
            eprintln!(
                "transport: dropping a {kind} of {size} bytes for {}, over the {} limit of {limit}",
                to.short(),
                class.name()
            );
            sender.dropped(to, class, kind);
            return;
        }
        let target = self
            .endpoints
            .read()
            .ok()
            .and_then(|endpoints| endpoints.get(&to).cloned());
        let Some(target) = target else {
            sender.dropped(to, class, kind);
            return;
        };
        let message = if self.encode {
            let bytes = borsh::to_vec(&message).expect("values in memory encode");
            match borsh::from_slice::<PeerMessage>(&bytes) {
                Ok(decoded) if bytes.len() == size => decoded,
                _ => {
                    eprintln!("transport: a {kind} does not round-trip through its encoding");
                    sender.dropped(to, class, kind);
                    return;
                }
            }
        } else {
            message
        };
        sender.sent(to, class, kind, size);
        target.traffic.received(from, class, kind, size);
        (target.deliver)(from, message);
    }
}

/// One node's endpoint on a [`MemoryNetwork`].
#[derive(Clone)]
pub struct MemoryTransport {
    local: NodeId,
    network: Arc<MemoryNetwork>,
    traffic: Arc<TrafficCounter>,
}

impl fmt::Debug for MemoryTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemoryTransport({})", self.local.short())
    }
}

impl Transport for MemoryTransport {
    fn local(&self) -> NodeId {
        self.local
    }

    fn send(&self, to: NodeId, message: PeerMessage) {
        let size = message.encoded_len();
        self.network
            .carry(self.local, to, message, size, &self.traffic);
    }

    fn send_all(&self, to: &[NodeId], message: &PeerMessage) {
        let size = message.encoded_len();
        for node in to {
            self.network
                .carry(self.local, *node, message.clone(), size, &self.traffic);
        }
    }

    fn knows(&self, node: &NodeId) -> bool {
        self.network
            .endpoints
            .read()
            .map(|endpoints| endpoints.contains_key(node))
            .unwrap_or(false)
    }

    fn stats(&self) -> TrafficStats {
        self.traffic.snapshot("memory")
    }
}
