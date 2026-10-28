//! The transport seam: what travels between nodes encodes canonically
//! within its class's limit, and the in-memory carrier delivers it as the
//! channels did.

use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use sion_consensus::{
    ApprovalProof, BatchHeader, Certificate, Header, LaneRequest, Message, ObjectRef, SignedHeader,
    Vote,
};
use sion_core::hash::Hash32;
use sion_core::ids::Version;
use sion_core::ids::{BlockDigest, Epoch, ObjectId, Pulse, Round, ShardId, TxDigest, ValidatorId};
use sion_net::class::{DUAL_SIGNATURE_BYTES, MAX_TRANSACTION_BYTES};

use super::memory::MemoryNetwork;
use super::*;
use crate::DevnetConfig;

/// The most members a committee can have: the active set's cap.
const MAX_VALIDATORS: usize = 100;

fn limits() -> ClassLimits {
    let core = DevnetConfig::default().core;
    ClassLimits::for_protocol(
        core.max_payload_bytes,
        core.max_items_per_header,
        MAX_VALIDATORS,
    )
}

fn hash() -> impl Strategy<Value = Hash32> {
    any::<[u8; 32]>().prop_map(Hash32)
}

fn validator() -> impl Strategy<Value = ValidatorId> {
    hash().prop_map(ValidatorId::from_hash)
}

fn tx() -> impl Strategy<Value = TxDigest> {
    hash().prop_map(TxDigest::from_hash)
}

fn block() -> impl Strategy<Value = BlockDigest> {
    hash().prop_map(BlockDigest::from_hash)
}

/// `n` bytes for `n` up to `max` (their values do not change the size).
fn bytes(max: usize) -> impl Strategy<Value = Vec<u8>> {
    (0..=max, any::<u8>()).prop_map(|(n, b)| vec![b; n])
}

/// A validator signature: dual post-quantum, or the simulation MAC.
fn signature() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![Just(vec![0x5a; DUAL_SIGNATURE_BYTES]), Just(vec![0x3c; 32]),]
}

/// A header within the protocol's bounds: at most a parent per member, at
/// most `max_items_per_header` items and `max_payload_bytes` of payload.
fn header() -> impl Strategy<Value = Header> {
    let core = DevnetConfig::default().core;
    (
        validator(),
        hash(),
        any::<u64>(),
        any::<u64>(),
        any::<u16>(),
        prop::collection::vec(block(), 0..=MAX_VALIDATORS),
        0..=core.max_items_per_header,
        0..=core.max_payload_bytes,
    )
        .prop_map(
            |(author, chain, round, epoch, shard, parents, items, total)| {
                // Split the payload over the items, the remainder in the first.
                let payload = match items {
                    0 => Vec::new(),
                    n => (0..n)
                        .map(|i| vec![7u8; total / n + usize::from(i == 0) * (total % n)])
                        .collect(),
                };
                Header {
                    author,
                    chain,
                    round: Round(round),
                    epoch: Epoch(epoch),
                    shard: ShardId(shard),
                    parents,
                    payload,
                }
            },
        )
}

fn consensus() -> impl Strategy<Value = Message> {
    prop_oneof![
        (header(), signature())
            .prop_map(|(header, signature)| Message::Header(SignedHeader { header, signature })),
        (block(), validator(), any::<u64>(), validator(), signature()).prop_map(
            |(header, author, round, voter, signature)| Message::Vote(Vote {
                header,
                author,
                round: Round(round),
                voter,
                signature,
            })
        ),
        (
            header(),
            signature(),
            prop::collection::vec((validator(), signature()), 0..=MAX_VALIDATORS)
        )
            .prop_map(|(header, header_signature, votes)| {
                Message::Certificate(Certificate {
                    header,
                    header_signature,
                    votes,
                })
            }),
        prop::collection::vec(block(), 0..=MAX_VALIDATORS).prop_map(Message::SyncRequest),
    ]
}

fn approval() -> impl Strategy<Value = ApprovalProof> {
    (
        validator(),
        hash(),
        any::<u64>(),
        any::<u16>(),
        any::<u64>(),
        hash(),
        any::<u64>(),
        signature(),
        prop::collection::vec(hash(), 0..=64),
    )
        .prop_map(
            |(validator, chain, epoch, shard, index, root, size, signature, path)| ApprovalProof {
                batch: BatchHeader {
                    validator,
                    chain,
                    epoch: Epoch(epoch),
                    shard: ShardId(shard),
                    index,
                    root,
                    size,
                },
                signature,
                leaf_index: size / 2,
                path,
            },
        )
}

