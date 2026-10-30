# Trust model

Every assumption the system relies on without proof is named, as an atom in
some trust formula (rule 5 of `AGENTS.md`). This chapter lists what the
architecture assumes, what this implementation adds, and what a light
client trusts when `sion verify` prints a check mark.

## What the architecture assumes

| Property | Comes from | Assumes |
|---|---|---|
| Validity of the state | The World Proof | The proof systems are sound (two independent stacks: either suffices), the specification is right, SHA3-256 is collision resistant |
| Ordering, finality, liveness | Committees | Fewer than a third of each committee's stake is Byzantine; partial synchrony for liveness |
| Censorship resistance | Forced inclusion, rotation, sampling | At least one honest proposer; a committee that censors can be replaced |
| Data availability | Sampling and erasure coding | Enough clients sample; the data root commits to a correct encoding (bound by the World Proof) |
| Signatures | ML-DSA-44 and SLH-DSA together | At least one of the two families is unbroken |
| Knowledge (claims) | The Assurance Algebra | Nothing: a claim states the atoms it rests on, and each consumer prices them under its own policy |

The trust formula of a dual proof, from the architecture:

```text
specification ∧ sha3-256 ∧ ((stack A ∧ circuit A) ∨ (stack B ∧ circuit B))
```

## What this implementation adds

These atoms are part of the formula of anything the devnet reports, on top
of the architecture's own. The list is copied from the repository's
[implementation status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md).

