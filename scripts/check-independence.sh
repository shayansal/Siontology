#!/usr/bin/env bash
# Checks that proof stack B (crates/sion-spec) does not call the reference
# executor's logic, as docs/spec/state-transition.md §0 requires.
#
# This is a lint, not a proof: it searches the library sources (unit-test
# modules excluded, since they cross-check helpers against the reference on
# purpose) for paths and method calls §0 forbids. Review still matters.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/crates/sion-spec/src"

python3 - "$src" <<'EOF'
import pathlib
import re
import sys

src = pathlib.Path(sys.argv[1])

# Forbidden by spec §0: the reference executor's logic.
forbidden = [
    (r"sion_exec::execute::(?!\{|Deferred\b|Event\b|ExecStatus\b|TxEffects\b|bond_atom\b|registered_atom\b|bond_owner\b)\w+",
     "sion_exec::execute logic (only the types and bond_atom, registered_atom, bond_owner are allowed)"),
    (r"\bexecute_transaction\b", "sion_exec::execute::execute_transaction"),
    (r"\bexecute_pulse(_traced)?\b", "sion_exec::pulse::execute_pulse"),
    (r"\bapply_deferred\b", "sion_exec::pulse::apply_deferred"),
    (r"\bcanonical_order\b", "sion_exec::pulse::canonical_order"),
    (r"sion_exec::stateless\b|\breplay_chunk\b|\bWitnessStore\b", "sion_exec::stateless"),
    (r"sion_exec::wallet\b|\bWallet\b", "sion_exec::wallet"),
    (r"sion_exec::sim\b", "sion_exec::sim"),
    (r"sion_exec::genesis\b|\bgenesis\s*\(", "sion_exec::genesis"),
    (r"\.spend\s*\(|Allotment::spend\b", "Allotment::spend"),
    (r"\.attenuates\s*\(|Caveats::attenuates\b", "Caveats::attenuates"),
    (r"\.escrow\s*\(|OrderTerms::escrow\b", "OrderTerms::escrow"),
    (r"\.clearing_batch\s*\(|Market::clearing_batch\b", "Market::clearing_batch"),
    (r"\.batch_index\s*\(|Market::batch_index\b", "Market::batch_index"),
    (r"\.offender\s*\(|Equivocation::offender\b", "Equivocation::offender"),
    (r"\.(lane_members|lane_committee|lane_committees|consensus_committee|epoch_end|rotation)\s*\(|ValidatorSet::\w+\s*\(",
     "ValidatorSet methods"),
    (r"\bposted_intents\b", "sion_exec::pulse::posted_intents"),
    (r"sion_exec::stream::(?!\{|Stream\b|Checkpoints\b|Jump\b|MAX_STREAM_LENGTH\b|MAX_SETTLE_STEPS\b|STREAM_STEP_FEE\b|STEP_DOMAIN\b|MAX_CHECKPOINTS\b|MAX_CHECKPOINT_DEPTH\b|CHECKPOINT_TAG\b)\w+",
     "sion_exec::stream helpers (only the Stream, Checkpoints and Jump types and the §5.31 constants are allowed)"),
    (r"\bPayerChain\b|\bextends\s*\(|\bcheckpoint_leaf\b|\bverify_jump\b|\bjump_hashes\b|\bcheckpoint_count\b|\bwalk\s*\(",
     "sion_exec::stream helpers"),
    (r"\bresult_formula\b|\bworker_bond\b|TaskResult::payload\b",
     "sion_exec::task helpers (result_formula, worker_bond, TaskResult::payload)"),
    (r"sion_exec::delegation::(?!\{|DelegationBook\b|Pool\b|Join\b|Delegation\b|DelegationState\b|REWARD_SCALE\b|MIN_DELEGATION\b|COMMISSION_CEILING_PERMILLE\b|COMMISSION_STEP_PERMILLE\b|DELEGATION_SLASH_PERMILLE\b|CAP_FLOOR_PERMILLE\b)\w+",
     "sion_exec::delegation helpers (only the types and the §5.45 constants are allowed)"),
    (r"\bmul_div\b|\bcap_permille\b|\bwithin_cap\b|\bStanding\b|\bresolve\s*\(|\.(shares|value_of|shares_for|bonded|earning_value|owed)\s*\(|Pool::\w+\s*\(",
     "sion_exec::delegation helpers (mul_div, cap_permille, within_cap, resolve, Pool methods)"),
    (r"sion_exec::storage::(?!\{|Deal\b|DealState\b|Custody\b|CustodyProof\b|LeafProof\b|MAX_DEAL_LEAVES\b|MAX_LEAF_BYTES\b|MAX_PATH\b|MAX_CHALLENGES\b|MAX_ROUNDS\b|MIN_ROUND_LENGTH\b|MAX_ROUND_LENGTH\b|MAX_LOCATOR\b|MAX_PROOF_LEAVES\b|ABANDON_ROUNDS\b|SEED_DOMAIN\b|CHALLENGE_DOMAIN\b)\w+",
     "sion_exec::storage helpers (only the types and the §5.50 constants are allowed)"),
    (r"\bchallenge_index\b|\bround_seed\b|\bcurrent_round\b|\bDataTree\b|\bsplit_leaves\b|Deal::end\b|\.end\s*\(\s*\)|\.round_start\s*\(",
     "sion_exec::storage helpers (challenge_index, round_seed, current_round, DataTree, split_leaves, Deal::end, Custody::round_start)"),
    (r"sion_exec::beacons::(?!\{|BeaconRecord\b|BEACON_INTERVAL\b|BEACON_SLOTS\b)\w+",
     "sion_exec::beacons helpers (only the type and the constants are allowed)"),
    (r"\bslot_of\b|\bnext_boundary\b",
     "sion_exec::beacons helpers (slot_of, next_boundary)"),
    (r"sion_exec::auth\b|\bfind_authority\b|\bcheck_authorization\b|\bcheck_membership\b|\bsigned_message\b|\bis_keyless\b",
     "sion_exec::auth and charter::is_keyless (the reference executor's authorization)"),
]

