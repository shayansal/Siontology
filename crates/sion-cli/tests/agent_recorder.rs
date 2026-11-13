//! A flight recorder on a devnet: a device's events are recorded with no
//! network, fixed by commits, and handed to an investigator as an extract
//! that is checked against the log the ledger holds. A stretch with no
//! record and a clock that stepped back show; a withheld record is there
//! as a digest; a bound is proven of sealed readings without showing
//! them; a changed, shortened or stale extract does not check; and what
//! cannot be recorded as it came is recorded as a fault.

use std::sync::{Arc, Mutex};

use serde_json::json;
use sion_cli::recorder::{self, Event, Kind, Reading, RecorderCommand, Seen};
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::AssetId;
use sion_core::Amount;
use sion_node::{Devnet, DevnetConfig};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    home
}

fn devnet() -> (Arc<Devnet>, SharedLedger) {
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            // Commits are confirmed from the node's events.
            history: 100_000,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    (devnet, ledger)
}

const T0: u64 = 1_700_000_000_000;

fn event(at_ms: u64, kind: Kind, channel: &str, name: &str, value: serde_json::Value) -> Event {
    Event {
        t_ms: T0 + at_ms,
        kind,
        channel: channel.into(),
        name: name.into(),
        value: (!value.is_null()).then_some(value),
    }
}

fn beat(at_ms: u64) -> Event {
    event(at_ms, Kind::Heartbeat, "", "", json!(null))
}

