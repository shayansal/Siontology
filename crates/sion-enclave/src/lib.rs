//! Attestation for agents that run in enclaves.
//!
//! An agent's runtime that runs inside an AMD SEV-SNP confidential VM asks
//! the chip for a report ([`collect`]): the measurement of what the VM was
//! launched with, and 64 bytes of its own choosing, signed by a key only
//! that chip holds, which AMD's chain certifies ([`amd`]). The 64 bytes
//! bind the session's keys ([`Binding`]): the ML-KEM key that whoever uses
//! the agent seals their requests to, and the ML-DSA key it signs its
//! answers and its counts of usage with. Both are made inside the VM and
//! never leave it, so a party that checks an [`Attestation`] ([`verify`])
//! knows that what it seals is read only by code with that measurement, on
//! a genuine chip, in a VM its host cannot debug.
//!
//! What the check rests on: AMD's chain and the chip (a flaw in either, or
//! a key taken from one, undoes it), and the measured code itself, which
//! the checker names by its measurement ([`Expectation::measurements`]).
//! An enclave's measurement says what was launched, not that it is right.
//!
//! [`simulated`] is a platform made of a key on this machine, for tests and
//! devnets: its reports have the same form, but nothing stands behind them,
//! and [`verify`] takes one only from a platform key it is given outright.
//!
//! Intel TDX quotes are collected ([`collect`]) but not yet checked.

pub mod amd;
pub mod report;
pub mod simulated;
pub mod tsm;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

pub use amd::Generation;
pub use report::Report;

/// Why an attestation was not taken.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The evidence is not in the form it should be.
    #[error("malformed: {0}")]
    Malformed(String),
    /// AMD's chain does not check.
    #[error("the certificate chain: {0}")]
    Chain(String),
    /// The report does not check, or says what is not taken.
    #[error("the report: {0}")]
    Report(String),
    /// The report does not bind what it was said to.
    #[error("the binding: {0}")]
    Binding(String),
    /// The platform or the request is not one this handles.
    #[error("{0}")]
    Unsupported(String),
    /// Asking the platform failed.
    #[error("collecting the evidence: {0}")]
    Collect(String),
}

/// What a session binds in its report: its keys, made inside the enclave,
/// and what it runs under.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Binding {
    /// The ML-KEM-768 key requests are sealed to.
    #[serde(with = "hex_bytes")]
    pub kem_public_key: Vec<u8>,
    /// The ML-DSA key the session signs its answers and usage with.
    #[serde(with = "hex_bytes")]
    pub signing_public_key: Vec<u8>,
    /// The agent it runs as.
    pub agent: [u8; 32],
    /// The digest of the terms it serves under (a listing's, say).
    pub terms: [u8; 32],
    /// The Pulse after which the binding is no longer to be relied on.
    pub expires_at: u64,
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        hex::decode(text).map_err(serde::de::Error::custom)
    }
}

/// The domain the binding's digest is taken in.
pub const BINDING_DOMAIN: &[u8] = b"sion/enclave/binding/v1";

impl Binding {
    /// The 64 bytes a report binds it by: SHA-512 of the domain and its
    /// canonical encoding.
    pub fn report_data(&self) -> [u8; 64] {
        let encoded = borsh::to_vec(self).unwrap_or_default();
        let mut context = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA512);
        context.update(&(BINDING_DOMAIN.len() as u32).to_be_bytes());
        context.update(BINDING_DOMAIN);
        context.update(&encoded);
        let digest = context.finish();
        let mut data = [0u8; 64];
        data.copy_from_slice(digest.as_ref());
        data
    }
}

/// The evidence an enclave gives.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Evidence {
    /// An AMD SEV-SNP report, and the certificate of the chip key that
    /// signed it (DER).
    SevSnp {
        /// The report.
        #[serde(with = "hex_bytes")]
        report: Vec<u8>,
        /// The chip's VCEK.
        #[serde(with = "hex_bytes")]
        vcek: Vec<u8>,
    },
    /// SIMULATED: a report of the same form, signed by a key on some
    /// machine ([`simulated`]). Nothing stands behind it.
    Simulated {
        /// The report.
        #[serde(with = "hex_bytes")]
        report: Vec<u8>,
        /// The simulated platform's public key (an uncompressed P-384
        /// point).
        #[serde(with = "hex_bytes")]
        platform_key: Vec<u8>,
    },
}

/// An enclave's evidence and what it binds.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Attestation {
    /// The evidence.
    pub evidence: Evidence,
    /// What its report binds.
    pub binding: Binding,
}

/// The schema of a claim that publishes an attestation.
pub const CLAIM_SCHEMA: &str = "enclave.attestation/v1";

