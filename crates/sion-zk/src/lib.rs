//! Zero-knowledge proofs about committed numbers.
//!
//! Someone who holds numbers it will not show (what an agent paid, to
//! whom, how often a run went badly) can still be held to rules about
//! them. It **commits** to each number: a commitment hides the number and
//! binds its holder to it. Then it **proves**, to anyone holding only the
//! commitments, that the numbers keep a rule, and the proof shows nothing
//! else about them.
//!
//! Two kinds of rule can be proven here:
//!
//! - **Linear rules over amounts** ([`prove`], [`verify`]): each committed
//!   amount is a whole number from 0 up to 2⁶⁴ − 1, and any number of
//!   statements of the form `a₁·v₁ + a₂·v₂ + … ≤ b` hold, with whole
//!   coefficients that may be negative. "No payment was above 10 USD",
//!   "all payments came to at most 500 USD" and "at most one call in fifty
//!   failed" are all of this form.
//! - **Membership** ([`prove_member`], [`verify_member`]): a committed name
//!   (a payee, say) is one of a public list, without showing which.
//!
//! How: commitments are Pedersen commitments on the Ristretto group. A
//! linear rule is turned into a commitment to its slack `b − Σ aᵢ·vᵢ`,
//! which anyone computes from the commitments, and the slack is shown to
//! be a whole number from 0 up to 2⁶⁴ − 1 by an aggregated Bulletproofs
//! range proof (the `bulletproofs` crate). Membership is a one-of-many
//! Schnorr proof (Cramer, Damgård and Schoenmakers). There is no trusted
//! setup.
//!
//! What these proofs rest on, and what they do not show:
//!
//! - **Hiding does not depend on any hardness assumption**: a commitment
//!   says nothing about its number, whatever computer looks at it.
//! - **Soundness does.** Someone who can compute discrete logarithms in
//!   this group, as a large quantum computer could, can open a commitment
//!   to another number and prove a false statement. Proofs here are for
//!   showing third parties that rules were kept. They are not fit for
//!   anything a ledger with post-quantum accounts would create or move
//!   money by.
//! - **A proof is about the numbers committed to.** That those are the
//!   numbers of what really happened is not something a proof can show:
//!   it has to come from when and by whom the commitments were made.

use bulletproofs::{BulletproofGens, PedersenGens, RangeProof};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use merlin::Transcript;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha3::Sha3_512;

/// The bits of an amount: amounts are whole numbers below 2⁶⁴.
const BITS: usize = 64;

/// The most numbers one range proof covers: more are proven in several.
const PER_PROOF: usize = 64;

/// The most committed amounts and statements one proof takes.
pub const MAX_VALUES: usize = 4_096;

/// The most names a list a membership is proven in may hold.
pub const MAX_SET: usize = 1_024;

/// Why a proof could not be made, or does not hold.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A statement to prove is not true of the numbers committed to.
    #[error("statement {0} does not hold of these numbers")]
    False(usize),
    /// A statement names a commitment that is not there.
    #[error("statement {0} names a number that is not among those committed to")]
    NoSuchValue(usize),
    /// Too many numbers or statements, or too long a list.
    #[error("too many: at most {0}")]
    TooMany(usize),
    /// The committed name is not in the list.
    #[error("the name committed to is not in the list")]
    NotAMember,
    /// A commitment or a proof is not well formed.
    #[error("not well formed: {0}")]
    Malformed(&'static str),
    /// The proof does not hold for these commitments and statements.
    #[error("the proof does not hold")]
    Invalid,
}

/// A commitment to a number or a name: it hides what it commits to and
/// binds whoever made it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Commitment(pub [u8; 32]);

impl Commitment {
    fn point(&self) -> Result<RistrettoPoint, Error> {
        CompressedRistretto(self.0)
            .decompress()
            .ok_or(Error::Malformed("a commitment is not a point of the group"))
    }

    /// The commitment in hex.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// The commitment `hex` writes.
    pub fn from_hex(text: &str) -> Result<Commitment, Error> {
        let bytes = hex::decode(text.trim()).map_err(|_| Error::Malformed("not hex"))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Malformed("a commitment is 32 bytes"))?;
        Ok(Commitment(bytes))
    }
}

impl std::fmt::Debug for Commitment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Commitment({})", self.to_hex())
    }
}

impl Serialize for Commitment {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Commitment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Commitment, D::Error> {
        let text = String::deserialize(deserializer)?;
        Commitment::from_hex(&text).map_err(serde::de::Error::custom)
    }
}