# Items of a module that are named in a `use` list or a path: the
# sion_exec::tx and sion_exec::charter logic stack B must write itself
# (spec §0), wherever an import puts them.
forbidden_items = {
    "tx": {"fee_floor", "required_fee", "sign_batch"},
    "charter": {"is_keyless"},
}

# The sion_exec::pulse items the library may use: type definitions only
# (the §7 helpers are reimplemented and cross-checked in unit tests).
allowed_pulse = {"ChunkStatement", "ChunkTrace", "Witness", "PulseInput"}

def library_text(path):
    """The file's code without comments or its trailing #[cfg(test)] module."""
    text = path.read_text()
    cut = text.find("#[cfg(test)]")
    if cut >= 0:
        text = text[:cut]
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())

problems = []
for path in sorted(src.rglob("*.rs")):
    text = library_text(path)
    for number, line in enumerate(text.splitlines(), 1):
        for pattern, what in forbidden:
            if re.search(pattern, line):
                problems.append(f"{path.relative_to(src.parent.parent.parent)}:{number}: uses {what}: {line.strip()}")
    for module, names in forbidden_items.items():
        for match in re.finditer(r"sion_exec::" + module + r"::(\{[^}]*\}|\w+)", text):
            for name in re.findall(r"\w+", match.group(1)):
                if name in names:
                    number = text[: match.start()].count("\n") + 1
                    problems.append(f"{path.relative_to(src.parent.parent.parent)}:{number}: uses sion_exec::{module}::{name}")
    for match in re.finditer(r"sion_exec::pulse::(\{[^}]*\}|\w+)", text):
        names = re.findall(r"\w+", match.group(1))
        for name in names:
            if name not in allowed_pulse:
                number = text[: match.start()].count("\n") + 1
                problems.append(f"{path.relative_to(src.parent.parent.parent)}:{number}: uses sion_exec::pulse::{name}")

if problems:
    print("Proof stack B calls the reference executor:")
    for problem in problems:
        print("  " + problem)
    sys.exit(1)
print(f"Proof stack B is independent of the reference executor ({len(list(src.rglob('*.rs')))} files checked).")
EOF
