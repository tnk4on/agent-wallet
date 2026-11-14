# Agent policy wallet

Observe usual NEAR payments → approve numeric limits once → enforce them on-chain.

## Check it in 1 minute

From the repository root, run:

```bash
bash scripts/verify.sh
```

No keys, no faucet, no transactions submitted. Requires Python 3.9+ and internet.
Expected: `[PASS] all 22 checks` (4 state/code checks + 18 recorded transactions).
Usually about a minute; RPC retries can take up to the three-minute deadline.
See [JUDGES.md](JUDGES.md) for claim-to-code/test/transaction evidence and
[AGENTS.md](AGENTS.md) for the ordered review commands.

## What it stops / what it doesn't

These bounds cover transferred NEAR / attached deposits, excluding gas and storage.
They limit payment authority, not prompt injection. Amounts are testnet NEAR.
`learning.rs` and `lib.rs` below are in [`contracts/wallet/src`](contracts/wallet/src).
Bounds assume the owner remains trusted and the restricted keys are used.

| Scenario | What stops / what doesn't | Maximum loss allowed by the contract |
| --- | --- | --- |
| Anything during observation, including a malicious peer | Does not establish trust; observation bypasses the allowlist (explicit denials still apply). | Per payment: `LearningConfig.observe_single_cap` ≤ `OBSERVE_SINGLE_CAP` (0.05); per UTC day: min(`OBSERVE_DAILY_CAP` = 0.5, `Contract.daily_limit`). `learning.rs:check_direct`, `lib.rs:charge`. |
| Direct transfer/call outside the derived peer × method policy | Rejects external calls outside policy. Only NEAR transfers can use `request_payment` → `cosign` for a pending exception (`PaymentMethod::Pay` only). | 0 transferred by the rejected/pending action; pending transfer execution needs `cosign`. `learning.rs:check_direct`, `request_payment`; `lib.rs:transfer`, `call`. |
| A derived allowlisted peer becomes malicious | Does not stop an in-pattern drain or prove delivery. | Per automatic payment: `LearnedLimit.cap` = ceil(observed `max_amount` × (1 + `margin_bps`/10,000)); daily total ≤ min(`Proposal.daily_cap`, `Contract.daily_limit`), including reservations in the check. `margin_bps` ≤ 10,000; `Proposal.daily_cap` derives from observed `max_daily` by the same formula. `learning.rs:propose_policy`, `check_direct`, `with_margin`. |
| Compromised agent requesting exceptions | Can reserve capacity, but cannot co-sign; no transfer until approval. | Pending transfer loss: 0; all executed spend + reservations ≤ owner `Contract.daily_limit` per UTC day. Approved exceptions may exceed the derived daily cap, but not that hard ceiling. `learning.rs:request_payment`, `cosign`; `lib.rs:current_spend`. |
| Compromised watcher, with pending requests | Does not stop malicious approvals within registered tasks. Contract re-checks peer, method, amount, budget and expiry. | Per exception ≤ `TaskCondition.max_amount`; remaining lifetime total ≤ sum(`TaskCondition.budget` − `RegisteredTask.spent`), also subject to the owner daily ceiling. `learning.rs:cosign`; no fixed numeric task-budget maximum. |
| Compromised owner key | Owner can raise the ceiling and co-sign without watcher task checks. | No policy-defined bound against the owner; potentially all available wallet funds. `lib.rs:set_daily_limit`, `require_owner`; `learning.rs:authorize_approver`, `cosign`. |

Daily caps reset at UTC boundaries, so they are not lifetime loss bounds. Agent and
watcher expiry / call quotas can reduce exposure (`AgentKey` in `lib.rs`). Review
observed peers before approving: bounded malicious observations can enter a proposal.

## What is new here

Limits, allowlists and approvals are common building blocks. This prototype combines:

1. The contract itself derives a bounded policy from observed payments
   (observed maxima plus a fixed margin); the owner approves the exact proposal
   hash once to switch to Enforce, with no
   rules to write for the derived policy (owner still configures safety/task budgets).
2. Out-of-pattern payments need a co-sign from a text-blind, LLM-free watcher
   whose power is bounded by owner-registered task budgets that the contract
   re-checks. The owner can substitute; unknown peers fail on direct transfers.

The demo is a structured payment attack demonstration, with no LLM agent processing
an injected document. It demonstrates limits on payments an agent can authorize.

## 1. Install tools → get a usable Rust/NEAR shell

