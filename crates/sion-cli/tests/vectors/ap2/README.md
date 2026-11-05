# AP2 test vectors

The `.txt` files are the "Encoded Token" examples of the Agent Payments
Protocol v0.2 specification, `docs/ap2/checkout_mandate.md` and
`docs/ap2/payment_mandate.md` of
[google-agentic-commerce/AP2](https://github.com/google-agentic-commerce/AP2)
at commit `e1ea56db72a6385bce3e5c1112b3a56ce60acb43` (Apache License 2.0),
copied unchanged:

- `open-checkout.txt`, `closed-checkout.txt`, `checkout-chain.txt`
- `open-payment.txt`, `closed-payment.txt`, `payment-chain.txt`

Their roots are signed by an agent provider key the specification does not
publish, so `tests/ap2_vectors.rs` checks what can be checked without it:
each closing hop's signature under the key its open mandate names, the
hash bindings, and the chain's joining.
