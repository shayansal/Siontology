//! The endpoint over real QUIC on the loopback interface.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sion_core::hash::hash_domain;
use sion_core::ids::ValidatorId;

use super::*;
use crate::key::NetworkKey;

type Received = Arc<Mutex<Vec<(NodeId, MessageClass, Vec<u8>)>>>;

fn chain(name: &str) -> Hash32 {
    hash_domain("sion/test/chain", name.as_bytes())
}

struct Node {
    endpoint: Endpoint,
    received: Received,
    key: NetworkKey,
}

impl Node {
    fn id(&self) -> NodeId {
        self.endpoint.local()
    }

    fn peer(&self) -> Peer {
        Peer {
            node: self.id(),
            key: self.key.public().clone(),
            addresses: vec![self.endpoint.local_addr()],
        }
    }

    fn received(&self) -> Vec<(NodeId, MessageClass, Vec<u8>)> {
        self.received.lock().unwrap().clone()
    }
}

/// A node named `id` with key seed `seed` on chain `chain`, listening on
/// `listen` (a validator) or dialing only (a gateway).
fn start(
    id: NodeId,
    seed: u8,
    chain_name: &str,
    listen: Option<&str>,
    tweak: impl FnOnce(&mut EndpointConfig),
) -> Node {
    let key = NetworkKey::from_seed(&[seed; 32]);
    let received: Received = Arc::default();
    let sink = received.clone();
    let mut config = EndpointConfig::new(chain(chain_name), id, key.clone());
    config.listen = listen.map(|a| a.parse().unwrap());
    config.worker_threads = 1;
    tweak(&mut config);
    let endpoint = Endpoint::start(
        config,
        Arc::new(move |from, class, body| sink.lock().unwrap().push((from, class, body))),
    )
    .unwrap();
    Node {
        endpoint,
        received,
        key,
    }
}

