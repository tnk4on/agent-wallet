# Contract reference

Testnet-only NEAR policy wallet, tested locally with Rust 1.93.0 and near-sdk 5.29.
Release WASM uses the pinned container toolchain in `Cargo.toml`.
See the [project README](../../README.md) for the five-step demo, observation
rules, proposal hash, pending reservation semantics, and authorization model.

```bash
cd contracts/wallet # from the repository root
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Install prerequisites using the [project instructions](../../README.md#1-install-tools--get-a-usable-rustnear-shell).
Running Cargo here selects the pinned toolchain automatically; no default toolchain
is required. Initial toolchain/dependency downloads need internet.
The suite contains 57 Rust tests.
Tests use the mocked NEAR runtime and synthetic keys, with no network access.

Fresh clones need no extra environment file. If your tools are not on `PATH`,
set `AGENT_WALLET_ENV` to a local script that adds them before running `scripts/demo.sh`.
The optional signing demo uses `cargo near build reproducible-wasm`
to produce `target/near/wallet.wasm`; see the project README.

## Verify the deployed code

Use cargo-near 0.22.0, Git, and a running Docker daemon with the `docker` CLI.
Podman also works through its Docker-compatible socket: start the Podman API
service and set `export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock`
(the `docker` CLI is still required). Use a clean committed checkout of the exact
revision used for deployment; commit `Cargo.lock` too. A different commit can
change the embedded metadata and WASM hash even if contract logic is identical.

The deployed testnet WASM was built with `cargo near build reproducible-wasm` at
[e89d329d7a0558d23de24c4621fd851edff1c1c5](https://github.com/tnk4on/agent-wallet/commit/e89d329d7a0558d23de24c4621fd851edff1c1c5).
Its SHA-256 is `90c867332bb74078d155f48f20ac46c359df5cdbcd198d21b360c8376596bd66`,
matching the on-chain `code_hash` (base58-encoded).

From the repository root:

```bash
git checkout e89d329d7a0558d23de24c4621fd851edff1c1c5
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