/// What a checker requires of an attestation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expectation {
    /// The launch measurements of the code it trusts; one of them is
    /// required.
    pub measurements: Vec<[u8; 48]>,
    /// The simulated platforms it takes, by their public keys: none unless
    /// given outright (tests, devnets).
    pub simulated_platforms: Vec<Vec<u8>>,
    /// The Pulse it checks at: a binding that expired before it is not
    /// taken.
    pub now: u64,
}

/// What an attestation that checks shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The platform it ran on.
    pub platform: Platform,
    /// What the enclave was launched with.
    pub measurement: [u8; 48],
    /// The keys and terms it binds.
    pub binding: Binding,
}

/// The platform behind an attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// An AMD SEV-SNP chip, its chain checked.
    SevSnp(amd::Endorsed),
    /// SIMULATED: a key on some machine, taken because the checker named it.
    Simulated,
}

impl Platform {
    /// A line saying what it is.
    pub fn describe(&self) -> String {
        match self {
            Platform::SevSnp(endorsed) => format!(
                "AMD SEV-SNP ({}, chip {}…)",
                endorsed.generation.name(),
                hex::encode(&endorsed.chip_id[..8.min(endorsed.chip_id.len())])
            ),
            Platform::Simulated => "a simulated enclave (nothing stands behind it)".into(),
        }
    }
}

/// Checks `attestation` against `expected`: its report signed as its
/// platform requires, binding exactly its keys and terms, launched with a
/// measurement `expected` names, not debuggable by its host, and not
/// expired.
pub fn verify(attestation: &Attestation, expected: &Expectation) -> Result<Verified, Error> {
    let (bytes, platform_key) = match &attestation.evidence {
        Evidence::SevSnp { report, .. } => (report, None),
        Evidence::Simulated {
            report,
            platform_key,
        } => (report, Some(platform_key)),
    };
    let report = Report::parse(bytes)?;
    if report.report_data() != attestation.binding.report_data() {
        return Err(Error::Binding(
            "the report binds other keys or terms than the attestation says".into(),
        ));
    }
    if report.debug_allowed() {
        return Err(Error::Report(
            "the enclave was launched debuggable: its host can read it".into(),
        ));
    }
    if !expected.measurements.contains(&report.measurement()) {
        return Err(Error::Report(format!(
            "the enclave runs code of measurement {}, which is not one trusted",
            hex::encode(report.measurement())
        )));
    }
    if attestation.binding.expires_at < expected.now {
        return Err(Error::Binding(format!(
            "the binding expired at Pulse {}",
            attestation.binding.expires_at
        )));
    }
    let platform = match (&attestation.evidence, platform_key) {
        (Evidence::SevSnp { vcek, .. }, _) => Platform::SevSnp(amd::verify(&report, vcek)?),
        (Evidence::Simulated { .. }, Some(key)) => {
            if !expected.simulated_platforms.contains(key) {
                return Err(Error::Unsupported(
                    "a simulated enclave, which is taken only from a platform named outright"
                        .into(),
                ));
            }
            simulated::check(&report, key)?;
            Platform::Simulated
        }
        (Evidence::Simulated { .. }, None) => unreachable!("a simulated report has its key"),
    };
    Ok(Verified {
        platform,
        measurement: report.measurement(),
        binding: attestation.binding.clone(),
    })
}

/// Asks the platform this runs on for evidence binding `binding`, through
/// the kernel's configfs-tsm interface: an SEV-SNP report, and its chip's
/// key if the host gives it ([`tsm`]); with none, `fetch_vcek` is asked for
/// it, given AMD's address for it (the caller's network).
pub fn collect(
    binding: Binding,
    fetch_vcek: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Attestation, Error> {
    let collected = tsm::collect(&binding.report_data())?;
    match collected.provider.as_str() {
        "sev_guest" => {
            let report = Report::parse(&collected.outblob)?;
            let vcek = match collected.auxblob.as_deref().and_then(tsm::vcek_in) {
                Some(vcek) => vcek,
                None => {
                    // The generation decides AMD's address; the key's own
                    // chain says which it was, once it is fetched.
                    let mut last = String::new();
                    let mut found = None;
                    for generation in Generation::ALL {
                        match fetch_vcek(&amd::vcek_url(generation, &report)) {
                            Ok(der) => {
                                found = Some(der);
                                break;
                            }
                            Err(e) => last = e,
                        }
                    }
                    found.ok_or_else(|| {
                        Error::Collect(format!("the chip's key could not be fetched: {last}"))
                    })?
                }
            };
            Ok(Attestation {
                evidence: Evidence::SevSnp {
                    report: collected.outblob,
                    vcek,
                },
                binding,
            })
        }
        "tdx_guest" => Err(Error::Unsupported(
            "this is an Intel TDX guest: its quotes are collected but not yet checked".into(),
        )),
        other => Err(Error::Unsupported(format!(
            "the platform {other:?} is not one this attests"
        ))),
    }
}