/// A lane request: the signed transaction (at most what a gateway accepts)
/// and its inputs listed again, as many as fit in that transaction.
fn lane() -> impl Strategy<Value = LaneRequest> {
    (
        tx(),
        0..=MAX_TRANSACTION_BYTES / 40,
        bytes(MAX_TRANSACTION_BYTES),
    )
        .prop_map(|(tx, inputs, owner_proof)| LaneRequest {
            tx,
            inputs: (0..inputs as u64)
                .map(|i| ObjectRef {
                    id: ObjectId::derive(&i.to_be_bytes()),
                    version: Version(i),
                })
                .collect(),
            owner_proof,
        })
}

fn peer_message() -> impl Strategy<Value = PeerMessage> {
    let core = DevnetConfig::default().core;
    prop_oneof![
        (any::<u64>(), consensus())
            .prop_map(|(session, message)| PeerMessage::Consensus { session, message }),
        bytes(core.max_payload_bytes).prop_map(|item| PeerMessage::Payload { item }),
        (any::<u16>(), lane()).prop_map(|(shard, request)| PeerMessage::Lane {
            shard: ShardId(shard),
            request
        }),
        (any::<u16>(), tx(), approval()).prop_map(|(shard, tx, proof)| PeerMessage::Approval {
            shard: ShardId(shard),
            tx,
            proof
        }),
        (tx(), ".{0,3000}").prop_map(|(tx, reason)| PeerMessage::rejected(tx, reason)),
        (any::<u64>(), hash(), signature()).prop_map(|(pulse, header, signature)| {
            PeerMessage::PulseSignature {
                pulse: Pulse(pulse),
                header,
                signature,
            }
        }),
        any::<u64>().prop_map(|pulse| PeerMessage::PulseSignatureRequest {
            pulse: Pulse(pulse)
        }),
        (any::<u16>(), tx(), approval()).prop_map(|(shard, tx, proof)| {
            PeerMessage::ForcedApproval {
                shard: ShardId(shard),
                tx,
                proof,
            }
        }),
        (any::<u64>(), 0..=EXECUTED_CHUNK).prop_map(|(pulse, n)| PeerMessage::Executed {
            pulse: Pulse(pulse),
            digests: (0..n as u64)
                .map(|i| TxDigest::derive(&i.to_be_bytes()))
                .collect(),
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Every message the protocol can send, at any size its bounds allow,
    /// has one encoding that decodes to it and fits its class's limit.
    #[test]
    fn every_peer_message_round_trips_through_borsh_within_its_class_limit(
        message in peer_message()
    ) {
        let limits = limits();
        let bytes = borsh::to_vec(&message).unwrap();
        prop_assert_eq!(bytes.len(), message.encoded_len());
        prop_assert!(
            bytes.len() <= limits.get(message.class()),
            "a {} of {} bytes exceeds the {} limit of {}",
            message.kind(),
            bytes.len(),
            message.class().name(),
            limits.get(message.class())
        );
        let decoded: PeerMessage = borsh::from_slice(&bytes).unwrap();
        prop_assert_eq!(decoded, message);
    }
}

/// The largest certificate a committee of 100 can form, with a full
/// payload, still fits the consensus class.
#[test]
fn the_largest_certificate_fits_the_consensus_class() {
    let core = DevnetConfig::default().core;
    let items = core.max_items_per_header;
    let header = Header {
        author: ValidatorId::derive(b"author"),
        chain: Hash32::ZERO,
        round: Round(u64::MAX),
        epoch: Epoch(u64::MAX),
        shard: ShardId(u16::MAX),
        parents: (0..MAX_VALIDATORS as u64)
            .map(|i| BlockDigest::derive(&i.to_be_bytes()))
            .collect(),
        payload: (0..items)
            .map(|_| vec![1u8; core.max_payload_bytes / items])
            .collect(),
    };
    let message = PeerMessage::Consensus {
        session: u64::MAX,
        message: Message::Certificate(Certificate {
            header,
            header_signature: vec![0; DUAL_SIGNATURE_BYTES],
            votes: (0..MAX_VALIDATORS as u64)
                .map(|i| {
                    (
                        ValidatorId::derive(&i.to_be_bytes()),
                        vec![0; DUAL_SIGNATURE_BYTES],
                    )
                })
                .collect(),
        }),
    };
    assert!(message.encoded_len() <= limits().consensus);
    assert!(message.encoded_len() > 2 * 1024 * 1024, "about 2.4 MB");
}

#[test]
fn refusal_reasons_are_cut_on_a_character_boundary() {
    let long = "é".repeat(MAX_REASON_BYTES);
    let PeerMessage::LaneRejected { reason, .. } =
        PeerMessage::rejected(TxDigest::derive(b"t"), long)
    else {
        unreachable!()
    };
    assert!(reason.len() <= MAX_REASON_BYTES && reason.len() >= MAX_REASON_BYTES - 1);
    assert!(reason.chars().all(|c| c == 'é'));
}

type Inbox = Arc<Mutex<Vec<(NodeId, PeerMessage)>>>;

fn node(name: &[u8]) -> NodeId {
    NodeId::Validator(ValidatorId::derive(name))
}

fn endpoint(network: &Arc<MemoryNetwork>, id: NodeId) -> (memory::MemoryTransport, Inbox) {
    let inbox: Inbox = Arc::default();
    let sink = inbox.clone();
    let transport = network.endpoint(
        id,
        Arc::new(move |from, message| sink.lock().unwrap().push((from, message))),
    );
    (transport, inbox)
}

fn request(n: u64) -> PeerMessage {
    PeerMessage::PulseSignatureRequest { pulse: Pulse(n) }
}

#[test]
fn the_memory_transport_stamps_the_sender_and_keeps_the_order() {
    let network = MemoryNetwork::new(limits(), false);
    let (a, _) = endpoint(&network, node(b"a"));
    let (b, b_inbox) = endpoint(&network, node(b"b"));
    let (c, c_inbox) = endpoint(&network, node(b"c"));
    for n in 0..5 {
        a.send(b.local(), request(n));
    }
    c.send_all(&[a.local(), b.local(), node(b"nobody")], &request(9));

    let got = b_inbox.lock().unwrap().clone();
    let expected: Vec<(NodeId, PeerMessage)> = (0..5)
        .map(|n| (a.local(), request(n)))
        .chain([(c.local(), request(9))])
        .collect();
    assert_eq!(got, expected);
    assert!(c_inbox.lock().unwrap().is_empty());
    assert!(a.knows(&b.local()) && !a.knows(&node(b"nobody")));

    // Five sent to b; the broadcast reached a and b and dropped the
    // unknown node.
    let stats = c.stats();
    assert_eq!(stats.transport, "memory");
    assert_eq!(stats.sent().messages, 2);
    assert_eq!(stats.dropped(), 1);
    let received = b.stats();
    assert_eq!(received.received().messages, 6);
    assert_eq!(received.peers[&a.local().to_string()].received.messages, 5);
    assert_eq!(
        received.kinds["pulse_signature_request"].received.bytes,
        6 * request(0).encoded_len() as u64
    );
}

#[test]
fn messages_over_their_class_limit_are_dropped_and_counted() {
    let small = ClassLimits {
        control: 64,
        consensus: 64,
        ingress: 64,
        bulk: 64,
    };
    for encode in [false, true] {
        let network = MemoryNetwork::new(small, encode);
        let (a, _) = endpoint(&network, node(b"a"));
        let (b, b_inbox) = endpoint(&network, node(b"b"));
        a.send(b.local(), PeerMessage::Payload { item: vec![0; 60] });
        a.send(b.local(), PeerMessage::Payload { item: vec![1; 10] });
        assert_eq!(
            *b_inbox.lock().unwrap(),
            vec![(a.local(), PeerMessage::Payload { item: vec![1; 10] })]
        );
        let stats = a.stats();
        assert_eq!(stats.classes["ingress"].dropped, 1);
        assert_eq!(stats.classes["ingress"].sent.messages, 1);
    }
}

#[test]
fn the_encoding_mode_delivers_an_equal_copy() {
    let network = MemoryNetwork::new(limits(), true);
    let (a, _) = endpoint(&network, node(b"a"));
    let (b, b_inbox) = endpoint(&network, node(b"b"));
    let message = PeerMessage::Executed {
        pulse: Pulse(3),
        digests: vec![TxDigest::derive(b"x"), TxDigest::derive(b"y")],
    };
    a.send(b.local(), message.clone());
    assert_eq!(*b_inbox.lock().unwrap(), vec![(a.local(), message)]);
}