fn validator(name: &str) -> NodeId {
    NodeId::Validator(ValidatorId::derive(name.as_bytes()))
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Two validators that know each other, connected.
fn pair() -> (Node, Node) {
    let a = start(validator("a"), 1, "test", Some("127.0.0.1:0"), |_| {});
    let b = start(validator("b"), 2, "test", Some("127.0.0.1:0"), |_| {});
    a.endpoint.set_peers(vec![b.peer()]);
    b.endpoint.set_peers(vec![a.peer()]);
    wait_until("the validators connect", || {
        a.endpoint.connected(&b.id()) && b.endpoint.connected(&a.id())
    });
    (a, b)
}

#[test]
fn pinned_validators_connect_and_carry_every_class_in_order() {
    let (a, b) = pair();
    // Exactly one of them dialed.
    assert_ne!(dials(&a.id(), &b.id()), dials(&b.id(), &a.id()));
    let big = vec![7u8; 2 * 1024 * 1024];
    for class in MessageClass::ALL {
        for i in 0..3u8 {
            a.endpoint.send(&b.id(), class, &[class.tag(), i]).unwrap();
        }
    }
    a.endpoint
        .send(&b.id(), MessageClass::Consensus, &big)
        .unwrap();
    b.endpoint
        .send(&a.id(), MessageClass::Control, b"answer")
        .unwrap();
    wait_until("everything arrives", || {
        b.received().len() == 13 && a.received().len() == 1
    });
    let got = b.received();
    assert!(got.iter().all(|(from, _, _)| *from == a.id()));
    for class in MessageClass::ALL {
        let bodies: Vec<Vec<u8>> = got
            .iter()
            .filter(|(_, c, body)| *c == class && body.len() == 2)
            .map(|(_, _, body)| body.clone())
            .collect();
        let expected: Vec<Vec<u8>> = (0..3u8).map(|i| vec![class.tag(), i]).collect();
        assert_eq!(bodies, expected, "{class:?} arrives in order");
    }
    assert!(got
        .iter()
        .any(|(_, c, body)| *c == MessageClass::Consensus && *body == big));
    assert_eq!(
        a.received(),
        vec![(b.id(), MessageClass::Control, b"answer".to_vec())]
    );
    let stats = b.endpoint.stats();
    let link = &stats.links[&a.id().to_string()];
    assert!(link.connected);
    assert_eq!(link.frames_received, 13);
    assert_eq!(link.connections, 1);
    assert_eq!(stats.handshakes_failed, 0);
    assert_eq!(
        a.endpoint.stats().links[&b.id().to_string()].frames_sent,
        13
    );
}

#[test]
fn an_unknown_key_is_refused() {
    let (a, b) = pair();
    // A gateway whose key nobody pinned dials b, pinning b's real key.
    let stranger = start(
        NetworkKey::from_seed(&[9; 32]).public().node_id(),
        9,
        "test",
        None,
        |_| {},
    );
    stranger.endpoint.set_peers(vec![b.peer()]);
    stranger
        .endpoint
        .send(&b.id(), MessageClass::Ingress, b"let me in")
        .unwrap();
    wait_until("b refuses the handshake", || {
        b.endpoint.stats().handshakes_failed > 0
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!b.endpoint.knows(&stranger.id()));
    assert!(!stranger.endpoint.connected(&b.id()));
    assert!(b.received().is_empty(), "nothing from the stranger");
    drop(a);
}

#[test]
fn a_node_presenting_another_key_is_refused() {
    let (a, b) = pair();
    // A gateway that a and b both know, dialing b's address but pinning
    // a's key: whoever answers there cannot prove it.
    let gateway_key = NetworkKey::from_seed(&[5; 32]);
    let gateway_id = gateway_key.public().node_id();
    let gateway = start(gateway_id, 5, "test", None, |_| {});
    let gateway_peer = Peer {
        node: gateway_id,
        key: gateway_key.public().clone(),
        addresses: Vec::new(),
    };
    b.endpoint.set_peers(vec![a.peer(), gateway_peer]);
    gateway.endpoint.set_peers(vec![Peer {
        node: a.id(),
        key: a.key.public().clone(),
        addresses: vec![b.endpoint.local_addr()],
    }]);
    wait_until("the gateway's dial fails", || {
        gateway.endpoint.stats().dials_failed > 0
    });
    assert!(!gateway.endpoint.connected(&a.id()));
    assert!(b.received().is_empty());
}

#[test]
fn a_peer_offering_only_classical_key_exchange_cannot_connect() {
    let (a, b) = pair();
    // a's own key, so only the key exchange stands in the way.
    let classical = tls::provider_with(vec![
        rustls::crypto::aws_lc_rs::kx_group::X25519,
        rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
    ]);
    let tls = tls::client_config_with(&a.key, b.key.public(), classical).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let address = b.endpoint.local_addr();
    let outcome = runtime.block_on(async move {
        let client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let quic = QuicClientConfig::try_from(tls).unwrap();
        let connecting = client
            .connect_with(
                quinn::ClientConfig::new(Arc::new(quic)),
                address,
                SERVER_NAME,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), connecting).await
    });
    assert!(
        matches!(outcome, Ok(Err(_))),
        "the handshake must fail: {outcome:?}"
    );
    wait_until("b counts the failure", || {
        b.endpoint.stats().handshakes_failed > 0
    });
    // The real link is unaffected.
    assert!(b.endpoint.connected(&a.id()));
}

#[test]
fn a_peer_on_another_chain_is_refused_at_the_hello() {
    let a = start(validator("a"), 1, "one chain", Some("127.0.0.1:0"), |_| {});
    let b = start(
        validator("b"),
        2,
        "another chain",
        Some("127.0.0.1:0"),
        |_| {},
    );
    a.endpoint.set_peers(vec![b.peer()]);
    b.endpoint.set_peers(vec![a.peer()]);
    a.endpoint
        .send(&b.id(), MessageClass::Control, b"hi")
        .unwrap();
    wait_until("a hello is refused", || {
        a.endpoint.stats().hellos_refused + b.endpoint.stats().hellos_refused > 0
    });
    assert!(!a.endpoint.connected(&b.id()) && !b.endpoint.connected(&a.id()));
    assert!(b.received().is_empty());
}

#[test]
fn a_gateway_dials_and_gets_answers_on_its_own_connection() {
    let v = start(validator("v"), 1, "test", Some("127.0.0.1:0"), |_| {});
    let key = NetworkKey::from_seed(&[3; 32]);
    let g = start(key.public().node_id(), 3, "test", None, |_| {});
    v.endpoint.set_peers(vec![Peer {
        addresses: Vec::new(),
        ..g.peer()
    }]);
    g.endpoint.set_peers(vec![v.peer()]);
    g.endpoint
        .send(&v.id(), MessageClass::Ingress, b"lane request")
        .unwrap();
    wait_until("the request arrives", || v.received().len() == 1);
    assert_eq!(v.received()[0].0, g.id());
    v.endpoint
        .send(&g.id(), MessageClass::Control, b"approval")
        .unwrap();
    wait_until("the answer arrives", || g.received().len() == 1);
    assert_eq!(
        g.received(),
        vec![(v.id(), MessageClass::Control, b"approval".to_vec())]
    );
    assert!(!dials(&v.id(), &g.id()) && dials(&g.id(), &v.id()));
}

#[test]
fn a_probe_gets_in_only_with_a_pinned_key_and_post_quantum_key_exchange() {
    let v = start(validator("v"), 1, "test", Some("127.0.0.1:0"), |_| {});
    let key = NetworkKey::from_seed(&[3; 32]);
    let id = key.public().node_id();
    v.endpoint.set_peers(vec![Peer {
        node: id,
        key: key.public().clone(),
        addresses: Vec::new(),
    }]);
    let target = v.peer();
    probe(chain("test"), id, &key, &target, KeyExchange::PostQuantum).unwrap();
    assert!(probe(chain("test"), id, &key, &target, KeyExchange::ClassicalOnly).is_err());
    let stranger = NetworkKey::from_seed(&[4; 32]);
    assert!(probe(
        chain("test"),
        stranger.public().node_id(),
        &stranger,
        &target,
        KeyExchange::PostQuantum
    )
    .is_err());
    assert!(probe(chain("other"), id, &key, &target, KeyExchange::PostQuantum).is_err());
    // A probe expecting another node at v's address fails at the hello.
    let elsewhere = Peer {
        node: validator("w"),
        ..target.clone()
    };
    assert!(probe(
        chain("test"),
        id,
        &key,
        &elsewhere,
        KeyExchange::PostQuantum
    )
    .is_err());
    let stats = v.endpoint.stats();
    assert!(stats.handshakes_failed >= 2, "{stats:?}");
    assert!(stats.links[&id.to_string()].connections >= 1);
}

#[test]
fn frames_queued_while_a_peer_is_down_arrive_when_it_returns() {
    let (a, b) = pair();
    let address = b.endpoint.local_addr().to_string();
    a.endpoint
        .send(&b.id(), MessageClass::Control, b"one")
        .unwrap();
    wait_until("the first frame arrives", || b.received().len() == 1);
    // Dropping the endpoint closes its connections and its socket.
    drop(b);
    wait_until("a sees b go", || !a.endpoint.connected(&validator("b")));
    let gone = validator("b");
    a.endpoint
        .send(&gone, MessageClass::Control, b"two")
        .unwrap();
    a.endpoint
        .send(&gone, MessageClass::Bulk, b"three")
        .unwrap();

    // b comes back on the same address with the same key.
    let b = start(validator("b"), 2, "test", Some(&address), |config| {
        config.peers = vec![a.peer()];
    });
    wait_until("the queued frames arrive", || b.received().len() == 2);
    let bodies: Vec<Vec<u8>> = b.received().into_iter().map(|(_, _, body)| body).collect();
    assert!(bodies.contains(&b"two".to_vec()) && bodies.contains(&b"three".to_vec()));
    assert!(a.endpoint.stats().links[&b.id().to_string()].connections >= 2);
}

#[test]
fn oversized_frames_are_refused_and_floods_dropped() {
    let a = start(validator("a"), 1, "test", Some("127.0.0.1:0"), |_| {});
    let b = start(validator("b"), 2, "test", Some("127.0.0.1:0"), |config| {
        config.quotas = Quotas {
            rate: [1_000; 4],
            burst: [2_000; 4],
            disconnect_after: 1_000,
        };
    });
    a.endpoint.set_peers(vec![b.peer()]);
    b.endpoint.set_peers(vec![a.peer()]);
    let too_big = vec![0u8; ClassLimits::default().control + 1];
    assert!(matches!(
        a.endpoint.send(&b.id(), MessageClass::Control, &too_big),
        Err(SendError::Frame(crate::frame::FrameError::TooLarge { .. }))
    ));
    assert!(matches!(
        a.endpoint
            .send(&validator("nobody"), MessageClass::Control, b"x"),
        Err(SendError::UnknownPeer(_))
    ));
    for _ in 0..20 {
        a.endpoint
            .send(&b.id(), MessageClass::Control, &[1u8; 1_000])
            .unwrap();
    }
    wait_until("b drops what exceeds the quota", || {
        b.endpoint.stats().links[&a.id().to_string()].quota_drops >= 10
    });
    assert!(b.received().len() <= 10, "{} delivered", b.received().len());
}
