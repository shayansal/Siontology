//! AMD's certificate chain for SEV-SNP: its root key (ARK) for each
//! generation of EPYC, which signs that generation's signing key (ASK),
//! which signs each chip's endorsement key (VCEK), which signs the chip's
//! reports. The roots and signing keys are built in, as AMD publishes them
//! (taken from the `virtee/sev` crate, Apache-2.0); a VCEK comes with the
//! report, from the host or AMD's key distribution service.
//!
//! Every signature is checked by aws-lc-rs: RSA-PSS with SHA-384 for the
//! ARK and ASK, ECDSA P-384 with SHA-384 for the VCEK's.

use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P384_SHA384_FIXED, RSA_PSS_2048_8192_SHA384};
use x509_cert::der::asn1::ObjectIdentifier;
use x509_cert::der::{Decode, Encode};
use x509_cert::Certificate;

use crate::report::{Report, SigningKey, ECDSA_P384_SHA384};
use crate::Error;

/// A generation of EPYC processors, with a chain of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Generation {
    /// EPYC 7003.
    Milan,
    /// EPYC 9004 and 8004.
    Genoa,
    /// EPYC 9005.
    Turin,
}

impl Generation {
    /// Every generation this knows.
    pub const ALL: [Generation; 3] = [Generation::Milan, Generation::Genoa, Generation::Turin];

    /// Its name, as AMD's key distribution service writes it.
    pub fn name(self) -> &'static str {
        match self {
            Generation::Milan => "Milan",
            Generation::Genoa => "Genoa",
            Generation::Turin => "Turin",
        }
    }

    fn ark_pem(self) -> &'static str {
        match self {
            Generation::Milan => include_str!("amd/milan-ark.pem"),
            Generation::Genoa => include_str!("amd/genoa-ark.pem"),
            Generation::Turin => include_str!("amd/turin-ark.pem"),
        }
    }

    fn ask_pem(self) -> &'static str {
        match self {
            Generation::Milan => include_str!("amd/milan-ask.pem"),
            Generation::Genoa => include_str!("amd/genoa-ask.pem"),
            Generation::Turin => include_str!("amd/turin-ask.pem"),
        }
    }
}

/// The DER of the one certificate in `pem`.
pub fn pem_to_der(pem: &str) -> Result<Vec<u8>, Error> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END CERTIFICATE-----"))
        .collect();
    if body.is_empty() {
        return Err(Error::Malformed("no certificate in the PEM".into()));
    }
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|e| Error::Malformed(format!("the PEM's base64: {e}")))
}

/// A certificate, as what is checked of it.
#[derive(Clone, Debug)]
struct Cert {
    der: Vec<u8>,
    tbs: Vec<u8>,
    signature: Vec<u8>,
    subject: Vec<u8>,
    issuer: Vec<u8>,
    public_key: Vec<u8>,
    extensions: Vec<(ObjectIdentifier, Vec<u8>)>,
}

fn read(der: &[u8]) -> Result<Cert, Error> {
    let bad = |what: &str, e: x509_cert::der::Error| Error::Malformed(format!("{what}: {e}"));
    let cert = Certificate::from_der(der).map_err(|e| bad("a certificate", e))?;
    let tbs = &cert.tbs_certificate;
    Ok(Cert {
        der: der.to_vec(),
        tbs: tbs.to_der().map_err(|e| bad("its signed part", e))?,
        signature: cert.signature.raw_bytes().to_vec(),
        subject: tbs.subject.to_der().map_err(|e| bad("its subject", e))?,
        issuer: tbs.issuer.to_der().map_err(|e| bad("its issuer", e))?,
        public_key: tbs
            .subject_public_key_info
            .subject_public_key
            .raw_bytes()
            .to_vec(),
        extensions: tbs
            .extensions
            .iter()
            .flatten()
            .map(|e| (e.extn_id, e.extn_value.as_bytes().to_vec()))
            .collect(),
    })
}

/// Checks that `issuer` signed `cert`, with RSA-PSS and SHA-384, and named
/// itself as its issuer.
fn signed_by(cert: &Cert, issuer: &Cert, what: &str) -> Result<(), Error> {
    if cert.issuer != issuer.subject {
        return Err(Error::Chain(format!("{what} names another issuer")));
    }
    UnparsedPublicKey::new(&RSA_PSS_2048_8192_SHA384, &issuer.public_key)
        .verify(&cert.tbs, &cert.signature)
        .map_err(|_| Error::Chain(format!("{what}'s signature does not check")))
}

/// AMD's extension naming the chip a VCEK is for.
const HW_ID: &str = "1.3.6.1.4.1.3704.1.4";

/// AMD's extensions naming the versions a VCEK was derived for: the
/// bootloader's, the TEE's, SNP's and the microcode's.
const BL_SPL: &str = "1.3.6.1.4.1.3704.1.3.1";
const TEE_SPL: &str = "1.3.6.1.4.1.3704.1.3.2";
const SNP_SPL: &str = "1.3.6.1.4.1.3704.1.3.3";
const UCODE_SPL: &str = "1.3.6.1.4.1.3704.1.3.8";

