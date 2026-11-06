//! ACP, the Agentic Commerce Protocol (API version 2026-04-17): selling an
//! agent's work to other agents through a checkout they already speak, and
//! buying from merchants that serve it.
//!
//! [ACP](https://github.com/agentic-commerce-protocol/agentic-commerce-protocol)
//! is the REST contract a merchant serves so that an agent can buy from it.
//! What is served here, each as ACP specifies it:
//!
//! - **Checkout** ([`merchant`]): `POST /checkout_sessions`, update, `GET`,
//!   `…/complete`, `…/cancel`, and `/.well-known/acp.json`. The merchant is
//!   the system of record: every answer is the whole session.
//! - **Carts** ([`merchant`]): `POST /carts`, `GET`, `PUT` (replace),
//!   `…/cancel`.
//! - **Orders and webhooks** ([`merchant`], [`webhooks`]): an order in ACP's
//!   shape for every verified payment, `order_create` and `order_update`
//!   events sent to the agent's platform, signed with `Merchant-Signature`,
//!   retried with backoff, delivered in order; fulfillment and refunds
//!   through the operator's admin API.
//! - **Request signing** ([`signing`]): the `Signature` and `Timestamp`
//!   headers, checked per client (ES256, Ed25519, or HMAC-SHA256).
//! - **Delegated payments** ([`psp`], [`card`]): a PSP's `POST
//!   /agentic_commerce/delegate_payment` vault, and the merchant's
//!   `dev.acp.tokenized.card` handler, charging tokens through such a PSP or
//!   through Stripe's shared payment tokens.
//! - **3-D Secure** ([`three_ds`]): the Delegate Authentication API, its
//!   issuer simulated, and the merchant's `authentication_metadata` and
//!   `authentication_result` handling.
//! - **Product feeds** ([`feeds`]): the agent-hosted Feed API (`POST
//!   /feeds`, `GET`, `GET`/`PATCH …/products`), and publishing a catalog to
//!   one.
//! - **Buying** ([`buyer`]): on the ledger, or by card, with or without
//!   3-D Secure.
//!
//! Every request passes the same gate ([`http`]): a bearer token, the
//! `API-Version` header, a signature from clients that sign, and ACP's
//! idempotency (the same key and body answered from what was stored, another
//! body refused, a request still running answered `409` with `Retry-After`,
//! `5xx` answers not kept, keys kept 24 hours). Errors are ACP's flat shape,
//! amounts integer minor units.
//!
//! **Payment on the ledger.** ACP's payment handlers are open by design: a
//! handler is a named specification that the session declares, and an
//! instrument that names it. This merchant declares [`HANDLER`]
//! (`org.siontology.ledger`): its `config.requirements` is an x402 quote
//! ([`crate::x402`]) for the session's total, and the instrument's credential
//! is a Siontology transaction signed against it and not submitted.
//! Completing the session settles that transaction with the x402
//! [`Facilitator`](crate::x402::Facilitator): submitted, executed, its
//! receipt verified as a light client does against the pinned chain, and
//! only then is the order made. What bounds the agent is its Agent Charter
//! on the ledger, which caps whom it pays and how much.
//!
//! **Payment by card** is the PSP's word: nothing here can check that money
//! moved (trust atom `agent:acp-card`). The PSP served here authorizes with a
//! simulated acquirer ([`psp::SimulatedAcquirer`]) and the 3-D Secure
//! provider with a simulated issuer ([`three_ds::SimulatedAcs`]): no card
//! network is reached. A real deployment charges through Stripe or another
//! ACP PSP.
//!
//! **AP2.** A merchant made [`Merchant::with_ap2`] also takes the Agent
//! Payments Protocol's mandates ([`crate::ap2`]) through an ACP extension,
//! [`AP2_EXTENSION`]: each session carries a checkout JWT this merchant
//! signed, with an audience and a nonce; `…/complete` may carry
//! `ap2.checkout_mandate` and `ap2.payment_mandate`, checked before any
//! money moves (budgets and recurrence against the payments made under the
//! same open mandate, which the merchant keeps); the session then carries
//! the signed receipts.
//!
//! **State** ([`state`]): a merchant's sessions, carts, orders, idempotency
//! keys, AP2 history and webhook outbox are kept in a file and taken back on
//! restart, quotes included; a ledger payment that was settling when it
//! stopped is finished from its receipt.

pub mod buyer;
pub mod card;
pub mod feeds;
pub mod http;
pub mod merchant;
pub mod psp;
pub mod signing;
pub mod state;
pub mod three_ds;
pub mod webhooks;

pub use buyer::{buy, buy_by_card, Buyer};
pub use http::{serve, Request, Response, Service};
pub use merchant::{Catalog, Merchant, Offer, Order};

/// The ACP version served.
pub const API_VERSION: &str = "2026-04-17";

/// The payment handler this merchant declares: stablecoin USD on Siontology.
pub const HANDLER: &str = "org.siontology.ledger";
/// The version of the handler's specification.
pub const HANDLER_VERSION: &str = "2026-11-05";
/// The handler's id within a session.
pub const HANDLER_ID: &str = "sion_ledger";
/// The instrument type the handler takes.
pub const INSTRUMENT: &str = "sion_transaction";
/// Where the handler is specified.
pub const HANDLER_SPEC: &str = "https://siontology.org/docs/agent/commerce.html";

/// The ACP extension that carries AP2 mandates and receipts.
pub const AP2_EXTENSION: &str = "org.siontology.ap2";

/// Micro-USD in a cent: ACP's amounts are cents, the ledger's micro-USD.
pub const MICROS_PER_CENT: u128 = 10_000;
