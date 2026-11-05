//! Attestation, checked: a real SEV-SNP report of an EPYC Milan chip and
//! its key (from the `virtee/sev` crate's tests, Apache-2.0) against AMD's
//! chain, built in; and the binding, the policy and the measurement, on a
//! simulated platform.

use sion_enclave::report::{Report, POLICY_DEBUG};
use sion_enclave::simulated::SimulatedPlatform;
use sion_enclave::{amd, tsm, verify, Binding, Error, Evidence, Expectation, Generation, Platform};

fn milan_report() -> Vec<u8> {
    hex::decode(include_str!("vectors/report_milan.hex").trim()).unwrap()
}

const MILAN_VCEK: &[u8] = include_bytes!("vectors/vcek_milan.der");
const TURIN_VCEK: &[u8] = include_bytes!("vectors/vcek_turin.der");

#[test]
fn a_real_milan_report_checks_against_amds_chain() {
    let report = Report::parse(&milan_report()).unwrap();
    let endorsed = amd::verify(&report, MILAN_VCEK).unwrap();
    assert_eq!(endorsed.generation, Generation::Milan);
    assert_eq!(endorsed.chip_id.len(), 64);
    assert_eq!(endorsed.chip_id, report.chip_id());
    // AMD's address for the key names the chip and its firmware.
    let url = amd::vcek_url(Generation::Milan, &report);
    assert!(url.starts_with("https://kdsintf.amd.com/vcek/v1/Milan/"));
    assert!(url.contains(&hex::encode(report.chip_id())));
}

#[test]
fn a_report_changed_by_a_bit_does_not_check() {
    let mut bytes = milan_report();
    // A bit of the measurement.
    bytes[0x90] ^= 0x01;
    let report = Report::parse(&bytes).unwrap();
    assert_eq!(
        amd::verify(&report, MILAN_VCEK),
        Err(Error::Report(
            "the report's signature does not check".into()
        ))
    );
    // A bit of the signature.
    let mut bytes = milan_report();
    bytes[0x2A0] ^= 0x01;
    let report = Report::parse(&bytes).unwrap();
    assert!(amd::verify(&report, MILAN_VCEK).is_err());
}