fn extension<'a>(cert: &'a Cert, oid: &str) -> Option<&'a [u8]> {
    let oid = ObjectIdentifier::new(oid).ok()?;
    cert.extensions
        .iter()
        .find(|(id, _)| *id == oid)
        .map(|(_, value)| value.as_slice())
}

/// A small DER INTEGER's value.
fn der_integer(bytes: &[u8]) -> Option<u64> {
    let (tag, rest) = bytes.split_first()?;
    let (len, value) = rest.split_first()?;
    if *tag != 0x02 || usize::from(*len) != value.len() || value.is_empty() || value.len() > 9 {
        return None;
    }
    value.iter().try_fold(0u64, |acc, b| {
        acc.checked_mul(256).map(|a| a + u64::from(*b))
    })
}

/// The chain that checks: the generation's root, signed by itself, its
/// signing key, signed by the root, and the VCEK, signed by the signing
/// key; the VCEK's public key.
fn vcek_key(vcek_der: &[u8]) -> Result<(Generation, Cert), Error> {
    let vcek = read(vcek_der)?;
    let mut last = None;
    for generation in Generation::ALL {
        let ark = read(&pem_to_der(generation.ark_pem())?)?;
        let ask = read(&pem_to_der(generation.ask_pem())?)?;
        if vcek.issuer != ask.subject {
            continue;
        }
        let checked = signed_by(&ark, &ark, "AMD's root")
            .and_then(|()| signed_by(&ask, &ark, "AMD's signing key"))
            .and_then(|()| signed_by(&vcek, &ask, "the chip's key"));
        match checked {
            Ok(()) => return Ok((generation, vcek)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        Error::Chain("the chip's key was issued by none of AMD's signing keys".into())
    }))
}

/// What a report that checks says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endorsed {
    /// The chip's generation.
    pub generation: Generation,
    /// The chip's identifier, as its key's certificate names it.
    pub chip_id: Vec<u8>,
    /// The versions the chip's key was derived for: the bootloader's, the
    /// TEE's, SNP's and the microcode's.
    pub tcb: [u64; 4],
}

/// Checks `report` against the chip key `vcek_der`, and that key against
/// AMD's chain: the report signed by the key, with ECDSA P-384 and
/// SHA-384, by a VCEK (a cloud provider's VLEK is not taken), for the chip
/// the report names (unless the report leaves its identifier out) and for
/// the firmware versions it was made with.
pub fn verify(report: &Report, vcek_der: &[u8]) -> Result<Endorsed, Error> {
    if report.signature_algo() != ECDSA_P384_SHA384 {
        return Err(Error::Report(format!(
            "the report names signature algorithm {}, not ECDSA P-384",
            report.signature_algo()
        )));
    }
    if report.signing_key() != SigningKey::Vcek {
        return Err(Error::Report(
            "the report is signed by another key than the chip's own (VCEK)".into(),
        ));
    }
    let (generation, vcek) = vcek_key(vcek_der)?;
    UnparsedPublicKey::new(&ECDSA_P384_SHA384_FIXED, &vcek.public_key)
        .verify(report.signed(), &report.signature_fixed()?)
        .map_err(|_| Error::Report("the report's signature does not check".into()))?;
    let chip_id = extension(&vcek, HW_ID)
        .map(|value| match value {
            // An OCTET STRING around the 64 bytes, or the bytes alone.
            [0x04, 0x40, rest @ ..] if rest.len() == 64 => rest.to_vec(),
            other => other.to_vec(),
        })
        .ok_or_else(|| Error::Chain("the chip's key names no chip".into()))?;
    if !report.chip_id_masked() && chip_id != report.chip_id() {
        return Err(Error::Report(
            "the report names another chip than its key".into(),
        ));
    }
    let spl = |oid| {
        extension(&vcek, oid)
            .and_then(der_integer)
            .ok_or_else(|| Error::Chain(format!("the chip's key lacks {oid}")))
    };
    let tcb = [spl(BL_SPL)?, spl(TEE_SPL)?, spl(SNP_SPL)?, spl(UCODE_SPL)?];
    // Milan's and Genoa's reports keep the versions as bytes 0, 1, 6 and 7
    // of the reported TCB; Turin's lay them out otherwise, and are checked
    // by the key's signature alone.
    if generation != Generation::Turin {
        let reported = report.reported_tcb().to_le_bytes();
        let said = [reported[0], reported[1], reported[6], reported[7]].map(u64::from);
        if said != tcb {
            return Err(Error::Report(format!(
                "the report was made with firmware {said:?}, its key is for {tcb:?}"
            )));
        }
    }
    let _ = vcek.der;
    Ok(Endorsed {
        generation,
        chip_id,
        tcb,
    })
}

/// Where AMD's key distribution service gives the VCEK for a report's chip
/// and firmware versions (Milan's and Genoa's layout of them).
pub fn vcek_url(generation: Generation, report: &Report) -> String {
    let tcb = report.reported_tcb().to_le_bytes();
    format!(
        "https://kdsintf.amd.com/vcek/v1/{}/{}?blSPL={}&teeSPL={}&snpSPL={}&ucodeSPL={}",
        generation.name(),
        hex::encode(report.chip_id()),
        tcb[0],
        tcb[1],
        tcb[6],
        tcb[7]
    )
}
