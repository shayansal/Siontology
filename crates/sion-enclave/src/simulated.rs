//! SIMULATED: a platform made of an ECDSA P-384 key on some machine, for
//! tests and devnets where there is no SEV-SNP hardware. Its reports have
//! the form of AMD's, signed by its key instead of a chip's, so that what
//! reads and checks them is the same code; nothing stands behind them, and
//! [`crate::verify`] takes one only from a platform key it is given
//! outright ([`crate::Expectation::simulated_platforms`]).

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P384_SHA384_FIXED,
    ECDSA_P384_SHA384_FIXED_SIGNING,
};

use crate::report::{compose, put_signature, Report};
use crate::{Attestation, Binding, Error, Evidence};

/// A simulated platform: its key.
pub struct SimulatedPlatform {
    key: EcdsaKeyPair,
}

impl std::fmt::Debug for SimulatedPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SimulatedPlatform({})",
            hex::encode(&self.public_key()[..8])
        )
    }
}

impl SimulatedPlatform {
    /// A new one, with a key made here.
    pub fn generate() -> Result<SimulatedPlatform, Error> {
        let key = EcdsaKeyPair::generate(&ECDSA_P384_SHA384_FIXED_SIGNING)
            .map_err(|_| Error::Collect("no key could be made".into()))?;
        Ok(SimulatedPlatform { key })
    }

    /// One whose key is kept as PKCS#8 (from [`SimulatedPlatform::pkcs8`]).
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<SimulatedPlatform, Error> {
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_FIXED_SIGNING, pkcs8)
            .map_err(|_| Error::Malformed("not a P-384 key in PKCS#8".into()))?;
        Ok(SimulatedPlatform { key })
    }

    /// Its key as PKCS#8, to keep.
    pub fn pkcs8(&self) -> Result<Vec<u8>, Error> {
        self.key
            .to_pkcs8v1()
            .map(|document| document.as_ref().to_vec())
            .map_err(|_| Error::Malformed("the key could not be written".into()))
    }

    /// Its public key: an uncompressed P-384 point.
    pub fn public_key(&self) -> Vec<u8> {
        self.key.public_key().as_ref().to_vec()
    }

    /// The attestation of an enclave launched with `measurement` binding
    /// `binding`.
    pub fn attest(&self, binding: Binding, measurement: [u8; 48]) -> Result<Attestation, Error> {
        let mut report = compose(&binding.report_data(), &measurement, 1 << 17, &[0u8; 64]);
        let signature = self
            .key
            .sign(&SystemRandom::new(), &report[..crate::report::SIGNED_SIZE])
            .map_err(|_| Error::Collect("the report could not be signed".into()))?;
        put_signature(&mut report, signature.as_ref());
        Ok(Attestation {
            evidence: Evidence::Simulated {
                report,
                platform_key: self.public_key(),
            },
            binding,
        })
    }
}

/// Checks that the simulated platform with `key` signed `report`.
pub fn check(report: &Report, key: &[u8]) -> Result<(), Error> {
    UnparsedPublicKey::new(&ECDSA_P384_SHA384_FIXED, key)
        .verify(report.signed(), &report.signature_fixed()?)
        .map_err(|_| Error::Report("the simulated report's signature does not check".into()))
}
