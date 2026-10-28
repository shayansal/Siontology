//! Privacy and forgetting: epoch keys, per-atom keys, sealed payloads, key
//! wrapping for members, and key destruction.
//!
//! - **Epoch keys.** A memory space has one 32-byte random key per epoch,
//!   from a generator the caller passes in (the operating system's in
//!   production, a seeded one in tests). Its id is
//!   `Hp("sion/mem/epoch-key-id", [space, u64_be(epoch), key])`.
//! - **Per-atom keys.** Each payload is encrypted under
//!   `Hp("sion/mem/atom-key", [epoch_key, writer, u64_be(seq)])`, so no two
//!   atoms share a key, with ChaCha20-Poly1305, a random 12-byte nonce, and
//!   as associated data `"sion/mem/payload" ‖ writer ‖ u64_le(seq) ‖ key_id`
//!   (the atom's identifying fields: a payload cannot be moved to another
//!   atom or key).
//! - **Sharing.** An epoch key is wrapped to a member's ML-KEM-768 public key
//!   with `sion_crypto::kem::seal` in the domain `sion/mem/epoch-key`, with
//!   the space, epoch and key id as associated data.
//! - **Forgetting.** Destroying an epoch key ([`KeyRing::destroy`]) makes
//!   every payload sealed under it unreadable; an `Erasure { key }` atom
//!   records it. The commitments (atom ids, log and index roots) remain.
//!   Copies of the key wrapped to members are outside the writer's control,
//!   so erasure holds only if every member destroys theirs too: a trust
//!   assumption to name wherever FORGET is relied on.
//!
//! `Hp` is `sion_core::hash::hash_parts` (length-prefixed parts).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use sion_core::encode::{decode, encode};
use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::AccountId;
use sion_crypto::kem::{self, KemKeypair, KemPublicKey, SealedEnvelope};
use zeroize::Zeroizing;

use crate::atom::{AtomError, EpochKeyId, MemoryAtom, Payload, SealedPayload, MAX_PAYLOAD_BYTES};
use crate::error::{MemoryError, Result};

/// Domain of per-atom keys.
pub const ATOM_KEY_DOMAIN: &str = "sion/mem/atom-key";
/// Domain of epoch-key ids.
pub const EPOCH_KEY_ID_DOMAIN: &str = "sion/mem/epoch-key-id";
/// Domain under which epoch keys are wrapped to members (ML-KEM-768).
pub const EPOCH_KEY_DOMAIN: &str = "sion/mem/epoch-key";
/// Prefix of a payload's associated data.
pub const PAYLOAD_AAD_PREFIX: &[u8] = b"sion/mem/payload";

/// A memory space's key for one epoch.
#[derive(Clone)]
pub struct EpochKey {
    space: Hash32,
    epoch: u64,
    key: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for EpochKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EpochKey(space {}, epoch {}, id {}, secret redacted)",
            self.space.short(),
            self.epoch,
            self.id().short()
        )
    }
}

impl EpochKey {
    /// A fresh random key for `epoch` of the memory space `space`.
    pub fn generate(space: Hash32, epoch: u64, rng: &mut impl CryptoRngCore) -> EpochKey {
        let mut key = Zeroizing::new([0u8; 32]);
        rng.fill_bytes(key.as_mut());
        EpochKey { space, epoch, key }
    }

    /// A key from its bytes.
    pub fn from_bytes(space: Hash32, epoch: u64, key: [u8; 32]) -> EpochKey {
        EpochKey {
            space,
            epoch,
            key: Zeroizing::new(key),
        }
    }

    /// The memory space.
    pub fn space(&self) -> Hash32 {
        self.space
    }

    /// The epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The key's id: `Hp("sion/mem/epoch-key-id", [space, u64_be(epoch), key])`.
    pub fn id(&self) -> EpochKeyId {
        EpochKeyId(hash_parts(
            EPOCH_KEY_ID_DOMAIN,
            &[&self.space.0, &self.epoch.to_be_bytes(), self.key.as_ref()],
        ))
    }