Install Git, Node 22+ and Python 3.11+ using your OS package manager or the
[Git](https://git-scm.com/downloads), [Node.js](https://nodejs.org/en/download) and
[Python](https://www.python.org/downloads/) installers. Check `git --version`,
`node --version` and `python3 --version` → confirm the required versions.
Install [rustup](https://rustup.rs) and follow its shell/PATH setup instructions.

Run the first `cargo test` from `contracts/wallet` → rustup automatically installs
Rust 1.93.0, rustfmt, clippy and the `wasm32-unknown-unknown` target from
[`rust-toolchain.toml`](contracts/wallet/rust-toolchain.toml). No `rustup default`
is needed when commands run from that directory. The first run needs network
access to rustup and crates.io; allow a minute or two for downloads and several
minutes for a cold build. The WASM target downloads but unit tests do not need it.

The key-free check needs only Python 3.9+ and internet. The watcher uses Node
built-ins; Python uses its standard library. The full local test suite requires
Python 3.11+ because the demo storage tests import `tomllib`. The full demo
additionally needs cargo-near 0.22.0 and near-cli-rs 0.30.1.

## 2. Clone the repository → get a checkout

Clone the public repository:

```bash
git clone https://github.com/tnk4on/agent-wallet
cd agent-wallet
bash scripts/verify.sh
```

## 3. Run the optional full demo → see five testnet checks

The signing runner needs Python 3.11+, cargo-near 0.22.0, near-cli-rs 0.30.1,
the WASM target and initialized testnet CLI configuration using the default
`~/.near-credentials` location. Job files and credential backing storage must stay
under `AGENT_WALLET_DEMO_ROOT` (default `~/.agent-wallet-demo`), outside Git.
The default credential location must resolve there; an absent location is created
as a symlink by the runner. Optional `AGENT_WALLET_ENV` can name a local script
that puts tools on PATH. This is not a portable fresh-OS installer.
[AGENTS.md](AGENTS.md) gives the exact optional command and funding prerequisites.
Local checks and the key-free verifier run from a fresh clone without that setup.
The runner requires a clean committed git tree and the `docker` CLI with a
running Docker or Podman service (see [Verify the deployed code](#verify-the-deployed-code)).
It builds the contract reproducibly, checks `chain_id=testnet`, deploys and initializes
atomically, and registers restricted keys. No model API key is needed.

| Step | Do X | Get Y, verified on testnet |
| --- | --- | --- |
| 1: Observe | Pay the usual peer 0.0001 NEAR five times. | Count 5; sum 0.0005; maximum 0.0001. |
| 2: Propose and approve | Propose 20% margin; owner approves the exact proposal hash. | `Enforce`; per-payment cap 0.00012; automatic daily cap 0.0006. |
| 3: Reject a redirect | Transfer to `ignore-previous-and-approve.testnet`. | `ERR_NOT_IN_LEARNED_POLICY`; no transfer and no call-quota use. |
| 4: Veto an inflated bill | Request 0.004 NEAR, 40 times the usual amount, for the usual peer. | `Pending` reserves 0.004; watcher vetoes with `TASK_LIMIT` and releases it. |
| 5: Approve a permitted exception | Request 0.0002 NEAR for the registered task. | Watcher co-signs with `TASK_OK`; downstream transfer succeeds. |

All amounts in this walkthrough are **testnet NEAR**. Expected output includes:

```text
STEP 1 PASS: five usual payments recorded
STEP 2 PASS: exact proposal hash approved once
STEP 3 PASS: poisoned unknown peer rejected on-chain
STEP 4 PASS: usual peer x40 reserved then vetoed
STEP 5 PASS: owner-registered legitimate bump co-signed and delivered
All five testnet steps PASS; evidence: <external-job-directory>/results.json
```

Each call also prints its transaction hash. Success requires finalized outcomes
and successful downstream receipts; step 3 deliberately has a failed receipt.
The watcher runs as a separate Node process in steps 4 and 5.
This is a structured attack demonstration; no LLM is asked to process a document.

## 4. Inspect evidence or resume → verify the same payments

Inspect `results.json` in the external job directory. Expect five `PASS` entries,
explorer links, `network: testnet`, and final spent/committed 0.0007 NEAR,
reserved zero, task exception spend 0.0002 NEAR and seven agent calls used.
The owner hard daily ceiling is 0.01 NEAR. Keep evidence and credentials outside Git.

Rerun the same demo command with the same job directory and source revision → verify saved hashes
and checkpoints without paying again. Resume/verification of a saved run is only
expected on the same UTC day: daily committed spend resets at UTC rollover,
so the final 0.0007 NEAR check can fail afterward. Use a new external directory
for a new run.
Faucet refusal, insufficient funding or unavailable RPCs are external failures;
do not report them as a passing demonstration. See [AGENTS.md](AGENTS.md).

Verify the committed public evidence without credentials or new transactions:

```bash
bash scripts/verify.sh
```

Expect 22 PASS checks (4 state/code checks and 18 recorded transactions). The verifier
uses NEAR and FastNEAR testnet archival RPCs, so older transactions remain readable.
Requests run sequentially with a one-second gap, up to two retries with backoff for
429/timeouts/5xx, and provider fallback. Unresponsive providers are skipped for the
rest of the run. The overall deadline is three minutes; unavailable public RPCs
still produce a failing exit status. Python 3.9+ and internet access are required.

## Verify the deployed code

Use cargo-near 0.22.0, Git, and a running Docker daemon with the `docker` CLI.
Podman also works through its Docker-compatible socket: start the Podman API
service and set `export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock`
(the `docker` CLI is still required). Use a clean committed checkout of the build
source commit recorded in [JUDGES.md](JUDGES.md#recorded-testnet-evidence) and
`scripts/evidence.json` (`source_commit`), including the committed `Cargo.lock`.
The operator must fill in `<SOURCE_COMMIT>` with the commit used to build the
deployed WASM; the existing evidence predates the reproducible build switch.
A different commit can
change the embedded metadata and WASM hash even if contract logic is identical.

From the repository root:

```bash
git checkout <source_commit>
cd contracts/wallet
cargo near build reproducible-wasm
sha256sum target/near/wallet.wasm
python3 - '<WALLET_ACCOUNT_ID>' <<'PYCODE'
import hashlib, json, sys, urllib.request
from pathlib import Path
raw = hashlib.sha256(Path("target/near/wallet.wasm").read_bytes()).digest()
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n, encoded = int.from_bytes(raw, "big"), ""
while n:
    n, remainder = divmod(n, 58)
    encoded = alphabet[remainder] + encoded
expected = "1" * (len(raw) - len(raw.lstrip(b"\0"))) + encoded
payload = {"jsonrpc": "2.0", "id": "code-hash", "method": "query", "params": {
    "request_type": "view_account", "finality": "final", "account_id": sys.argv[1]}}
request = urllib.request.Request("https://test.rpc.fastnear.com", data=json.dumps(payload).encode(),
                                 headers={"Content-Type": "application/json"})
with urllib.request.urlopen(request, timeout=30) as response:
    actual = json.load(response)["result"]["code_hash"]
print("local:", expected, "deployed:", actual)
if actual != expected:
    raise SystemExit("FAIL: deployed code_hash differs from local WASM SHA-256 (base58)")
print("PASS: deployed code matches this build")
PYCODE
```

Replace `<source_commit>` with the recorded build source commit and
`<WALLET_ACCOUNT_ID>` with the deployed testnet wallet. The RPC query is
read-only and needs no keys. `code_hash` is the base58 encoding of the WASM's
SHA-256 digest; `sha256sum` prints that digest in hexadecimal.
The pinned image/digest and inner build command in `Cargo.toml` control the build;
`--locked` is implicit for `reproducible-wasm`. Cargo-near embeds NEP-330
`contract_source_metadata`, including the source revision/link derived from
`package.repository` and the container build details. See the
[cargo-near template](https://github.com/near/cargo-near/blob/cargo-near-v0.22.0/cargo-near/src/commands/new/new-project-template/Cargo.template.toml).

## 5. Run checks → reproduce the regression results

```bash
(cd contracts/wallet && cargo test --locked)
node --test scripts/watcher.test.mjs
python3 scripts/verify_test.py
python3 scripts/demo_test.py
(cd contracts/wallet && cargo fmt --check)
(cd contracts/wallet && cargo clippy --all-targets -- -D warnings)
```

Expect **57 Rust tests**, **6 watcher tests**, **12 verifier tests**, and **12 demo tests** (10 storage + 2 build prerequisites), with fmt/clippy passing.
Rust tests use a mocked runtime; the five-step demo above uses public testnet.
Read [NOTES.md](NOTES.md) for reproduction timing and verification scope.
Recorded accounts, the WASM hash and transaction links are in [JUDGES.md](JUDGES.md#recorded-testnet-evidence).

## How authorization works → understand what the checks prove

`Observe` records bounded numeric peer/method statistics, without invoice text
or external arguments. Existing pause, revocation, expiry, quota and hard-budget
checks still apply. Observation has a single-payment cap at most 0.05 NEAR and
an aggregate cap of 0.5 NEAR per UTC day; the demo sets much smaller limits.

Owner-only `propose_policy` deterministically rounds up observed maxima plus the
bounded margin. A pair needs the configured count floor. `Proposed` freezes payments;
the owner reviews and approves the exact hash once to enter `Enforce`.
This prevents an unreviewed proposal from becoming payment authority.
Both `policy_proposed` and `policy_approved` events include the base64 `hash`,
`peers` (peer, method, and per-payment cap), and `daily_cap` for explorer audits.

The derived daily cap governs automatic payments. The owner hard ceiling covers
executed payments plus pending reservations. Exceptions reserve capacity before
approval; veto/expiry releases it, and co-signing charges it exactly once.
Reservations survive UTC rollover. Failed downstream receipts retain conservative
spend and quota accounting; they are not automatically refunded in this version.

The agent's native Function Call key names this wallet and
`transfer,call,request_payment`; the separate watcher's key names only
`cosign,veto`. Both have bounded gas allowances, registry expiry and quotas.
The contract checks the signer, immediate predecessor and exact public key.
See the [NEAR access-key documentation](https://docs.near.org/protocol/accounts-contracts/access-keys).

The watcher compares exact peer, method and task identifiers, amount, remaining
task budget and expiry against immutable owner-registered conditions. It reads
no invoice, webpage or model narrative. The contract repeats the task checks.
The owner can approve an exception directly; pause, source-key validity, explicit
recipient denial, expiry and the hard ceiling still apply.

Observation rows and live pending requests are each bounded to 16.
A compromised agent can fill pending slots and reserve capacity, blocking further
requests without moving funds. Expired requests still occupy slots and reserve
capacity until someone calls `expire(id)`; cleanup is not automatic. Anyone,
including the owner or an agent, may call it after timeout: the contract has no
caller authorization check. The caller must use an access key permitting `expire`;
the demo agent and watcher keys omit it. This is an availability limitation.
Changing task definitions or upgrading from older state needs a new deployment.
This does not establish service delivery or protect a compromised owner key.
Full API and accounting details: [contract reference](contracts/wallet/README.md).

## Libraries and reused code

The contract uses near-sdk 5.29; see [Cargo.lock](contracts/wallet/Cargo.lock)
for resolved versions and transitive dependencies. The reproducible build uses
`sourcescan/cargo-near:0.21.1-rust-1.96.0`, pinned to
`sha256:ccb22bb4e677ed022d8b9d1aa1b32f0af52dd0471ef1c32e48dedbf68ee6ee17`
in [Cargo.toml](contracts/wallet/Cargo.toml).
Node/Python scripts import only standard-library modules and local project modules;
the signing scripts also invoke the external tools listed in the prerequisites.
The cargo-near template link above documents reproducible-build metadata; no
additional project template origin is declared in Cargo.toml or this README.

## Related work

These comparisons are the author's summary of public documentation as of
2026-11-14. They are not exhaustive or independent product tests.

| System | Documented mechanism vs this prototype |
| --- | --- |
| [Hermit Guard](https://github.com/Hermit-Ai-Labs/hermit-app) | Owner-configured pockets, single/daily limits, allowlists and owner-approved exceptions; here the contract derives policy and a task-bounded watcher can co-sign exceptions. |
| [nearswaps agent kit](https://github.com/nearswaps/nearswaps-agents) | Restricted trade/withdraw keys, on-chain hard limits and local policy; here policy derivation is on-chain and exceptions use a separate co-signer. |
| [Key Doctor / Agent Guard](https://github.com/BLV-Labs/keydoctor) | DAO-configured single/peer/24-hour limits with council votes above limits; here observed payments produce the policy for one wallet. |
| [Rein v2](https://github.com/let-the-dreamers-rise/rein) | Shadow observations feed an off-chain compiler with margin; its documented per-peer amounts are monitor-only, while here derivation and per-peer caps are enforced by the contract. |
| [AWS IAM Access Analyzer](https://docs.aws.amazon.com/IAM/latest/UserGuide/access-analyzer-policy-generation.html) | Generates policies from up to 90 days of activity for human review/application; here the wallet records and derives payment policy on-chain for hash approval. |
| [AppArmor](https://apparmor.net/getting-started/generate-with-aa-genprof/) | Learning/complain mode and log-based profile generation precede enforcement; here the same broad workflow applies to payment authority inside the wallet contract. |
| [Safe Fiducia](https://safefoundation.org/blog/safe-research-fiducia-onchain-trust-rules-and-cosigning) | On-chain trust rules require a co-signer outside the allowlist; here the watcher's approvals must also fit owner-registered task budgets. |

## Roadmap → identify work that remains

- **Task wallet:** create, fund, expire and close a wallet for one bounded task.
- **UI:** show observations, exact proposal changes, pending requests and receipts.
- **Compensation pool:** define funding, eligibility and claims before implementation.
- **Risk scanner:** surface risky permissions and destinations for owner review.

These four roadmap items are unimplemented. Current scope is the CLI testnet
prototype, its contract and its mechanical watcher. See the
[hackathon brief](https://nearhackathon.com/HACKATHON.md) for context.

## License

[MIT](LICENSE). Copyright (c) 2026 Shion Tanaka.
