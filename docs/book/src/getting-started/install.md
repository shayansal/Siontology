# Install from source

Siontology has no binary releases yet. You build it from source.

## Requirements

- **Rust 1.80 or later** (the workspace's `rust-version`). Install it with
  [rustup](https://rustup.rs).
- Git.
- No system libraries. The cryptography (ML-DSA, SLH-DSA, ML-KEM,
  ChaCha20-Poly1305, SHA3) is pure Rust, and the HTTP client and server
  need no TLS library.

The code builds and runs on Linux. Other platforms supported by Rust should
work but have not been tested.

## Build

```bash
git clone https://github.com/ColdAI-org/siontology
cd siontology
cargo build --release -p sion-cli
./target/release/sion --version
```

```text
sion 0.1.0
```

A cold release build took about ten minutes on the 2-core machine used for
this book. The workspace profiles optimize dependencies even in debug
builds, because post-quantum signing is slow without optimization, and
release builds keep integer overflow checks on.

To put `sion` on your `PATH`:

```bash
cargo install --locked --path crates/sion-cli
```

The rest of this book calls the binary `sion`.

## Where things live

| What | Where |
|---|---|
| Wallet seeds | `$SION_HOME/wallets/<name>.seed`, by default `~/.sion/wallets` |
| The node the CLI talks to | `--node` or `$SION_NODE`, by default `http://127.0.0.1:8730` |
| The chain identity you pin | `--chain` or `$SION_CHAIN` |
| A devnet's data | the directory you pass to `sion devnet --data-dir`; without it, nothing is written to disk |

## Run the tests

```bash
cargo test --workspace
```

The suite includes property tests for the Assurance Algebra, the
linear-programming certificates and the state machine, a deterministic
consensus simulator with adversarial network phases, scenario tests for
every transaction kind, differential tests of the two proof stacks,
end-to-end devnet tests with dual post-quantum signatures, and every demo.
See `CONTRIBUTING.md` for the full checklist.

## Next

- [Run the demos](demos.md): eight scripted scenarios, no setup.
- [Run a devnet](devnet.md) and use it from another terminal.