    /// The key of the atom at `seq` in `writer`'s log.
    pub fn atom_key(&self, writer: &AccountId, seq: u64) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(
            hash_parts(
                ATOM_KEY_DOMAIN,
                &[self.key.as_ref(), &writer.0 .0, &seq.to_be_bytes()],
            )
            .0,
        )
    }

    /// Seals a payload for the atom at `seq` in `writer`'s log.
    pub fn seal(
        &self,
        writer: &AccountId,
        seq: u64,
        plaintext: &[u8],
        rng: &mut impl CryptoRngCore,
    ) -> Result<SealedPayload> {
        if plaintext.len() > MAX_PAYLOAD_BYTES {
            return Err(MemoryError::InvalidAtom(AtomError::TooLong {
                field: "sealed payload",
                len: plaintext.len(),
                max: MAX_PAYLOAD_BYTES,
            }));
        }
        let key_id = self.id();
        let mut nonce = [0u8; 12];
        rng.fill_bytes(&mut nonce);
        let atom_key = self.atom_key(writer, seq);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(atom_key.as_ref()));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: plaintext,
                    aad: &payload_aad(writer, seq, &key_id),
                },
            )
            .map_err(|_| MemoryError::Decryption)?;
        Ok(SealedPayload {
            key: key_id,
            nonce,
            ciphertext,
        })
    }

    /// Opens an atom's sealed payload.
    pub fn open(&self, atom: &MemoryAtom) -> Result<Vec<u8>> {
        let Payload::Sealed(sealed) = &atom.payload else {
            return Err(MemoryError::NotSealed);
        };
        let key_id = self.id();
        if sealed.key != key_id {
            return Err(MemoryError::UnknownKey(sealed.key));
        }
        let atom_key = self.atom_key(&atom.writer, atom.seq);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(atom_key.as_ref()));
        cipher
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                AeadPayload {
                    msg: &sealed.ciphertext,
                    aad: &payload_aad(&atom.writer, atom.seq, &key_id),
                },
            )
            .map_err(|_| MemoryError::Decryption)
    }

    /// Wraps the key to a member's ML-KEM-768 public key.
    pub fn wrap(
        &self,
        member: &KemPublicKey,
        rng: &mut impl CryptoRngCore,
    ) -> Result<WrappedEpochKey> {
        let key = self.id();
        let envelope = kem::seal(
            member,
            EPOCH_KEY_DOMAIN,
            &wrap_aad(&self.space, self.epoch, &key),
            self.key.as_ref(),
            rng,
        )?;
        Ok(WrappedEpochKey {
            space: self.space,
            epoch: self.epoch,
            key,
            member: member.fingerprint(),
            envelope,
        })
    }

    /// Unwraps a key with the member's ML-KEM-768 keypair, checking its id.
    pub fn unwrap(wrapped: &WrappedEpochKey, keypair: &KemKeypair) -> Result<EpochKey> {
        let bytes = Zeroizing::new(keypair.open(
            EPOCH_KEY_DOMAIN,
            &wrap_aad(&wrapped.space, wrapped.epoch, &wrapped.key),
            &wrapped.envelope,
        )?);
        let array: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| MemoryError::Decryption)?;
        let key = EpochKey::from_bytes(wrapped.space, wrapped.epoch, array);
        if key.id() != wrapped.key {
            return Err(MemoryError::Decryption);
        }
        Ok(key)
    }
}

fn payload_aad(writer: &AccountId, seq: u64, key: &EpochKeyId) -> Vec<u8> {
    let mut aad = Vec::with_capacity(PAYLOAD_AAD_PREFIX.len() + 72);
    aad.extend_from_slice(PAYLOAD_AAD_PREFIX);
    aad.extend_from_slice(&writer.0 .0);
    aad.extend_from_slice(&seq.to_le_bytes());
    aad.extend_from_slice(&key.0 .0);
    aad
}

fn wrap_aad(space: &Hash32, epoch: u64, key: &EpochKeyId) -> Vec<u8> {
    encode(&(space, epoch, key))
}

/// An epoch key sealed to one member.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct WrappedEpochKey {
    /// The memory space.
    pub space: Hash32,
    /// The epoch.
    pub epoch: u64,
    /// The key's id.
    pub key: EpochKeyId,
    /// Fingerprint of the member's ML-KEM public key.
    pub member: Hash32,
    /// The sealed key.
    pub envelope: SealedEnvelope,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct StoredKey {
    space: Hash32,
    epoch: u64,
    key: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize)]
struct KeyFile {
    keys: Vec<StoredKey>,
    destroyed: Vec<EpochKeyId>,
}

