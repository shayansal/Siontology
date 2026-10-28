## What and why

<!-- What this change does and why. Link the issue it closes. -->

## How it was tested

<!-- The commands you ran and what they showed. -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
      and `cargo test --workspace` pass; if the node, the CLI or a demo
      changed, `cargo test --workspace -- --include-ignored` passes too.
- [ ] `./scripts/check-independence.sh` passes.
- [ ] If the state transition changes: the specification changed first (a
      new version and an entry in its **Changes** section), and both proof
      stacks and the differential tests follow it.
- [ ] If something is now built, simplified or simulated:
      `docs/implementation-status.md` says so, with any new `impl:` atom.
- [ ] The documentation is updated (the book in `docs/book`, command help)
      and claims no more than `docs/implementation-status.md`;
      `mdbook build docs/book` passes.
- [ ] Every commit is signed off (`git commit -s`; see CONTRIBUTING.md).