fn scalar_hex<S: serde::Serializer>(scalar: &Scalar, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&hex::encode(scalar.to_bytes()))
}

fn hex_scalar<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Scalar, D::Error> {
    let text = String::deserialize(deserializer)?;
    let bytes: [u8; 32] = hex::decode(text.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| serde::de::Error::custom("a scalar is 32 bytes of hex"))?;
    Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes))
        .ok_or_else(|| serde::de::Error::custom("not a scalar in its canonical form"))
}

/// What opens a commitment to an amount: the amount, and the secret that
/// hides it. Whoever holds it can prove things about the amount; whoever
/// is shown it learns the amount.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    /// The amount.
    pub value: u64,
    #[serde(serialize_with = "scalar_hex", deserialize_with = "hex_scalar")]
    blinding: Scalar,
}

impl std::fmt::Debug for Opening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Opening(..)")
    }
}

impl Opening {
    /// The commitment it opens.
    pub fn commitment(&self) -> Commitment {
        let gens = PedersenGens::default();
        Commitment(
            gens.commit(Scalar::from(self.value), self.blinding)
                .compress()
                .to_bytes(),
        )
    }
}

/// Commits to `value`: the commitment to show, and the opening to keep.
pub fn commit(value: u64) -> (Commitment, Opening) {
    let opening = Opening {
        value,
        blinding: Scalar::random(&mut OsRng),
    };
    (opening.commitment(), opening)
}

/// A statement about committed amounts: `Σ coefficient · amount ≤ bound`,
/// each amount named by its place among the commitments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    /// The terms: which amount, and what it is multiplied by.
    pub terms: Vec<(usize, i64)>,
    /// What the sum is at most.
    pub bound: i64,
}

impl Statement {
    /// `amount[at] ≤ most`.
    pub fn at_most(at: usize, most: u64) -> Statement {
        Statement {
            terms: vec![(at, 1)],
            bound: most.min(i64::MAX as u64) as i64,
        }
    }

    /// `amount[at] ≥ least`.
    pub fn at_least(at: usize, least: u64) -> Statement {
        Statement {
            terms: vec![(at, -1)],
            bound: -(least.min(i64::MAX as u64) as i64),
        }
    }

    /// `Σ amount[i] ≤ most`, over `these`.
    pub fn sum_at_most(these: impl IntoIterator<Item = usize>, most: u64) -> Statement {
        Statement {
            terms: these.into_iter().map(|at| (at, 1)).collect(),
            bound: most.min(i64::MAX as u64) as i64,
        }
    }
}

/// A scalar for a whole number that may be negative.
fn signed(value: i64) -> Scalar {
    let magnitude = Scalar::from(value.unsigned_abs());
    if value < 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// A proof that committed amounts are whole numbers below 2⁶⁴ and keep
/// some statements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// The range proofs, in hex: one for every [`PER_PROOF`] numbers shown
    /// to be in range (the amounts, then each statement's slack).
    pub ranges: Vec<String>,
}

fn transcript(context: &[u8], statements: &[Statement], commitments: &[Commitment]) -> Transcript {
    let mut transcript = Transcript::new(b"sion-zk/linear/v1");
    transcript.append_message(b"context", context);
    transcript.append_u64(b"amounts", commitments.len() as u64);
    for commitment in commitments {
        transcript.append_message(b"commitment", &commitment.0);
    }
    transcript.append_u64(b"statements", statements.len() as u64);
    for statement in statements {
        transcript.append_u64(b"terms", statement.terms.len() as u64);
        for (at, coefficient) in &statement.terms {
            transcript.append_u64(b"at", *at as u64);
            transcript.append_message(b"coefficient", &coefficient.to_le_bytes());
        }
        transcript.append_message(b"bound", &statement.bound.to_le_bytes());
    }
    transcript
}

/// The commitment to a statement's slack, `bound − Σ coefficient · amount`,
/// as anyone computes it from the amounts' commitments.
fn slack_point(
    statement: &Statement,
    at: usize,
    points: &[RistrettoPoint],
) -> Result<RistrettoPoint, Error> {
    let gens = PedersenGens::default();
    let mut slack = gens.B * signed(statement.bound);
    for (index, coefficient) in &statement.terms {
        let point = points.get(*index).ok_or(Error::NoSuchValue(at))?;
        slack -= point * signed(*coefficient);
    }
    Ok(slack)
}