| Atom | What it means |
|---|---|
| `impl:replay-proofs` | World Proofs are checked by re-execution, so "proof finality" costs a verifier as much as executing; succinctness is assumed, not delivered |
| `impl:shared-primitives` | Both proof stacks use the same hashing, Merkle, signature, Assurance Algebra and clearing-certificate code; a bug there is common-mode |
| `impl:devnet-keys` | Devnet validator keys are derived from the chain name; anyone can derive them. A devnet secures nothing of value |
| `impl:in-process-network` | `sion devnet` only: validators share a machine and a process; network faults are exercised only through the in-process fault injector and the consensus simulator. A network of `sion node` processes does not carry this atom |
| `impl:reliable-handover` | At an epoch boundary each validator starts a new consensus session; a validator that has not reached the boundary finishes the old session from messages already sent, which the in-process network always delivers. A real network needs a drain period: on a network of processes, a validator that misses old-session messages at a boundary can stall |
| `net:transport-ml-dsa` | Networks: transport authentication rests on ML-DSA-44 alone (each node's network key, presented as an RFC 7250 raw public key and pinned at both ends). A forger could impersonate a peer at the transport layer (delay, drop, eclipse), not sign consensus messages, which carry dual signatures |
| `net:aws-lc` | Networks: the TLS provider is aws-lc-rs, C code outside the workspace's audit scope (the workspace's own post-quantum code is pure Rust) |
| `net:raw-public-keys` | Networks: TLS authenticates nodes with RFC 7250 raw public keys (an ML-DSA-44 SubjectPublicKeyInfo, signed for in the handshake's CertificateVerify), not the self-signed ML-DSA X.509 certificates the design proposed, and with no channel-binding fallback. Both ends pin the exact key, so no certificate path is validated; the handshake relies on rustls's raw-public-key support, newer and less used than its certificate path |
| `net:static-peers` | Networks: peers come from a genesis file and a peer file of signed records, read when a node starts, not from the state. A validator that joins by staking is reachable only by nodes started with its record in their peer file |
| `net:partial-synchrony` | Networks: liveness needs message delays to become bounded after some time (assumed by the architecture; named because a real network now exercises it) |
| `net:gateway-trusts-node` | Networks: a gateway running alone learns committees and answers state queries from one node it trusts. A lying node can make it ask the wrong validators or show wrong balances; it cannot get a transaction certified that a committee did not approve |
| `net:no-rejoin` | Networks: a validator that stops cannot rejoin. Restarting in a session it signed in needs the consensus signing log, and catching up to a later session needs Pulse sync; neither is built, so it refuses to start rather than risk equivocation. Each stopped validator is a permanent fault; more than a third of the stake stopped halts the chain |
| `impl:slashed-seat` | A slashed validator keeps its seat, and its weight in quorums, until the epoch ends |
| `impl:active-floor` | Validators can leave until one remains: nothing keeps the active set large enough to tolerate a faulty member, so the operators of the other validators can leave a single validator forming every quorum |
| `impl:evidence-scope` | Equivocation evidence is not bound to this chain: a validator's keys must not be used on another network |
| `impl:lock-handover` | Certificates are not drained when a lane committee changes: certificate finality for owned transactions holds within one committee's term (an owner who double-signs could get the second spend certified by a fully replaced committee) |
| `impl:hash-chain-beacon` | The beacon is a hash-chain commit-reveal; the last validator to reveal can bias it by withholding (at the cost of being seen) |
| `impl:public-amounts` | Amounts and balances are visible to everyone |
| `impl:memory-index` | The ledger stores a memory log's index root without checking it: a memory query proof is complete relative to that root, and nothing on the ledger yet acts on evidence that an index leaves an atom out |
| `impl:unaudited-pq-crates` | The FIPS 203/204/205 implementations are unaudited pure-Rust crates |
| `storage:no-replication` | A storage proof shows that the provider could produce the challenged leaves during the round, not that it keeps a copy of its own: it could fetch them from another copy (or from the client) when challenged. Two deals with two providers are not proof of two copies |
| `storage:raw-proofs` | Every storage proof carries its challenged leaves and paths on the ledger, about 17 to 22 KiB a deal a round with 4 challenges. Proofs are not aggregated across deals or rounds, so ledger bandwidth bounds how many deals a network can keep |
| `storage:no-retrieval` | Getting data back is not part of the protocol: a provider proves custody, and returning the data to its client is up to them |
| `app:agent-search` | The agents the app finds for the device come from the node's history of events, unproven, and only the Pulses the node still keeps: a node can leave out an agent the device backs, and the Agents tab would not show it. The standing and budget of every agent it shows are proven |
| `app:first-use-pin` | The app pins the chain identity of the first node it verifies (trust on first use) unless its user pins one first. A first use against a lying node pins the liar's chain |
| `app:plain-key` | The app keeps the device's key unencrypted in its directory (mode 0600 on Unix), not in the platform's keystore. Anything that can read the directory can spend |
| `app:unproven-balances` | The balances the app shows come from the node unproven; its own receipts and the Pulses it verifies are proven |
| `agent:local-policy` | An agent runtime's autonomy level, approvals, grants and taint are enforced by the runtime on the machine it runs on. Only the agent's charter (payees, limits, budget, expiry, suspension) is enforced by the ledger |
| `agent:provider-sees-context` | A model provider receives the conversation it is asked to continue, tool results included |
| `agent:plain-sessions` | Agent sessions, with their conversations and journals, are stored unencrypted under `$SION_HOME/agents/sessions` |
| `agent:local-skill-checks` | The runtime on the agent's machine checks a skill's bundle against its release when installing it, and takes a skill out of use when its release is no longer active at the start of each run. A session already running keeps the skill until it ends, and installed files changed on disk afterwards are not checked again |
| `agent:eval-runs` | Evaluation runs are not proven. A verifier's attestation that a version passes an evaluation set rests on its bond and the bounty's adjudicator; with a model that varies between runs, another run can differ. The ledger refuses an attestation by the version's author (specification v1.15), but not one by an account the author controls under another name: such an attestation is worth its own bond |
| `agent:channel-token` | Whoever holds the machine's channel token can start and continue the sessions of every profile on the machine, approvals included; whoever holds a profile's own token, that profile's, and approvals only with a token made for them; always within each profile's autonomy and the agent's charter. Tokens are kept as their hashes. The channel speaks plain HTTP |
| `agent:gateway` | An agent gateway sees, and could change, the messages and answers it passes; it cannot act as an agent on the ledger, holding no key. It routes a profile's messages only to a host that proved the key of the agent its tokens name for the profile, and checks the agent's standing at most five seconds old, from the node it trusts: a message can reach an agent suspended within those seconds. It speaks plain HTTP unless put behind TLS |
| `agent:messenger-identity` | Slack, Discord, Matrix, Signal and Telegram say who wrote a message: a bridge's approvers are who the messenger says they are, and anyone who can write in an allowed chat talks to the profile |
| `agent:telegram-chats` | A Telegram bot trusts Telegram for who writes in the chats it answers; in a group chat, every member speaks for its owner, approvals included unless `--approvers` names who may decide |
| `agent:pairing` | An approved pairing code lets in the chat it was sent to, on that channel and profile, until revoked: the operator trusts that whoever gave them the code is the person they mean to let in, and the messenger for who writes in that chat afterwards (`agent:messenger-identity`). Codes are kept in a file only the operator's user can read |
| `agent:webhooks` | A webhook (WhatsApp, LINE, SMS through Twilio) is taken as its service's when it carries the service's signature with the secret the operator gave. The signatures name no time, so a webhook copied on its way could be sent again later and is answered again once its id has left the last thousand remembered; the bridge listens on plain HTTP, and TLS is the operator's proxy's |
| `agent:voice` | The services `--hear` and `--speak` name receive the voice notes and the replies spoken; the profile acts on what the transcription says was said, within its autonomy and the agent's charter. A `command:` provider runs as the operator's user, outside any sandbox |
| `agent:local-scheduler` | Scheduled jobs run only while `sion agent scheduler` runs on the machine (or a system timer calls it); a missed run happens once, late |
| `agent:keyless-signer` | A keyless subagent is contained by the runtime that holds its nearest keyed ancestor's keys: the ledger lets only that ancestor sign for it, and holds it to its own caveats, but a subagent that could make that runtime sign something else as the ancestor would act within the ancestor's caveats, not its own. The runtime signs only for the session's own tool calls |
| `agent:sandbox` | Commands an agent runs (its shell, its skills' scripts, MCP servers started in a sandbox) are contained only as far as the kernel's namespaces (bubblewrap) or the container runtime (Docker) hold: an escape gives them the runtime's user. Bubblewrap bounds how long a command runs and the size of each process, not how many processes it starts; Docker bounds those too. A container started by a runtime that is killed runs until its command ends |
| `agent:sandbox-network` | With the network allowed, a sandboxed command reaches what that network reaches. Bubblewrap's `host` network is the machine's own, unfiltered: local services and a cloud's metadata address included. The `declared` network gives commands none, and skills' scripts only what their skills declare, through the egress proxy (`agent:declared-hosts`). A Docker network restricts only as its operator set it up: the runtime refuses Docker's `host` and `bridge` networks by name and cannot check the rest |
| `agent:shell-repeats` | Shell commands, skills' scripts and file writes are not journaled: a command the model gives again runs again, so with the network a repeated command can repeat an action outside the machine |
| `agent:workspace-content` | What an agent reads from its own workspace (files, and the output of commands without the network) does not taint its session, even content it saved there from outside in an earlier session |
| `agent:mcp-unsandboxed` | An MCP server added without a sandbox runs as the operator's user, with the environment variables its configuration names: it can do whatever that user can |
| `agent:mcp-effects` | What an MCP tool's call does is what its server does. The runtime knows it only from the annotations reviewed with the tool, or from the effect its operator set: a tool that does more than it says (moves money elsewhere, say) is caught only by the review, and an agent's charter does not bound money a tool moves outside the ledger. A pin shows that a tool's definition is the one reviewed, not that its server's code is |
| `agent:consolidation` | A consolidated note is a model's reading of the episodes it cites. The gates check that it cites episodes it was shown, that the notes it replaces exist, its kind and its size, not that it reads the episodes right. Each note links to its episodes and names the model and the consolidator, so it can be checked against them |
| `agent:recall-local` | Session search reads the sessions kept on the machine the agent runs on. A hit's check shows whether a stored session still hashes to the digest its episode recorded; a session never recorded (it did not finish, or ran without a memory log) is searched unchecked |
| `memory:archive-key` | A memory archive is encrypted under a key derived from its writer's wallet seed: whoever holds the seed can open it, and on a devnet anyone who knows a named wallet's name. A provider sees an archive's size, which log it is of, and when it was made |
| `storage:retrieval-replay` | A provider checks a retrieval request's signature and expiry (at most five minutes ahead), not whether it was seen before: within that window a copied request retrieves the data again. Retrieval speaks plain HTTP unless put behind TLS |
| `agent:storage-from-account` | A storage deal an agent posts is paid from its account's balance, the money it pays fees with, not from its charter's budget: the charter's caveats do not bound it |
| `agent:skill-pre-approval` | A skill a session has read pre-approves the calls its `allowed-tools` names until the session ends: where the autonomy level would ask a person, they run. A rule naming a command's first words (`Bash(git:*)`) covers whatever that program can be told to do through its arguments, inside the sandbox. A pre-approval never covers a call the level refuses, spending, or a session that has read content from outside, and the agent cannot grant one, since it writes only a skill's name, description and instructions: it comes from whoever placed the skill, or from the operator's consent when installing its release |
| `agent:declared-hosts` | The egress proxy holds a skill's scripts to the hosts their skill declares by the name a script asks for, at addresses that are public where they resolve on the machine. A declared host that forwards what it is sent, serves what others put there, or shares a front (a CDN) whose other sites a script can ask for inside the encrypted tunnel takes a script further. Behind an upstream proxy, a name the machine cannot resolve goes to that proxy, and its network decides where the name leads. With Docker, declared hosts are not filtered: the operator's network decides |
| `agent:secret-store` | Skills' secrets are kept encrypted under a key in a file beside them, readable only by the operator's user: whoever can read that file can read them. A script that receives a secret can send it wherever its skill may connect, and can change it so that it is not cut out of what the model sees: the runtime removes the value as it is, not every encoding of it |
| `agent:mcp-remote` | An MCP server over HTTP runs elsewhere, under whoever runs it: it sees the arguments of every call made to it and answers what it likes. Over HTTPS its certificate shows only that it holds the name in its URL; plain HTTP, which the client allows to any address the operator gives, is as private as the networks it crosses. A header whose value names an environment variable (`${VAR}`) sends that value with every request |
| `agent:mcp-oauth` | OAuth tokens for MCP servers are kept encrypted under the secrets' key (`agent:secret-store`): whoever can read that key can act at that server as the person who consented, with the scopes granted, until the tokens expire or are revoked, and a refresh token renews them without the person. The login trusts the authorization server the server's metadata names: it checks that the metadata names the server, that the authorization server's own names it as issuer and that its answer comes back with the state and the issuer sent, and holds discovery and tokens to HTTPS at public addresses (plain HTTP when the MCP server is on this machine, private addresses when it is at one), but an authorization server of an attacker's that a server names is caught only by the person reading the page they consent on. `logout` revokes tokens only where the authorization server takes revocations; elsewhere they stay valid until they expire |
| `agent:browser` | The browser is Chromium (or Chrome), contained by bubblewrap's namespaces where it runs in them (its own sandbox is off there, as it cannot nest inside), with the machine's managed browser policies; elsewhere by its own sandbox, as the operator's user, and never as root. Without bubblewrap it reaches the egress proxy through a port on this machine's loopback, which any program of the machine can use while it runs, the runtime's upstream proxy and its credentials included |
| `agent:browser-hosts` | A browser held to the hosts the operator names acts on their pages after reading them, as the autonomy level allows: a page there can lead the model to carry what it knows to that site, in an address, a form or a post, and a named host that forwards what it is sent, or serves what others put there, takes it further. Without named hosts, its actions after a page wait for a person |
| `agent:browser-logins` | A login is filled only into a field whose frame is of the login's origin, as the browser knows it: a page of that origin can still send what is typed in it elsewhere, so a login is as safe as its site's pages. The password is cut out of what the browser returns as it is, not in every form a page could show it in |
| `agent:desktop` | A desktop's programs are contained as its sandbox contains them, their address space unbounded under bubblewrap; the X server they share lets each read the others' windows and send them input. A Docker desktop reaches what its Docker network reaches |
| `impl:batch-size-fee` | A batch member's fee floor splits the cost of checking signatures by the size its batch declares. A signer that declares 256 members and sends one pays about a fifth of the base fee for a signature check; validators bound this by checking cheap things first and taking batches whole |
| `impl:insecure-sim-signatures` | Only when a devnet is started with `--insecure-fast-signatures`: validators sign with a simulation MAC. The chain identity records it, and light-client reports say so |

## What a light client trusts

A client pins one value, the [chain identity](../reference/chain-identity.md),
from a source it trusts. Everything below follows from it. Here is what
each line of `sion verify` establishes, and what it still assumes:

| Line | Establishes | Still assumes |
|---|---|---|
| `chain: … matches the pinned identity` | The node serves the chain you pinned; validator keys derive their identifiers | The pin came from a source you trust |
| `committees: followed from the pinned epoch 0 to epoch …` | Each later committee came from the state, certified by the committee before it | Each of those committees had fewer than a third Byzantine stake |
| `certificate: … of … stake signed (quorum …)` | More than two thirds of the epoch's committee signed this header | Fewer than a third of it is Byzantine (else a conflicting header may exist) |
| `forced inclusion: …` | The committee certified how many forced intents are pending and overdue | The summary is certified, not proven: the World Proof proves the obligations but does not check the summary against them |
| `receipts: …` | The node's transactions are the certified order, and each receipt is in the receipts root | As for the certificate |
| `state: …` | An object or claim is in the certified state root | As for the certificate, until the World Proof is checked. That it is still so: a proof shows the state at its Pulse, and a node can answer from an older one. The `sion` commands that read the ledger's state this way (`sion skill latest`, say) refuse a proof from more than 16 Pulses before the latest certified Pulse they know of, but cannot know of a later Pulse that no source shows them |
| `availability: … random shares … verified` | Withholding enough data to prevent reconstruction would have been caught with the stated probability | Your samples are private and random |
| `world proof: … re-verified by stack ["A", "B"]` | The transition from the previous state is valid, the data is what executed | `impl:replay-proofs`, `impl:shared-primitives`, the specification |
| `lane certificates: …` | Every executed transaction locked its inputs with a quorum of its shard's committee, from the proven validator set | As for the certificate |

Without `--chain`, the first line reads `NOT PINNED: learned from the node
itself`, and every other line is only as good as the node.

## What the devnet does not protect

A devnet runs every validator in one process with keys anyone can derive.
Whoever runs it can sign anything as any validator. It demonstrates the
protocol; it secures nothing.
