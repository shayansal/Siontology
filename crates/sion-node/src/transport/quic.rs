//! QUIC between processes and machines: the protocol's messages over a
//! `sion-net` endpoint.
//!
//! Each message is encoded once (Borsh) and queued for its peer in its
//! class; whatever arrives is decoded, checked to travel in its own class,
//! and delivered with the sender the connection authenticated. A message
//! for this node itself (a node that hosts a validator and a gateway talks
//! to itself) is delivered at once, without a connection.

use std::fmt;
use std::sync::Arc;

use sion_net::{Endpoint, EndpointConfig, NetError};

use super::{Delivery, NodeId, PeerMessage, PeerSet, TrafficCounter, TrafficStats, Transport};

/// A node's transport between machines.
pub struct QuicTransport {
    endpoint: Endpoint,
    traffic: Arc<TrafficCounter>,
    deliver: Delivery,
}

impl fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuicTransport({:?})", self.endpoint)
    }
}

impl QuicTransport {
    /// Starts the endpoint `config` describes; what arrives goes to
    /// `deliver`.
    pub fn start(config: EndpointConfig, deliver: Delivery) -> Result<QuicTransport, NetError> {
        let traffic = Arc::new(TrafficCounter::default());
        let counter = traffic.clone();
        let inbound = deliver.clone();
        let endpoint = Endpoint::start(
            config,
            Arc::new(move |from, class, body| {
                match borsh::from_slice::<PeerMessage>(&body) {
                    Ok(message) if message.class() == class => {
                        counter.received(from, class, message.kind(), body.len());
                        inbound(from, message);
                    }
                    // Undecodable, or smuggled into another class's stream.
                    _ => counter.dropped(from, class, "malformed"),
                }
            }),
        )?;
        Ok(QuicTransport {
            endpoint,
            traffic,
            deliver,
        })
    }

    /// The endpoint (its address, links and counters).
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn carry(&self, to: NodeId, message: &PeerMessage, bytes: &[u8]) {
        let (class, kind) = (message.class(), message.kind());
        if to == self.local() {
            self.traffic.sent(to, class, kind, bytes.len());
            self.traffic.received(to, class, kind, bytes.len());
            (self.deliver)(to, message.clone());
            return;
        }
        match self.endpoint.send(&to, class, bytes) {
            Ok(()) => self.traffic.sent(to, class, kind, bytes.len()),
            Err(_) => self.traffic.dropped(to, class, kind),
        }
    }
}

impl Transport for QuicTransport {
    fn local(&self) -> NodeId {
        self.endpoint.local()
    }

    fn send(&self, to: NodeId, message: PeerMessage) {
        let bytes = borsh::to_vec(&message).expect("values in memory encode");
        self.carry(to, &message, &bytes);
    }

    fn send_all(&self, to: &[NodeId], message: &PeerMessage) {
        let bytes = borsh::to_vec(message).expect("values in memory encode");
        for node in to {
            self.carry(*node, message, &bytes);
        }
    }

    fn knows(&self, node: &NodeId) -> bool {
        *node == self.local() || self.endpoint.knows(node)
    }

    fn set_peers(&self, peers: &PeerSet) {
        self.endpoint.set_peers(peers.peers.clone());
    }

    fn stats(&self) -> TrafficStats {
        let mut stats = self.traffic.snapshot("quic");
        stats.endpoint = Some(self.endpoint.stats());
        stats
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use sion_core::hash::hash_domain;
    use sion_core::ids::{Pulse, ValidatorId};
    use sion_net::{NetworkKey, Peer};

    use super::*;

    type Inbox = Arc<Mutex<Vec<(NodeId, PeerMessage)>>>;

    fn start(name: &str, seed: u8) -> (QuicTransport, Inbox, NetworkKey) {
        let key = NetworkKey::from_seed(&[seed; 32]);
        let inbox: Inbox = Arc::default();
        let sink = inbox.clone();
        let mut config = EndpointConfig::new(
            hash_domain("sion/test/chain", b"quic"),
            NodeId::Validator(ValidatorId::derive(name.as_bytes())),
            key.clone(),
        );
        config.listen = Some("127.0.0.1:0".parse().unwrap());
        config.worker_threads = 1;
        let transport = QuicTransport::start(
            config,
            Arc::new(move |from, message| sink.lock().unwrap().push((from, message))),
        )
        .unwrap();
        (transport, inbox, key)
    }

    fn peer(transport: &QuicTransport, key: &NetworkKey) -> Peer {
        Peer {
            node: transport.local(),
            key: key.public().clone(),
            addresses: vec![transport.endpoint().local_addr()],
        }
    }

    #[test]
    fn peer_messages_cross_quic_stamped_with_their_sender() {
        let (a, a_inbox, a_key) = start("a", 1);
        let (b, b_inbox, b_key) = start("b", 2);
        a.set_peers(&PeerSet {
            peers: vec![peer(&b, &b_key)],
        });
        b.set_peers(&PeerSet {
            peers: vec![peer(&a, &a_key)],
        });
        let request = PeerMessage::PulseSignatureRequest { pulse: Pulse(7) };
        a.send(b.local(), request.clone());
        // To itself: delivered at once.
        a.send(a.local(), request.clone());
        assert_eq!(*a_inbox.lock().unwrap(), vec![(a.local(), request.clone())]);
        let deadline = Instant::now() + Duration::from_secs(30);
        while b_inbox.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the message never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(*b_inbox.lock().unwrap(), vec![(a.local(), request.clone())]);
        let stats = b.stats();
        assert_eq!(stats.transport, "quic");
        assert_eq!(
            stats.kinds["pulse_signature_request"].received.bytes,
            request.encoded_len() as u64
        );
        assert!(stats.endpoint.unwrap().links[&a.local().to_string()].connected);
        assert!(a.knows(&a.local()) && a.knows(&b.local()));
    }
}