fn check_sizes(amounts: usize, statements: usize) -> Result<(), Error> {
    if amounts + statements > MAX_VALUES || amounts == 0 {
        return Err(Error::TooMany(MAX_VALUES));
    }
    Ok(())
}

/// Proves, about the amounts `openings` open, that each `statements`
/// holds, bound to `context` (what the proof is for: it does not hold for
/// another). The proof shows nothing else about the amounts.
pub fn prove(
    context: &[u8],
    openings: &[Opening],
    statements: &[Statement],
) -> Result<Proof, Error> {
    check_sizes(openings.len(), statements.len())?;
    let commitments: Vec<Commitment> = openings.iter().map(Opening::commitment).collect();
    // What is shown to be in range: every amount, then every slack.
    let mut values: Vec<u64> = openings.iter().map(|o| o.value).collect();
    let mut blindings: Vec<Scalar> = openings.iter().map(|o| o.blinding).collect();
    for (at, statement) in statements.iter().enumerate() {
        let mut sum: i128 = 0;
        let mut blinding = Scalar::ZERO;
        for (index, coefficient) in &statement.terms {
            let opening = openings.get(*index).ok_or(Error::NoSuchValue(at))?;
            sum += i128::from(*coefficient) * i128::from(opening.value);
            blinding -= opening.blinding * signed(*coefficient);
        }
        let slack = i128::from(statement.bound) - sum;
        let slack = u64::try_from(slack).map_err(|_| Error::False(at))?;
        values.push(slack);
        blindings.push(blinding);
    }
    let pedersen = PedersenGens::default();
    let mut transcript = transcript(context, statements, &commitments);
    let mut ranges = Vec::new();
    for (values, blindings) in values.chunks(PER_PROOF).zip(blindings.chunks(PER_PROOF)) {
        // A range proof takes a power of two of numbers: the rest are zero,
        // committed to with nothing hiding them.
        let size = values.len().next_power_of_two();
        let mut padded_values = values.to_vec();
        let mut padded_blindings = blindings.to_vec();
        padded_values.resize(size, 0);
        padded_blindings.resize(size, Scalar::ZERO);
        let gens = BulletproofGens::new(BITS, size);
        let (proof, _) = RangeProof::prove_multiple_with_rng(
            &gens,
            &pedersen,
            &mut transcript,
            &padded_values,
            &padded_blindings,
            BITS,
            &mut OsRng,
        )
        .map_err(|_| Error::Malformed("the range proof could not be made"))?;
        ranges.push(hex::encode(proof.to_bytes()));
    }
    Ok(Proof { ranges })
}

/// Checks that `proof` shows the amounts committed to in `commitments`
/// are whole numbers below 2⁶⁴ that keep every one of `statements`, for
/// this `context`.
pub fn verify(
    context: &[u8],
    commitments: &[Commitment],
    statements: &[Statement],
    proof: &Proof,
) -> Result<(), Error> {
    check_sizes(commitments.len(), statements.len())?;
    let points: Vec<RistrettoPoint> = commitments
        .iter()
        .map(Commitment::point)
        .collect::<Result<_, _>>()?;
    let mut shown: Vec<CompressedRistretto> = commitments
        .iter()
        .map(|c| CompressedRistretto(c.0))
        .collect();
    for (at, statement) in statements.iter().enumerate() {
        shown.push(slack_point(statement, at, &points)?.compress());
    }
    if proof.ranges.len() != shown.len().div_ceil(PER_PROOF) {
        return Err(Error::Invalid);
    }
    let pedersen = PedersenGens::default();
    let mut transcript = transcript(context, statements, commitments);
    for (shown, range) in shown.chunks(PER_PROOF).zip(&proof.ranges) {
        let size = shown.len().next_power_of_two();
        let mut padded = shown.to_vec();
        padded.resize(size, RistrettoPoint::identity().compress());
        let bytes = hex::decode(range).map_err(|_| Error::Malformed("a proof is not hex"))?;
        let range =
            RangeProof::from_bytes(&bytes).map_err(|_| Error::Malformed("not a range proof"))?;
        let gens = BulletproofGens::new(BITS, size);
        range
            .verify_multiple_with_rng(&gens, &pedersen, &mut transcript, &padded, BITS, &mut OsRng)
            .map_err(|_| Error::Invalid)?;
    }
    Ok(())
}

/// A name as proofs speak of it: any bytes (an account's id, a payee's
/// name), taken to a number of the group's field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name(Scalar);

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Name({})", hex::encode(&self.0.to_bytes()[..8]))
    }
}

