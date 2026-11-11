# A run, within its limits

`sion agent run` puts an [agent identity](../guides/agents.md) to work: a
language model plans and calls tools, and the runtime runs each call under
rules you set. The ledger is one of its tools, and the agent's
[charter](../guides/charters.md) is the limit no runtime setting can
loosen: a payment outside it is refused by the validators, whatever the
runtime allowed.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

## An agent and its limits

alice registers agent-7: it may pay only bob, at most 10 USD a payment and
50 USD at once, its bucket refilled by 0.05 USD a Pulse, from a budget of
100 USD. The agent's account
pays its transactions' fees, so it gets 1 USD for them:

```text
$ sion agent register alice agent-7 --payees bob --max-payment 10 --burst 50 --rate 0.05 --funding 100
alice signs: Register agent-7 as an agent, funded with 100 USD: only to bob, at most 10 USD per payment, 50 USD at once, refilled by 0.05 USD a Pulse, until Pulse 100000, a receipt claim for every payment, may delegate narrower limits. Fee 0.0011 USD; valid until Pulse 20.
Pulse 2: Done: agent bdbdb742 registered, signing as agent-7; fee 0.0011 USD.
agent bdbdb742f506a53671928c83efc1c57321d70dd0636d84ab873575f9f70f60df
…
$ sion faucet agent-7 1
```

## A run

The script checks the agent's budget, pays bob, then tries two payments
its charter forbids: a tip to carol, and 50 USD to bob. At autonomy L6
the runtime lets every call through, so the ledger is what refuses them:

```text
$ sion agent run agent-7 bdbdb742…60df --task 'Pay bob 5 USD for inference.' --model script:pay.json --autonomy L6
session 4c16d4afda9b2fc7 for agent bdbdb742
  → ledger_balance {}
  ← {"budget_remaining_usd":"100","burst_usd":"50","can_spend_now_usd":"50","expires_at_pulse":100000,"fee_balance_usd":"1","max_per_payment_usd":"10","payees":["bob"],"refill_usd_per_pulse":"0.05","status":"active"}
  → ledger_pay {"amount":"5","purpose":"inference","to":"bob"}
  ← Pulse 7: Done: agent paid 5 USD to bob (receipt claim 60bed29d); fee 0.0012 USD. Transaction ef8072e0….
  → ledger_pay {"amount":"5","purpose":"a tip","to":"carol"}
  ✗ refused by the ledger at Pulse 9: Failed (charter caveat violated: payee not allowed); nothing happened except the fee of 0.0011 USD.
  → ledger_pay {"amount":"50","purpose":"a big order","to":"bob"}
  ✗ refused by the ledger at Pulse 12: Failed (charter caveat violated: payment 50 above per-payment limit 10); nothing happened except the fee of 0.0011 USD.
agent: Paid bob 5 USD for inference. The ledger refused a tip to carol, who is not a payee my charter allows, and a 50 USD order, above my 10 USD limit per payment.
session 4c16d4afda9b2fc7: done
```

A script is a JSON array of steps: `{"call": "<tool>", "input": {…}}`,
`{"calls": [ … ]}` for several calls in one turn, and `{"say": "…"}` to
finish. The agent's tools:

| Tool | Effect | What it does |
|---|---|---|
| `ledger_balance` | read | The budget left, the limit per payment, what the agent's bucket lets it spend now and how fast it refills, the payees allowed, the account's balance for fees, the agent's status |
| `ledger_pay` | spend | Pays from the charter's budget; the receipt records which runtime call made the payment |
| `ledger_transaction` | read | Whether a transaction executed, by digest, to check a payment whose outcome is unknown |
| `read_file`, `write_file`, `list_files` | read, write | Files in the directory `--workspace` gives, and nowhere else |
| `fetch_url` | external | Public web pages, with `--web`; local and private addresses are refused |
| `fetch_paid` | spend | With `--web`: a page that charges through x402 (HTTP 402), its quote paid from the budget under the charter, at most what the call allows, and the page fetched again with the payment ([x402](../guides/x402.md#in-an-agents-own-run-fetch_paid)) |
| `agent_spawn` | spend | With `--delegate`: a subagent with a budget carved from the agent's, working on a task in a session of its own ([Subagents and hired help](teams.md)); those of one turn are spawned together and work at the same time |
| `task_post`, `task_status`, `task_submit`, `task_release` | spend, read, external, external | With `--delegate`: hire another agent through a Task Charter (with a check its verifiers run, if you fix one), follow it, deliver one you were hired for (with the attestations it needs), or give one up that cannot be done |
| `shell`, `skill_run` | write, or external with the network | With `--shell`: commands, and a skill's own scripts, in a sandbox that writes only the workspace ([Commands, scripts and MCP servers](tools.md)) |
| `<server>_<tool>` | as reviewed | With `--mcp <server>`: the reviewed tools of an MCP server |
| `storage_keep`, `storage_deals`, `storage_close` | spend, read, write | With `--storage`: keep a workspace file with storage providers, follow and close the deals |
| `share_file` | read | With a workspace: a file of it sent to the person the agent answers, after its reply, where the channel takes files ([Pictures and files](channels.md#pictures-and-files)); a run in the terminal names it |
| `image_generate` | write | With `--image-model openai:<model>` on the profile, and a workspace: a picture made through OpenAI's Images API (`OPENAI_API_KEY`, `OPENAI_BASE_URL`), put in the workspace's `images/` and shown to the agent |
| `ha_states`, `ha_state`, `ha_services` | read | With `--home-assistant` on the profile: Home Assistant's entities and their states, one entity's state, and the services it offers, through its REST API (`HOME_ASSISTANT_URL`, `HOME_ASSISTANT_TOKEN`); what they say is from outside, as a web page is (`agent:home-assistant`) |
| `ha_call` | external | With `--home-assistant`: a Home Assistant service called (a light turned on, a thermostat set), as the token's user may |