/// The epoch keys a writer or member holds, and the ids of those destroyed.
#[derive(Clone, Default)]
pub struct KeyRing {
    keys: BTreeMap<EpochKeyId, EpochKey>,
    destroyed: BTreeSet<EpochKeyId>,
}

impl fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyRing")
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .field("destroyed", &self.destroyed)
            .finish()
    }
}

impl KeyRing {
    /// An empty ring.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a key; a key once destroyed is never taken back.
    pub fn insert(&mut self, key: EpochKey) -> Result<EpochKeyId> {
        let id = key.id();
        if self.destroyed.contains(&id) {
            return Err(MemoryError::KeyDestroyed(id));
        }
        self.keys.insert(id, key);
        Ok(id)
    }

    /// The key with this id.
    pub fn get(&self, id: &EpochKeyId) -> Result<&EpochKey> {
        if self.destroyed.contains(id) {
            return Err(MemoryError::KeyDestroyed(*id));
        }
        self.keys.get(id).ok_or(MemoryError::UnknownKey(*id))
    }

    /// Destroys a key: drops (and zeroizes) it and remembers its id.
    pub fn destroy(&mut self, id: &EpochKeyId) -> Result<()> {
        if self.destroyed.contains(id) {
            return Err(MemoryError::KeyDestroyed(*id));
        }
        if self.keys.remove(id).is_none() {
            return Err(MemoryError::UnknownKey(*id));
        }
        self.destroyed.insert(*id);
        Ok(())
    }

    /// Whether a key was destroyed.
    pub fn is_destroyed(&self, id: &EpochKeyId) -> bool {
        self.destroyed.contains(id)
    }

    /// The ids of the keys held.
    pub fn ids(&self) -> Vec<EpochKeyId> {
        self.keys.keys().copied().collect()
    }

    /// Opens an atom's sealed payload with the key it names.
    pub fn open(&self, atom: &MemoryAtom) -> Result<Vec<u8>> {
        let Payload::Sealed(sealed) = &atom.payload else {
            return Err(MemoryError::NotSealed);
        };
        self.get(&sealed.key)?.open(atom)
    }