/// The name `bytes` is.
pub fn name(bytes: &[u8]) -> Name {
    let mut input = b"sion-zk/name/v1:".to_vec();
    input.extend_from_slice(bytes);
    Name(Scalar::hash_from_bytes::<Sha3_512>(&input))
}

/// What opens a commitment to a name.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameOpening {
    #[serde(serialize_with = "scalar_hex", deserialize_with = "hex_scalar")]
    name: Scalar,
    #[serde(serialize_with = "scalar_hex", deserialize_with = "hex_scalar")]
    blinding: Scalar,
}

impl std::fmt::Debug for NameOpening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NameOpening(..)")
    }
}

impl NameOpening {
    /// The commitment it opens.
    pub fn commitment(&self) -> Commitment {
        let gens = PedersenGens::default();
        Commitment(gens.commit(self.name, self.blinding).compress().to_bytes())
    }
}

/// Commits to `name`.
pub fn commit_name(name: &Name) -> (Commitment, NameOpening) {
    let opening = NameOpening {
        name: name.0,
        blinding: Scalar::random(&mut OsRng),
    };
    (opening.commitment(), opening)
}

/// A proof that a committed name is one of a list, without showing which.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProof {
    /// A challenge and a response for each name of the list, in the list's
    /// sorted order, in hex.
    pub parts: Vec<(String, String)>,
}

/// The list as both sides order it: sorted, each name once.
fn sorted(set: &[Name]) -> Result<Vec<Scalar>, Error> {
    let mut names: Vec<[u8; 32]> = set.iter().map(|name| name.0.to_bytes()).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() || names.len() > MAX_SET {
        return Err(Error::TooMany(MAX_SET));
    }
    Ok(names
        .into_iter()
        .filter_map(|bytes| Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes)))
        .collect())
}

fn member_transcript(context: &[u8], commitment: &Commitment, set: &[Scalar]) -> Transcript {
    let mut transcript = Transcript::new(b"sion-zk/member/v1");
    transcript.append_message(b"context", context);
    transcript.append_message(b"commitment", &commitment.0);
    transcript.append_u64(b"names", set.len() as u64);
    for name in set {
        transcript.append_message(b"name", &name.to_bytes());
    }
    transcript
}

fn challenge(transcript: &mut Transcript, announcements: &[RistrettoPoint]) -> Scalar {
    for announcement in announcements {
        transcript.append_message(b"announcement", &announcement.compress().to_bytes());
    }
    let mut wide = [0u8; 64];
    transcript.challenge_bytes(b"challenge", &mut wide);
    Scalar::from_bytes_mod_order_wide(&wide)
}

/// Proves that the name `opening` opens is one of `set`, bound to
/// `context`. The proof does not show which.
pub fn prove_member(
    context: &[u8],
    opening: &NameOpening,
    set: &[Name],
) -> Result<MemberProof, Error> {
    let names = sorted(set)?;
    let own = names
        .iter()
        .position(|name| *name == opening.name)
        .ok_or(Error::NotAMember)?;
    let gens = PedersenGens::default();
    let commitment = opening.commitment();
    let point = commitment.point()?;
    // For each name, the point that is a multiple of the hiding generator
    // alone exactly when the commitment is to that name.
    let residues: Vec<RistrettoPoint> = names.iter().map(|name| point - gens.B * name).collect();
    let nonce = Scalar::random(&mut OsRng);
    let mut challenges = vec![Scalar::ZERO; names.len()];
    let mut responses = vec![Scalar::ZERO; names.len()];
    let mut announcements = Vec::with_capacity(names.len());
    for (at, residue) in residues.iter().enumerate() {
        if at == own {
            announcements.push(gens.B_blinding * nonce);
        } else {
            // Made up backwards for the names it is not: a challenge and a
            // response first, then the announcement they answer.
            challenges[at] = Scalar::random(&mut OsRng);
            responses[at] = Scalar::random(&mut OsRng);
            announcements.push(gens.B_blinding * responses[at] - residue * challenges[at]);
        }
    }
    let mut transcript = member_transcript(context, &commitment, &names);
    let total = challenge(&mut transcript, &announcements);
    let others: Scalar = challenges.iter().sum();
    challenges[own] = total - others;
    responses[own] = nonce + challenges[own] * opening.blinding;
    Ok(MemberProof {
        parts: challenges
            .iter()
            .zip(&responses)
            .map(|(c, z)| (hex::encode(c.to_bytes()), hex::encode(z.to_bytes())))
            .collect(),
    })
}