#[test]
fn a_devices_events_are_fixed_by_the_ledger_and_checked_by_an_investigator() {
    let _one = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let _home = home("agent-recorder");
    let (_devnet, ledger) = devnet();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(100))
        .unwrap();
    recorder::run(
        Some((&mut session, ledger.as_ref())),
        RecorderCommand::Open {
            name: "arm-7".into(),
            wallet: "alice".into(),
            heartbeat_ms: 1_000,
            sealed: vec!["arm/joint-3".into()],
        },
    )
    .unwrap();
    let mut settings = recorder::load("arm-7").unwrap();
    assert!(recorder::load("arm-8").is_err());

    // The device works, and reports as it goes: nothing here needs the
    // ledger.
    let first = vec![
        beat(0),
        event(200, Kind::Command, "arm", "move_to", json!("bin-4")),
        event(400, Kind::Sensor, "arm/joint-3", "torque_mnm", json!(4_200)),
        beat(1_000),
        event(
            1_300,
            Kind::Decision,
            "planner",
            "slow_down",
            json!("a person is in the zone"),
        ),
        event(
            1_500,
            Kind::Sensor,
            "arm/joint-3",
            "torque_mnm",
            json!(4_500),
        ),
        event(
            1_600,
            Kind::Sensor,
            "arm/base",
            "temperature_mc",
            json!(41_300),
        ),
        beat(2_000),
    ];
    assert_eq!(recorder::record(&settings, &first).unwrap(), (0, 8, 0));
    let read = recorder::records(&settings).unwrap();
    assert_eq!(read.len(), 8);
    assert_eq!(
        read[1].line(),
        "1 t=1700000000.200 command arm move_to = \"bin-4\""
    );
    // A sealed reading is in the log as a commitment and nothing else.
    assert!(matches!(read[2].reading, Reading::Sealed(_)));
    assert_eq!(
        read[2].line(),
        "2 t=1700000000.400 sensor arm/joint-3 torque_mnm = (sealed)"
    );
    assert_eq!(read[6].reading, Reading::Number(41_300));

    let shown = recorder::lines(&settings).unwrap().join("\n");
    assert!(
        shown.contains("8 record(s) in log")
            && shown.contains("0 fixed by a commit, 8 only on this machine"),
        "{shown}"
    );
    // Nothing is committed: there is nothing to extract.
    assert!(recorder::extract(ledger.as_ref(), &settings, T0, T0 + 60_000, &[]).is_err());

    // Online, the log is committed: these eight are fixed.
    let one = recorder::commit(&mut session, ledger.as_ref(), &mut settings, false).unwrap();
    assert_eq!(one.size, 8);

    // Out of reach, the device goes on. For fourteen seconds it records
    // nothing, then a fault; later its clock steps back.
    let second = vec![
        beat(3_000),
        beat(17_000),
        event(
            17_100,
            Kind::Fault,
            "arm/joint-3",
            "overcurrent",
            json!("drive tripped"),
        ),
        event(
            17_200,
            Kind::Sensor,
            "arm/joint-3",
            "torque_mnm",
            json!(4_900),
        ),
        beat(18_000),
        beat(16_500),
        // Text that looks like a sealed reading is text.
        event(
            16_600,
            Kind::Decision,
            "planner",
            "note",
            json!("sealed:00"),
        ),
        beat(17_500),
    ];
    assert_eq!(recorder::record(&settings, &second).unwrap(), (8, 8, 0));
    let shown = recorder::lines(&settings).unwrap().join("\n");
    assert!(
        shown.contains("8 fixed by a commit, 8 only on this machine"),
        "{shown}"
    );
    assert!(
        shown.contains(&format!("records 0 to 7 were fixed at Pulse {}", one.pulse)),
        "{shown}"
    );
    assert!(
        shown.contains("records 8 to 15 are in no commit"),
        "{shown}"
    );
    assert!(
        shown.contains(
            "silence: nothing recorded for 14.000 s, from t=1700000003.000 (record 8) to \
             t=1700000017.000 (record 9)"
        ),
        "{shown}"
    );
    assert!(
        shown.contains("the device's clock stepped back 1.500 s at record 13"),
        "{shown}"
    );
    let text = recorder::records(&settings).unwrap();
    assert_eq!(text[14].reading, Reading::Text(" sealed:00".into()));

    // What is not committed is not in an extract: the window reaches only
    // the first eight.
    let early = recorder::extract(ledger.as_ref(), &settings, T0, T0 + 60_000, &[]).unwrap();
    assert_eq!((early.first, early.last(), early.size), (0, 7, 8));
    let (lines, holds) = recorder::check(ledger.as_ref(), &early).unwrap();
    assert!(holds, "{lines:?}");

    let two = recorder::commit(&mut session, ledger.as_ref(), &mut settings, false).unwrap();
    assert_eq!(two.size, 16);
    assert!(two.pulse > one.pulse);
    // The log moved on: the earlier extract is of a head the ledger no
    // longer holds.
    let (lines, holds) = recorder::check(ledger.as_ref(), &early).unwrap();
    assert!(
        !holds && lines[0].contains("have it made again"),
        "{lines:?}"
    );

    // The extract for the investigator: the whole run, the planner's
    // records withheld.
    let mut extract = recorder::extract(
        ledger.as_ref(),
        &settings,
        T0,
        T0 + 60_000,
        &["planner".to_string()],
    )
    .unwrap();
    assert_eq!((extract.first, extract.last()), (0, 15));
    let seen = extract.seen();
    assert_eq!(
        seen.iter()
            .filter(|s| matches!(s, Seen::Withheld(_)))
            .count(),
        2
    );
    assert!(matches!(seen[4], Seen::Withheld(4)));
    // A narrower window is the run from its first record to its last.
    let narrow =
        recorder::extract(ledger.as_ref(), &settings, T0 + 17_000, T0 + 17_300, &[]).unwrap();
    assert_eq!((narrow.first, narrow.last()), (9, 11));

    // A bound on the sealed torque readings, proven without them. There
    // is no proof of a bound one of them breaks, nor of readings that are
    // not sealed.
    let joint = ("arm/joint-3", "torque_mnm");
    assert!(recorder::prove(&settings, &mut extract, joint.0, joint.1, Some(4_500), None).is_err());
    assert!(recorder::prove(
        &settings,
        &mut extract,
        "arm/base",
        "temperature_mc",
        Some(1),
        None
    )
    .is_err());
    assert_eq!(
        recorder::prove(&settings, &mut extract, joint.0, joint.1, Some(5_000), None).unwrap(),
        3
    );

    let (lines, holds) = recorder::check(ledger.as_ref(), &extract).unwrap();
    let text = lines.join("\n");
    assert!(holds, "{text}");
    assert!(
        text.contains("holds: the extract is of the log as the ledger holds it: 16 record(s)"),
        "{text}"
    );
    assert!(
        text.contains(
            "holds: records 0 to 15, all 16 of them, are in that log in this order (14 shown, 2 \
             withheld)"
        ),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "records 0 to 7 were fixed at Pulse {} (the ledger's events confirm it)",
            one.pulse
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "records 8 to 15 were fixed at Pulse {}",
            two.pulse
        )),
        "{text}"
    );
    assert!(
        text.contains("silence: nothing recorded for 14.000 s"),
        "{text}"
    );
    assert!(
        text.contains(
            "holds, proven without the readings: every one of the 3 sealed reading(s) of \
             arm/joint-3 torque_mnm in these records is at most 5000"
        ),
        "{text}"
    );
    assert!(text.contains("(2 record(s) are withheld"), "{text}");
    // The file holds no reading that was sealed or withheld.
    let file = serde_json::to_string(&extract).unwrap();
    assert!(!file.contains("4900") && !file.contains("a person is in the zone"));
    assert!(file.contains("drive tripped"));

    // What an investigator would catch: a record changed, one left out,
    // a withheld one swapped, a bound that was not the one proven.
    let mut changed = extract.clone();
    let atom = changed.rows[1].atom.as_mut().unwrap();
    atom.statement = sion_memory::Statement::claim(
        "arm",
        "command:move_to",
        sion_memory::Value::Text("bin-9".into()),
    );
    let (lines, holds) = recorder::check(ledger.as_ref(), &changed).unwrap();
    assert!(
        !holds
            && lines
                .join("\n")
                .contains("1 of the 16 record(s) are not at their place")
    );
    let mut shortened = extract.clone();
    shortened.rows.remove(10);
    assert!(!recorder::check(ledger.as_ref(), &shortened).unwrap().1);
    let mut swapped = extract.clone();
    swapped.rows[4].id = swapped.rows[14].id;
    assert!(!recorder::check(ledger.as_ref(), &swapped).unwrap().1);
    let mut tighter = extract.clone();
    tighter.bounds[0].at_most = Some(4_000);
    let (lines, holds) = recorder::check(ledger.as_ref(), &tighter).unwrap();
    assert!(!holds && lines.join("\n").contains("does not hold for these records"));
    // A proof made for one run of records is not a proof for another.
    let mut moved = narrow.clone();
    moved.bounds = extract.bounds.clone();
    assert!(!recorder::check(ledger.as_ref(), &moved).unwrap().1);

    // After the incident the log is sealed: it takes no more, and the
    // extract stays checkable.
    let sealed = recorder::commit(&mut session, ledger.as_ref(), &mut settings, true).unwrap();
    assert_eq!(sealed.size, 16);
    let (lines, holds) = recorder::check(ledger.as_ref(), &extract).unwrap();
    assert!(
        holds && lines[0].contains("and sealed: it takes no more"),
        "{lines:?}"
    );

    // A recorder drops nothing. What it cannot record as it came (a line
    // that is no event, a sealed reading below zero, an event with no
    // channel) is recorded as a fault that says why, and the rest of the
    // batch with it. A sealed reading's value is not in the fault.
    let stream = format!(
        "{}\nnot json at all\n\n{}\n{}\n{}\n",
        json!({"t_ms": T0 + 19_000, "kind": "heartbeat"}),
        json!({"t_ms": T0 + 19_100, "kind": "sensor", "channel": "arm/joint-3",
               "name": "torque_mnm", "value": -77}),
        json!({"t_ms": T0 + 19_200, "kind": "sensor", "channel": "", "name": "x", "value": 1}),
        json!({"t_ms": T0 + 19_300, "kind": "sensor", "channel": "arm/base",
               "name": "temperature_mc", "value": 41_900}),
    );
    let read = recorder::events(stream.as_bytes()).unwrap();
    assert_eq!(read.len(), 5);
    assert_eq!(recorder::record(&settings, &read).unwrap(), (16, 5, 3));
    let after = recorder::records(&settings).unwrap();
    assert_eq!(after.len(), 21);
    assert!(after[17]
        .line()
        .starts_with("17 t=1700000019.000 fault recorder unreadable = "));
    assert!(matches!(&after[18].reading, Reading::Text(why)
        if why.contains("sealed reading") && !why.contains("77")));
    assert_eq!(after[20].reading, Reading::Number(41_900));
    // The sealed log takes no more commits: these stay the device's.
    assert!(recorder::commit(&mut session, ledger.as_ref(), &mut settings, false).is_err());
    let _ = SharedLedger::clone;
}