#[test]
fn a_chip_key_not_issued_by_amd_or_of_another_chip_is_not_taken() {
    let report = Report::parse(&milan_report()).unwrap();
    // The key changed by a byte inside what AMD signed.
    let mut forged = MILAN_VCEK.to_vec();
    forged[40] ^= 0xff;
    assert!(amd::verify(&report, &forged).is_err());
    // A genuine key, of another chip and generation.
    let other = amd::verify(&report, TURIN_VCEK).unwrap_err();
    assert!(
        matches!(other, Error::Report(_) | Error::Chain(_)),
        "{other:?}"
    );
    // Not a certificate.
    assert!(matches!(
        amd::verify(&report, b"not a certificate"),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_report_is_read_only_whole() {
    assert!(Report::parse(&[0u8; 100]).is_err());
    let mut old = milan_report();
    old[0..4].copy_from_slice(&1u32.to_le_bytes());
    assert!(Report::parse(&old).is_err());
}

fn binding(expires_at: u64) -> Binding {
    Binding {
        kem_public_key: vec![7u8; 1184],
        signing_public_key: vec![9u8; 1312],
        agent: [1u8; 32],
        terms: [2u8; 32],
        expires_at,
    }
}

const MEASUREMENT: [u8; 48] = [0xAB; 48];

#[test]
fn a_simulated_enclave_is_taken_only_from_its_named_platform_and_checks_what_it_binds() {
    let platform = SimulatedPlatform::generate().unwrap();
    let attestation = platform.attest(binding(500), MEASUREMENT).unwrap();
    let mut expected = Expectation {
        measurements: vec![MEASUREMENT],
        simulated_platforms: Vec::new(),
        now: 100,
    };
    // Not named: not taken.
    assert!(matches!(
        verify(&attestation, &expected),
        Err(Error::Unsupported(_))
    ));
    expected.simulated_platforms.push(platform.public_key());
    let verified = verify(&attestation, &expected).unwrap();
    assert_eq!(verified.platform, Platform::Simulated);
    assert_eq!(verified.measurement, MEASUREMENT);
    assert_eq!(verified.binding, binding(500));
    assert!(verified.platform.describe().contains("simulated"));

    // Other keys than it bound: refused.
    let mut swapped = attestation.clone();
    swapped.binding.kem_public_key = vec![8u8; 1184];
    assert!(matches!(
        verify(&swapped, &expected),
        Err(Error::Binding(_))
    ));
    // Code nobody trusts.
    let other = platform.attest(binding(500), [0xCD; 48]).unwrap();
    assert!(matches!(verify(&other, &expected), Err(Error::Report(_))));
    // Expired.
    expected.now = 501;
    assert!(matches!(
        verify(&attestation, &expected),
        Err(Error::Binding(_))
    ));
    expected.now = 100;
    // Another platform's key named for this report: its signature fails.
    let impostor = SimulatedPlatform::generate().unwrap();
    let mut claimed = attestation.clone();
    if let Evidence::Simulated { platform_key, .. } = &mut claimed.evidence {
        *platform_key = impostor.public_key();
    }
    expected.simulated_platforms.push(impostor.public_key());
    assert!(matches!(verify(&claimed, &expected), Err(Error::Report(_))));
    // Kept and read back, the platform is the same.
    let kept = SimulatedPlatform::from_pkcs8(&platform.pkcs8().unwrap()).unwrap();
    assert_eq!(kept.public_key(), platform.public_key());
}

#[test]
fn a_debuggable_enclave_proves_nothing() {
    let platform = SimulatedPlatform::generate().unwrap();
    let attestation = platform.attest(binding(500), MEASUREMENT).unwrap();
    let Evidence::Simulated { report, .. } = &attestation.evidence else {
        unreachable!()
    };
    let parsed = Report::parse(report).unwrap();
    assert!(!parsed.debug_allowed());
    // The same report launched debuggable: refused before anything else,
    // whoever signed it.
    let bytes = sion_enclave::report::compose(
        &binding(500).report_data(),
        &MEASUREMENT,
        (1 << 17) | POLICY_DEBUG,
        &[0u8; 64],
    );
    assert!(Report::parse(&bytes).unwrap().debug_allowed());
    let debuggable = sion_enclave::Attestation {
        evidence: Evidence::Simulated {
            report: bytes,
            platform_key: platform.public_key(),
        },
        binding: binding(500),
    };
    let expected = Expectation {
        measurements: vec![MEASUREMENT],
        simulated_platforms: vec![platform.public_key()],
        now: 100,
    };
    assert_eq!(
        verify(&debuggable, &expected),
        Err(Error::Report(
            "the enclave was launched debuggable: its host can read it".into()
        ))
    );
}

#[test]
fn the_hosts_table_of_certificates_gives_the_chip_key() {
    // One entry for the VCEK (its GUID as UEFI keeps it), then the end.
    let guid = hex::decode("8d75da6364e66445adc5f4b93be8accd").unwrap();
    let mut table = Vec::new();
    table.extend_from_slice(&guid);
    table.extend_from_slice(&48u32.to_le_bytes());
    table.extend_from_slice(&(MILAN_VCEK.len() as u32).to_le_bytes());
    table.extend_from_slice(&[0u8; 24]);
    table.extend_from_slice(MILAN_VCEK);
    assert_eq!(tsm::vcek_in(&table).as_deref(), Some(MILAN_VCEK));
    assert_eq!(tsm::vcek_in(&[0u8; 24]), None);
    // Outside a confidential VM there is nothing to ask.
    let missing = tsm::collect_in(std::path::Path::new("/nonexistent/tsm"), &[0u8; 64]);
    assert!(matches!(missing, Err(Error::Unsupported(_))));
}