    /// The ring's encoding for the key file (zeroized when dropped).
    pub(crate) fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let file = KeyFile {
            keys: self
                .keys
                .values()
                .map(|k| StoredKey {
                    space: k.space,
                    epoch: k.epoch,
                    key: *k.key,
                })
                .collect(),
            destroyed: self.destroyed.iter().copied().collect(),
        };
        let bytes = Zeroizing::new(encode(&file));
        for mut stored in file.keys {
            zeroize::Zeroize::zeroize(&mut stored.key);
        }
        bytes
    }

    /// Decodes a key file's body.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<KeyRing> {
        let file: KeyFile =
            decode(bytes).map_err(|e| MemoryError::Store(format!("key file: {e}")))?;
        let mut ring = KeyRing {
            keys: BTreeMap::new(),
            destroyed: file.destroyed.into_iter().collect(),
        };
        for mut stored in file.keys {
            let key = EpochKey::from_bytes(stored.space, stored.epoch, stored.key);
            zeroize::Zeroize::zeroize(&mut stored.key);
            ring.insert(key)?;
        }
        Ok(ring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::{AtomClass, MemoryKind, Statement, Value};

    fn writer() -> AccountId {
        AccountId::derive(b"seal-test")
    }

    fn sealed_atom(key: &EpochKey, seq: u64, plaintext: &[u8]) -> MemoryAtom {
        let mut rng = sion_core::rng::seeded(seq);
        let sealed = key.seal(&writer(), seq, plaintext, &mut rng).unwrap();
        MemoryAtom::builder(
            writer(),
            seq,
            MemoryKind::Episodic,
            AtomClass::AgentMemory,
            Statement::claim("user", "said", Value::Text("(sealed)".into())),
        )
        .payload(Payload::Sealed(sealed))
        .build()
        .unwrap()
    }

    fn key() -> EpochKey {
        EpochKey::generate(Hash32([7; 32]), 0, &mut sion_core::rng::seeded(1))
    }

    #[test]
    fn sealed_payloads_open_with_their_key_only() {
        let key = key();
        let atom = sealed_atom(&key, 3, b"the door code is 1234");
        assert_eq!(atom.policy.erasure_key, Some(key.id()));
        assert_eq!(key.open(&atom).unwrap(), b"the door code is 1234");
        let other = EpochKey::generate(Hash32([7; 32]), 1, &mut sion_core::rng::seeded(2));
        assert!(matches!(other.open(&atom), Err(MemoryError::UnknownKey(_))));
        // Moving the ciphertext to another position or writer fails.
        let mut moved = atom.clone();
        moved.seq = 4;
        assert_eq!(key.open(&moved), Err(MemoryError::Decryption));
        let mut stolen = atom.clone();
        stolen.writer = AccountId::derive(b"thief");
        assert_eq!(key.open(&stolen), Err(MemoryError::Decryption));
        let mut tampered = atom.clone();
        if let Payload::Sealed(s) = &mut tampered.payload {
            s.ciphertext[0] ^= 1;
        }
        assert_eq!(key.open(&tampered), Err(MemoryError::Decryption));
        let public = MemoryAtom::builder(
            writer(),
            0,
            MemoryKind::Episodic,
            AtomClass::AgentMemory,
            Statement::claim("a", "b", Value::Boolean(true)),
        )
        .build()
        .unwrap();
        assert_eq!(key.open(&public), Err(MemoryError::NotSealed));
        assert!(key
            .seal(
                &writer(),
                0,
                &vec![0; MAX_PAYLOAD_BYTES + 1],
                &mut sion_core::rng::seeded(0)
            )
            .is_err());
    }

    #[test]
    fn per_atom_keys_differ_and_ids_bind_space_and_epoch() {
        let key = key();
        assert_ne!(
            key.atom_key(&writer(), 0).as_ref(),
            key.atom_key(&writer(), 1).as_ref()
        );
        assert_ne!(
            key.atom_key(&writer(), 0).as_ref(),
            key.atom_key(&AccountId::derive(b"x"), 0).as_ref()
        );
        let same_bytes = EpochKey::from_bytes(Hash32([8; 32]), 0, *key.key);
        assert_ne!(same_bytes.id(), key.id());
        assert_eq!(key.space(), Hash32([7; 32]));
        assert_eq!(key.epoch(), 0);
        assert!(format!("{key:?}").contains("redacted"));
    }

    #[test]
    fn forgetting_makes_payloads_unreadable() {
        let mut ring = KeyRing::new();
        let key = key();
        let id = ring.insert(key.clone()).unwrap();
        let atom = sealed_atom(&key, 0, b"private");
        assert_eq!(ring.open(&atom).unwrap(), b"private");
        ring.destroy(&id).unwrap();
        assert!(ring.is_destroyed(&id));
        assert_eq!(ring.open(&atom), Err(MemoryError::KeyDestroyed(id)));
        assert_eq!(ring.destroy(&id), Err(MemoryError::KeyDestroyed(id)));
        assert_eq!(ring.insert(key), Err(MemoryError::KeyDestroyed(id)));
        assert!(ring.ids().is_empty());
        let unknown = EpochKeyId(Hash32([1; 32]));
        assert_eq!(
            ring.destroy(&unknown),
            Err(MemoryError::UnknownKey(unknown))
        );
        assert!(matches!(
            ring.get(&unknown),
            Err(MemoryError::UnknownKey(_))
        ));
        // The ring survives its encoding, destroyed ids included.
        let back = KeyRing::from_bytes(&ring.to_bytes()).unwrap();
        assert!(back.is_destroyed(&id));
        assert!(format!("{back:?}").contains("destroyed"));
        assert!(KeyRing::from_bytes(b"junk").is_err());
    }

    #[test]
    fn keys_wrap_to_members() {
        let key = key();
        let member = KemKeypair::from_seed(&[4; 32]);
        let stranger = KemKeypair::from_seed(&[5; 32]);
        let mut rng = sion_core::rng::seeded(9);
        let wrapped = key.wrap(member.public(), &mut rng).unwrap();
        assert_eq!(wrapped.member, member.public().fingerprint());
        let unwrapped = EpochKey::unwrap(&wrapped, &member).unwrap();
        assert_eq!(unwrapped.id(), key.id());
        let atom = sealed_atom(&key, 1, b"shared");
        assert_eq!(unwrapped.open(&atom).unwrap(), b"shared");
        assert!(EpochKey::unwrap(&wrapped, &stranger).is_err());
        let mut relabelled = wrapped.clone();
        relabelled.epoch = 1;
        assert!(EpochKey::unwrap(&relabelled, &member).is_err());
    }
}
