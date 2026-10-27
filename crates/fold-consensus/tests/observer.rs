//! A validator outside the committee observes: it never proposes or votes,
//! but it commits exactly what the members commit.

use std::collections::VecDeque;

use fold_consensus::{
    insecure_sim_keyring, Action, Authenticator, Committee, Core, CoreConfig, Event,
    InsecureSimAuthenticator, InsecureSimVerifier, Message, TimerKind,
};
use fold_core::ids::{Epoch, ShardId, ValidatorId};

type SimCore = Core<InsecureSimAuthenticator, InsecureSimVerifier>;

#[test]
fn an_observer_commits_what_members_commit_without_speaking() {
    let (keys, verifier) = insecure_sim_keyring(5, 11);
    let members: Vec<ValidatorId> = keys[..4].iter().map(|k| k.validator()).collect();
    let committee = Committee::equal_weight(Epoch(3), ShardId(0), members.clone()).unwrap();
    let mut cores: Vec<SimCore> = keys
        .iter()
        .map(|k| {
            Core::new(
                committee.clone(),
                k.clone(),
                verifier.clone(),
                CoreConfig::default(),
            )
        })
        .collect();
    assert!(cores[..4].iter().all(|c| c.is_member()));
    assert!(!cores[4].is_member());
    let ids: Vec<ValidatorId> = keys.iter().map(|k| k.validator()).collect();

    // A synchronous network: broadcasts reach every other node (observer
    // included), sends reach their target; timers fire in rounds.
    let mut queue: VecDeque<(usize, Event)> = VecDeque::new();
    for i in 0..5 {
        queue.push_back((i, Event::Start));
    }
    for (n, item) in (0..20u8).enumerate() {
        queue.push_back((n % 4, Event::Submit(vec![item; 8])));
    }
    let mut spoke = Vec::new();
    let mut committed: Vec<Vec<Vec<u8>>> = vec![Vec::new(); 5];
    let mut steps = 0;
    while steps < 200_000 {
        let Some((at, event)) = queue.pop_front() else {
            // Idle: fire round timers so leaders that missed are skipped.
            for (i, core) in cores.iter().enumerate() {
                queue.push_back((i, Event::Timer(TimerKind::Round(core.round()))));
                queue.push_back((i, Event::Timer(TimerKind::Resend)));
            }
            if committed[..4].iter().all(|c| c.len() >= 20) {
                break;
            }
            steps += 1;
            continue;
        };
        steps += 1;
        for action in cores[at].handle(event) {
            match action {
                Action::Broadcast(message) => {
                    if at == 4 {
                        spoke.push(message.clone());
                    }
                    for to in 0..5 {
                        if to != at {
                            queue.push_back((
                                to,
                                Event::Message {
                                    from: ids[at],
                                    message: message.clone(),
                                },
                            ));
                        }
                    }
                }
                Action::Send { to, message } => {
                    if at == 4 && !matches!(message, Message::SyncRequest(_)) {
                        spoke.push(message.clone());
                    }
                    let index = ids.iter().position(|v| *v == to).unwrap();
                    queue.push_back((
                        index,
                        Event::Message {
                            from: ids[at],
                            message,
                        },
                    ));
                }
                Action::Commit(sub_dag) => committed[at].extend(sub_dag.items),
                Action::SetTimer { .. } | Action::Evidence(_) => {}
            }
        }
    }
    assert!(committed[0].len() >= 20, "members committed every item");
    for node in 1..5 {
        let common = committed[node].len().min(committed[0].len());
        assert_eq!(
            committed[node][..common],
            committed[0][..common],
            "node {node} commits the same sequence"
        );
    }
    assert!(
        committed[4].len() >= 20,
        "the observer followed the commits"
    );
    assert!(spoke.is_empty(), "the observer never proposed or voted");
}