fn scalar_of(text: &str) -> Result<Scalar, Error> {
    let bytes: [u8; 32] = hex::decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::Malformed("a proof's number is 32 bytes of hex"))?;
    Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes)).ok_or(Error::Malformed(
        "a proof's number is not in its canonical form",
    ))
}

/// Checks that `proof` shows `commitment` to be to one of `set`, for this
/// `context`.
pub fn verify_member(
    context: &[u8],
    commitment: &Commitment,
    set: &[Name],
    proof: &MemberProof,
) -> Result<(), Error> {
    let names = sorted(set)?;
    if proof.parts.len() != names.len() {
        return Err(Error::Invalid);
    }
    let gens = PedersenGens::default();
    let point = commitment.point()?;
    let mut announcements = Vec::with_capacity(names.len());
    let mut sum = Scalar::ZERO;
    for (name, (c, z)) in names.iter().zip(&proof.parts) {
        let (c, z) = (scalar_of(c)?, scalar_of(z)?);
        let residue = point - gens.B * name;
        announcements.push(gens.B_blinding * z - residue * c);
        sum += c;
    }
    let mut transcript = member_transcript(context, commitment, &names);
    if challenge(&mut transcript, &announcements) == sum {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed(values: &[u64]) -> (Vec<Commitment>, Vec<Opening>) {
        values.iter().map(|v| commit(*v)).unzip()
    }

    #[test]
    fn a_rule_that_holds_is_proven_and_checked_without_the_numbers() {
        // Three payments, of 5, 8 and 2 USD: none above 10, 15 in all.
        let (commitments, openings) = committed(&[5, 8, 2]);
        let rules = vec![
            Statement::at_most(0, 10),
            Statement::at_most(1, 10),
            Statement::at_most(2, 10),
            Statement::sum_at_most(0..3, 15),
            Statement::at_least(1, 8),
        ];
        let proof = prove(b"audit 7", &openings, &rules).unwrap();
        assert_eq!(verify(b"audit 7", &commitments, &rules, &proof), Ok(()));
        // The proof and the commitments say nothing a reader can use: the
        // same numbers committed to again look different.
        assert_ne!(commit(5).0, commit(5).0);
        // It goes through a file as it is.
        let text = serde_json::to_string(&(&commitments, &rules, &proof)).unwrap();
        let (c, r, p): (Vec<Commitment>, Vec<Statement>, Proof) =
            serde_json::from_str(&text).unwrap();
        assert_eq!(verify(b"audit 7", &c, &r, &p), Ok(()));
        let kept: Opening =
            serde_json::from_str(&serde_json::to_string(&openings[0]).unwrap()).unwrap();
        assert_eq!(kept.commitment(), commitments[0]);
        assert_eq!(format!("{kept:?}"), "Opening(..)");
    }

    #[test]
    fn a_rule_that_does_not_hold_cannot_be_proven() {
        let (commitments, openings) = committed(&[5, 12, 2]);
        let within = |most| {
            vec![
                Statement::at_most(0, most),
                Statement::at_most(1, most),
                Statement::at_most(2, most),
            ]
        };
        // The second payment is above 10: there is no proof to make.
        assert_eq!(prove(b"a", &openings, &within(10)), Err(Error::False(1)));
        // A proof of what is true does not pass for what is not: for
        // another rule, other commitments, or another purpose.
        let proof = prove(b"a", &openings, &within(12)).unwrap();
        assert_eq!(verify(b"a", &commitments, &within(12), &proof), Ok(()));
        assert_eq!(
            verify(b"a", &commitments, &within(10), &proof),
            Err(Error::Invalid)
        );
        assert_eq!(
            verify(b"b", &commitments, &within(12), &proof),
            Err(Error::Invalid)
        );
        let (others, _) = committed(&[5, 12, 2]);
        assert_eq!(
            verify(b"a", &others, &within(12), &proof),
            Err(Error::Invalid)
        );
        let mut swapped = commitments.clone();
        swapped.swap(0, 1);
        assert_eq!(
            verify(b"a", &swapped, &within(12), &proof),
            Err(Error::Invalid)
        );
        // A rule about a number that is not there.
        assert_eq!(
            prove(b"a", &openings, &[Statement::at_most(3, 1)]),
            Err(Error::NoSuchValue(0))
        );
    }

    #[test]
    fn a_number_that_is_not_a_whole_amount_cannot_be_slipped_in() {
        // A cheat opens a commitment to "minus five" (the group's order
        // less five), so that 20 less it would look like a small number.
        let cheat = Opening {
            value: 0,
            blinding: Scalar::random(&mut OsRng),
        };
        let gens = PedersenGens::default();
        let minus_five = gens.commit(-Scalar::from(5u64), cheat.blinding);
        let forged = Commitment(minus_five.compress().to_bytes());
        // With an honest proof about another number, the forged one does
        // not verify: every amount is shown to be below 2⁶⁴ itself.
        let (_, honest) = committed(&[3]);
        let rule = [Statement::at_most(0, 20)];
        let proof = prove(b"a", &honest, &rule).unwrap();
        assert_eq!(verify(b"a", &[forged], &rule, &proof), Err(Error::Invalid));
    }

    #[test]
    fn rules_with_several_numbers_and_ratios_are_proven() {
        // 400 calls, 6 failed: at most one in fifty failed, and at least
        // one in a hundred.
        let (commitments, openings) = committed(&[400, 6]);
        let rules = vec![
            Statement {
                terms: vec![(1, 50), (0, -1)],
                bound: 0,
            },
            Statement {
                terms: vec![(0, 1), (1, -100)],
                bound: 0,
            },
        ];
        let proof = prove(b"r", &openings, &rules).unwrap();
        assert_eq!(verify(b"r", &commitments, &rules, &proof), Ok(()));
        // At most one in a hundred is not true.
        let strict = [Statement {
            terms: vec![(1, 100), (0, -1)],
            bound: 0,
        }];
        assert_eq!(prove(b"r", &openings, &strict), Err(Error::False(0)));
    }

    #[test]
    fn many_numbers_are_proven_in_several_proofs() {
        let values: Vec<u64> = (0..70).collect();
        let (commitments, openings) = committed(&values);
        let rules: Vec<Statement> = (0..70).map(|at| Statement::at_most(at, 69)).collect();
        let proof = prove(b"m", &openings, &rules).unwrap();
        assert_eq!(proof.ranges.len(), 3);
        assert_eq!(verify(b"m", &commitments, &rules, &proof), Ok(()));
        let mut cut = proof.clone();
        cut.ranges.pop();
        assert_eq!(
            verify(b"m", &commitments, &rules, &cut),
            Err(Error::Invalid)
        );
    }

    #[test]
    fn a_name_is_shown_to_be_on_a_list_without_showing_which() {
        let list: Vec<Name> = ["acme-ltd", "birch", "cedar", "dune"]
            .iter()
            .map(|n| name(n.as_bytes()))
            .collect();
        let (commitment, opening) = commit_name(&name(b"cedar"));
        let proof = prove_member(b"p", &opening, &list).unwrap();
        assert_eq!(verify_member(b"p", &commitment, &list, &proof), Ok(()));
        // The list's order is not part of what is proven.
        let mut shuffled = list.clone();
        shuffled.reverse();
        assert_eq!(verify_member(b"p", &commitment, &shuffled, &proof), Ok(()));
        // Not for another list, another commitment or another purpose.
        assert_eq!(
            verify_member(b"p", &commitment, &list[..3], &proof),
            Err(Error::Invalid)
        );
        let other = [
            name(b"acme-ltd"),
            name(b"birch"),
            name(b"cedar"),
            name(b"elm"),
        ];
        assert_eq!(
            verify_member(b"p", &commitment, &other, &proof),
            Err(Error::Invalid)
        );
        assert_eq!(
            verify_member(b"q", &commitment, &list, &proof),
            Err(Error::Invalid)
        );
        let (elsewhere, _) = commit_name(&name(b"cedar"));
        assert_eq!(
            verify_member(b"p", &elsewhere, &list, &proof),
            Err(Error::Invalid)
        );
        // A name that is not on the list has no proof, and a proof altered
        // does not hold.
        let (_, mallory) = commit_name(&name(b"mallory"));
        assert_eq!(prove_member(b"p", &mallory, &list), Err(Error::NotAMember));
        let mut altered = proof.clone();
        altered.parts.swap(0, 1);
        assert_eq!(
            verify_member(b"p", &commitment, &list, &altered),
            Err(Error::Invalid)
        );
        // A list of one name: the commitment is to that name.
        let only = [name(b"cedar")];
        let proof = prove_member(b"p", &opening, &only).unwrap();
        assert_eq!(verify_member(b"p", &commitment, &only, &proof), Ok(()));
    }
}
