//! Content Credentials: a manifest c2pa-rs (0.91.3, through c2pa-python
//! 0.38.1) signed, checked here; and one signed here, checked here and
//! refused when the asset or its claim changes. The fixtures' key and
//! certificates are a throwaway test PKI (`tests/fixtures/c2pa/`), made
//! with OpenSSL for these tests and trusted nowhere else.

use sion_cli::c2pa::{self, Action, Maker, TRAINED_ALGORITHMIC_MEDIA};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/c2pa");

/// A day into the test certificates' validity.
const NOW: u64 = 1_794_040_918 + 86_400;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

#[test]
fn a_manifest_c2pa_rs_signed_is_checked_here() {
    let asset = fixture("pic.png");
    let store = fixture("c2pa-rs.c2pa");
    let report = c2pa::verify(&asset, &store, &[], NOW).unwrap();
    assert_eq!(report.title, "pic.png");
    assert_eq!(report.generator, "reference");
    assert!(!report.trusted, "no root was given");
    assert!(
        report.signer.contains("Siontology Test Agent"),
        "{}",
        report.signer
    );
    assert_eq!(
        report.actions,
        vec![(
            "c2pa.created".to_string(),
            Some(TRAINED_ALGORITHMIC_MEDIA.to_string())
        )]
    );
    assert!(report
        .assertions
        .contains(&"c2pa.thumbnail.claim".to_string()));
    let roots = c2pa::certificates(&String::from_utf8(fixture("root.pem")).unwrap()).unwrap();
    assert!(c2pa::verify(&asset, &store, &roots, NOW).unwrap().trusted);
    let mut changed = asset.clone();
    changed[40] ^= 1;
    let refused = c2pa::verify(&changed, &store, &[], NOW).unwrap_err();
    assert!(refused.to_string().contains("data hash"), "{refused}");
}

#[test]
fn what_the_agent_makes_is_signed_as_made_by_it() {
    let asset = fixture("pic.png");
    let chain = c2pa::certificates(&String::from_utf8(fixture("chain.pem")).unwrap()).unwrap();
    let key =
        c2pa::private_key(&String::from_utf8(fixture("test-only-leaf.key")).unwrap()).unwrap();
    let maker = Maker {
        did: "did:sion:devnet:0707".into(),
        name: "lead".into(),
        models: vec!["anthropic:claude-opus-5-5".into()],
    };
    let store = c2pa::sign(&asset, "pic.png", &maker, Action::Created, &chain, &key).unwrap();
    // For checking it with another implementation: SION_C2PA_DUMP=<file>.
    if let Ok(out) = std::env::var("SION_C2PA_DUMP") {
        std::fs::write(out, &store).unwrap();
    }
    let roots = c2pa::certificates(&String::from_utf8(fixture("root.pem")).unwrap()).unwrap();
    let report = c2pa::verify(&asset, &store, &roots, NOW).unwrap();
    assert!(report.trusted);
    assert_eq!(report.agent, Some(maker));
    assert_eq!(report.generator, "Siontology");
    assert_eq!(report.actions[0].0, "c2pa.created");
    // Another asset, or a claim changed by a byte, is refused.
    assert!(c2pa::verify(b"other bytes", &store, &[], NOW).is_err());
    let at = store.windows(7).position(|w| w == b"pic.png").unwrap();
    let mut forged = store.clone();
    forged[at] = b'P';
    let refused = c2pa::verify(&asset, &forged, &[], NOW).unwrap_err();
    assert!(refused.to_string().contains("signature"), "{refused}");
    // A root it does not chain to is no trust.
    let other = c2pa::certificates(&String::from_utf8(fixture("chain.pem")).unwrap()).unwrap();
    assert!(c2pa::verify(&asset, &store, &other[..1], NOW).is_err());
}