Replace `<WALLET_ACCOUNT_ID>` with the deployed testnet wallet. The RPC query is
read-only and needs no keys. `code_hash` is the base58 encoding of the WASM's
SHA-256 digest; `sha256sum` prints that digest in hexadecimal.
The pinned image/digest and inner build command in `Cargo.toml` control the build;
`--locked` is implicit for `reproducible-wasm`. Cargo-near embeds NEP-330
`contract_source_metadata`, including the source revision/link derived from
`package.repository` and the container build details. See the
[cargo-near template](https://github.com/near/cargo-near/blob/cargo-near-v0.22.0/cargo-near/src/commands/new/new-project-template/Cargo.template.toml).

Initialize and deploy atomically on a new wallet account. `init` accepts
`owner`, `daily_limit` (yoctoNEAR string), and optional `learning`.
A populated learning configuration starts in `Observe`; null selects the legacy
explicit allowlist mode. The layout is incompatible with earlier contract state.
Initialization must be a direct owner or wallet transaction.

`LearningConfig` fields:

| Field | Meaning |
| --- | --- |
| `observe_single_cap` | Positive yoctoNEAR string, at most 0.05 testnet NEAR. |
| `min_observations`, `min_count` | Minimum samples before proposing and minimum count per pair; positive. |
| `watcher`, `watcher_key` | Separate testnet signer account and exact public key. |
| `watcher_expires_at_ns`, `watcher_max_calls` | Registry lifetime and quota. |
| `pending_timeout_ns` | Positive timeout, at most one day. |
| `tasks` | At most 16 unique immutable owner task conditions. |

A task condition contains `task_id`, `peer`, `method: "Pay"`, `max_amount`,
`budget`, and `expires_at_ns`. Amounts and ids/timestamps are decimal strings.
The contract computes a SHA-256 hash of each condition's canonical Borsh bytes.
`get_stats` returns each condition, hash, and cumulative exception spend.
`get_pending` returns statuses `Pending`, `Cosigned`, `Vetoed`, or `Expired`.
`Cosigned` records authorization/scheduling; verify downstream receipts for delivery.

Payment and owner-control methods:

| Method | Authority / effect |
| --- | --- |
| `set_daily_limit(daily_limit)` | Owner; hard aggregate ceiling without clearing spend/reservations. |
| `allow_recipient(account_id)` / `deny_recipient(account_id)` | Owner; explicit destination controls. Learned pair checks still apply. |
| `allow_method(method)` / `deny_method(method)` | Owner; exact external method names for `call`. |
| `register_key(public_key, expires_at_ns, max_calls)` | Owner; registers or renews key usage. Renewal invalidates old requests. |
| `revoke_key(public_key)` | Owner; disables the agent or watcher immediately. |
| `pause()` / `unpause()` | Owner; stops or resumes outgoing actions/co-signs. Veto/expiry can release holds. |
| `transfer(receiver, amount)` | Agent; direct transfer, rejecting learned-policy exceptions. |
| `call(receiver, method, args, deposit, gas)` | Agent; allowed external call; base64 args, gas-unit string. |
| `request_payment(to: AccountId, amount: U128, method: PaymentMethod, task_id: U64) -> PaymentResult` | Agent; `method` is `"Pay"` only. Schedules and charges an allowed NEAR transfer, or reserves an out-of-policy transfer as `Pending` within the hard ceiling, consuming one agent call. Returns `id` (null for direct payment) and `rule`. |
| `propose_policy(margin_bps: u32, min_count: u32) -> Proposal` | Owner; requires learning configuration, enough observations, margin ≤ 10,000 and count ≥ configured floor. Outside Enforce, derives and stores the hashed proposal and enters Proposed, freezing payments. |
| `approve_policy(hash: Base64VecU8) -> ()` | Owner; requires Proposed and the exact proposal hash (base64). Replaces legacy allowlists with proposal entries and enters Enforce. |
| `cosign(id: U64) -> Promise` | Owner or registered watcher; requires an unexpired Pending request, unpaused wallet, valid unchanged source key, no explicit recipient denial and committed spend within the hard ceiling. Watcher must also satisfy task identity, amount, budget and expiry checks. Marks Cosigned, converts reservation to executed spend and schedules the NEAR transfer; consumes a watcher call and task budget when watcher-approved. |
| `veto(id: U64) -> ()` | Owner or registered watcher; marks an unexpired Pending request Vetoed and releases its reservation, even while paused. Consumes a watcher call when watcher-vetoed; does not refund the source agent call. |
| `expire(id: U64) -> ()` | Anyone with an access key permitting this method; at or after timeout, marks a Pending request Expired and releases its reservation, even while paused. Does not refund the source agent call. |

Provision native agent keys on the wallet account with receiver equal to the
wallet and methods `transfer,call,request_payment`. Provision the watcher key on
the separate watcher account with that receiver and only `cosign,veto`.
Native keys must have a bounded gas allowance. Registry changes do not add or
delete native keys; those are separate owner actions.
Both signer and immediate predecessor must match the corresponding account.
Owner controls reject keys in the agent/watcher registry even after revocation.
Self-directed external calls cannot invoke wallet administrative methods.

The daily ceiling is shared across agents, transfer amounts, call deposits,
and live pending reservations. Gas and balances of external tokens are outside
that numeric NEAR ceiling. Failed downstream receipts do not refund spend or
call quota. Zero-deposit external calls still use a call; zero payments fail.
Pending requests are bounded to 16 slots. A compromised agent can fill them and
reserve capacity, blocking further requests without moving funds. Expired pending
requests keep occupying slots and reserving capacity until `expire(id)` is called;
cleanup is not automatic. After timeout, anyone (including the owner or an agent)
can call `expire`: there is no caller authorization check. The access key must
permit that method; the demo's restricted agent and watcher keys omit it.
This is an availability limitation.
Daily accounting uses `block_timestamp / 86_400_000_000_000` (UTC).

Views: `get_owner`, `is_paused`, `get_daily_limit`, `get_spent_today`,
`get_day_bucket`, `is_recipient_allowed`, `is_method_allowed`, `get_agent_key`.
Learning views: `get_stats() -> StatsView` and
`get_pending(id: Option<U64>) -> Vec<PendingPayment>` (null selects all retained requests).
`get_spent_today` includes active reservations, including ones from yesterday.
`get_day_bucket` identifies the last executed-spend bucket.
`get_agent_key` returns `[expires_at_ns, max_calls, used_calls, active]`, or null.

Events use the `EVENT_JSON` envelope. Base payload fields are
`amount`, `to`, `key`, `rule`; payment events add the numeric `id`.
Owner-only proposal and approval events (`policy_proposed`, `policy_approved`)
also include base64 `hash`, `peers` entries with `peer`, `method`, and `cap`,
and `daily_cap`. Caps are yoctoNEAR decimal strings.
External call arguments and narrative text are excluded.
