# sion-http

`sion-http` is the outbound HTTP client that Siontology's tools use when they
reach the internet: storage providers fetching a deal's data, and agents
calling model providers or fetching pages. Every request is bounded and
guarded. TLS offers the hybrid post-quantum key exchange X25519MLKEM768
first, reads stop at a limit the caller sets, redirects are off unless
asked for, and names that resolve to private or local addresses are
refused, so a URL written by a stranger cannot reach the machine or its
network. The crate also holds the small protocol helpers those services need:
a WebSocket client and the digests and ciphers that webhook protocols name.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (a support library; the status file has no row for it
and names it where outbound HTTP is used, for example storage deals and the
channel bridges, see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `lib.rs` (crate root) | `Client`, `Options`, `Response`, `Fetched`, `Stream`, `HttpError`; the address check `is_public`; SHA-256, random bytes and RSA-SHA256 sign and verify helpers |
| `ws` | WebSocket connections (RFC 6455), as a client over `Client::connect` or as a server over an accepted connection, for stand-ins in tests |
| `digest` | SHA-1 and HMAC over SHA-1 and SHA-256, with a constant-time comparison (`same_bytes`), plus `base64` |
| `cipher` | AES in CBC and ECB modes and Ed25519, by aws-lc-rs, for protocols that name them (WeCom, WeChat, Feishu, QQ) |

## Main types and functions

- [`Client`](src/lib.rs): makes requests; `Client::new(Options)`, then `get`, `post_json`, `send` and `request`.
- [`Options`](src/lib.rs): connect timeout, overall timeout, whether redirects are followed (`redirects`) and whether private addresses are allowed (`allow_private`).
- [`Response`](src/lib.rs): headers, media type and a body that is read with a byte limit (`read_all`).
- [`Fetched`](src/lib.rs): the result of a `get`.
- [`HttpError`](src/lib.rs): refused, transport, status, too large, configuration, or a closed WebSocket.
- [`is_public`](src/lib.rs): whether an address is one a request may reach.
- [`ws`](src/ws.rs): the WebSocket client and server ends.
- [`hmac_sha256`](src/digest.rs), [`hmac_sha1`](src/digest.rs) and [`same_bytes`](src/digest.rs): webhook signatures. SHA-1 is here only where a protocol fixes it.

## How it fits

Depends on: no workspace crate. Outside crates: `ureq`, `rustls` with
`aws-lc-rs`, `rustls-pki-types` and `webpki-roots`. Roots are Mozilla's plus
any PEM file that `SSL_CERT_FILE` names. Used by: `sion-agent` and
`sion-cli`.

The crate does not follow redirects or reach local addresses unless the
caller says so. Behind a proxy
(`HTTPS_PROXY`, honouring `NO_PROXY`) the proxy resolves names, so only
literal addresses and local names (`localhost`, `*.local`, `*.internal`) can
be refused.

The crate is 2,476 lines of Rust under `src/` at the time of writing.

## Tests

23 unit tests (counted with `grep -c "#\[test\]"`): 12 in `src/lib.rs`, 5 in
`src/ws.rs`, 3 in `src/digest.rs` and 3 in `src/cipher.rs`. There is no
`tests/` directory. Two tests are ignored because they need the network:
one in `src/lib.rs` and one in `src/ws.rs` (the status file records that
the WebSocket client reached Discord's real gateway and a public echo
server through a proxy in an ignored test).

```text
cargo test -p sion-http
cargo test -p sion-http -- --ignored
```

## Read more

- [Crates](../../docs/book/src/architecture/crates.md)
- [Tools for agents](../../docs/book/src/agent/tools.md)
- [Storage deals](../../docs/book/src/concepts/storage.md)
- [Implementation status: storage deals](../../docs/implementation-status.md)
