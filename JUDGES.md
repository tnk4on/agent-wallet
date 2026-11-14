# Judge evidence guide

Run `bash scripts/verify.sh` from the repository root: expect `[PASS] all 22 checks`.
It reads public testnet state and the 18 transactions in `scripts/evidence.json`;
it never loads credentials or submits transactions. See [AGENTS.md](AGENTS.md)
for ordered commands, timings and prerequisites.

Code prefixes below: `learning.rs` and `lib.rs` are under `contracts/wallet/src/`.
Rust tests live in `contracts/wallet/src/learning_tests.rs`, except
`agent_cannot_change_owner_controls` in `contracts/wallet/src/lib.rs`.
Watcher tests are in `scripts/watcher.test.mjs`; verifier tests in `scripts/verify_test.py`.

Command key (run from repository root; each runs the named tests as part of its suite):

- **R**: `(cd contracts/wallet && cargo test --locked)` → 57 passed.
- **W**: `node --test scripts/watcher.test.mjs` → 6 passed.
- **P**: `python3 scripts/verify_test.py` → 12 passed.
- **D**: `python3 scripts/demo_test.py` → 12 passed (10 storage containment + 2 build prerequisites, all offline).
- **V**: `bash scripts/verify.sh` → all 22 checks pass.

Transactions illustrate the scenario; unit tests cover additional edge cases.
A recorded transaction does not prove every adversarial variant. Use **V** to
verify the [recorded transactions](#recorded-transactions)' finalized outcomes and expected receipt results.

| Claim | Code location (file:function) | Test name(s) | Command | Testnet transaction |
| --- | --- | --- | --- | --- |
| Numeric observations, bounded single/daily spend | `learning.rs:record_observation, check_direct; lib.rs:charge` | `observation_records_numeric_rows_and_keeps_methods_separate`; `observation_caps_daily_rollover_pause_and_revoke_remain_effective` | R + V | `observe-5` |
| Deterministic caps, count filter, rounded margin and proposal hash | `learning.rs:propose_policy, with_margin` | `proposal_filters_rare_pairs_rounds_up_and_hashes_deterministically` | R + V | `propose` |
| Owner approves exact hash once; replaces legacy allowlist | `learning.rs:approve_policy` | `approval_requires_owner_exact_hash_and_only_occurs_once`; `approved_policy_blocks_unknown_peer_and_legacy_transfer_bypass` | R + V | `approve` |
| Direct unknown-peer transfer rejected without bypass | `lib.rs:transfer; learning.rs:check_direct` | `approved_policy_blocks_unknown_peer_and_legacy_transfer_bypass`; `learned_methods_are_per_peer_and_external_calls_cannot_bypass_caps` | R + V | `poisoned-unknown` |
| Exceptions reserve the hard ceiling; no double spend or replay, including rollover | `learning.rs:request_payment, cosign, reserved_total; lib.rs:current_spend` | `reservations_block_double_spend_across_agent_keys_before_cosign`; `rollover_keeps_pending_capacity_and_cosign_charges_execution_day`; `pending_cosign_schedules_exact_transfer_once_and_preserves_reservation_total` | R + V | `large-40x` |
| Veto releases reservation, not source call quota | `learning.rs:veto` | `veto_releases_budget_without_refunding_agent_quota_and_cannot_replay` | R + V | `watcher-1` |
| Contract re-checks watcher task amount, budget, deadline and identity | `learning.rs:cosign, authorize_approver` | `watcher_task_amount_budget_and_deadline_fail_closed`; `watcher_identity_is_scoped_and_cannot_request_spend_or_change_owner_policy` | R + V | `watcher-2` |
| Owner can substitute and bypass watcher task conditions | `learning.rs:cosign, authorize_approver; lib.rs:set_daily_limit` | `owner_can_approve_unknown_peer_without_watcher_task_permission`; `agent_cannot_change_owner_controls` | R | Not exercised in recorded demo; unit tests only |
| Text-blind mechanical watcher compares identifiers and integers | `scripts/watcher.mjs:decide` | `40x amount is vetoed`; `legitimate exception is co-signed`; `account names are compared exactly, including injection-like identifiers` | W + V | `watcher-1` |
| Watcher aggregates task spending and rejects expiry | `scripts/watcher.mjs:decide` | `budget aggregates previously co-signed amounts using arbitrary precision`; `exact expiry and terminal status cannot authorize a payment` | W + V | `watcher-2` |
| Public verifier requires finality, delivery and exact rejection reason | `scripts/verify.py:check_transaction` | `test_success_requires_finality_and_downstream_success`; `test_rejection_requires_exact_reason_in_status_and_receipt` | P + V | `poisoned-unknown` |
| Pause, revocation, source-key renewal and lowered ceiling block co-signing; watcher expiry/quota apply | `learning.rs:cosign, authorize_approver; lib.rs:authorize_agent` | `pause_origin_revocation_renewal_and_lowered_ceiling_block_cosign`; `watcher_expiry_quota_and_event_payloads_contain_only_codes_and_numeric_fields` | R | Not exercised in recorded demo; unit tests only |
| Observation rows and live pending storage are bounded to 16 | `learning.rs:check_direct, request_payment` | `observation_rejects_row_seventeen_without_losing_existing_rows`; `pending_storage_is_bounded_and_terminal_records_are_reused_without_id_replay` | R | Not exercised in recorded demo; unit tests only |

## Known limits

- No prompt-injection prevention claim: no LLM agent processes an attack document
  in the demo. Names resembling instructions are compared as account identifiers.
- Observation bypasses the allowlist, within observation caps and explicit denials.
  Malicious bounded payments can influence the proposal; owner review is required.
- An in-pattern malicious peer can receive funds up to the derived payment/daily
  limits. Caps reset daily; they do not prove service delivery or limit lifetime loss.
- A stolen watcher can approve matching pending requests within remaining task
  budgets; it can also veto them. The watcher repeats contract conditions and
  adds a separate key, not independent semantic judgment. It is a single key.
- Owner compromise is outside protection: the owner can raise the hard ceiling
  and co-sign without watcher task checks. Task conditions are fixed at init.
- `propose_policy` is owner-only in the current code (test
  `proposal_requires_owner_and_non_owner_cannot_replace_approval_hash`). The
  recorded deployment uses owner-only `propose_policy`; **V** checks its deployed
  bytes against the hash in [recorded evidence](#recorded-testnet-evidence).
  Enforce has no observation restart / policy re-derivation path.
- Failed downstream receipts retain conservative spend/quota accounting. Pending
  reservations survive UTC rollover; terminal pending records may be evicted.
- Recorded tasks/keys expire; evidence verification checks finalized outcomes,
  not current permission to pay. Public archival RPC availability is required.
- CLI testnet prototype only: no UI, production audit, delivery attestation or
  implemented compensation pool. The optional signing demo needs the tools, testnet CLI configuration and
  external job/credential storage described in [README.md](README.md); the
  key-free verifier and unit tests do not need that setup.

## Verify the deployed code

Use cargo-near 0.22.0, Git, and a running Docker daemon with the `docker` CLI.
Podman also works through its Docker-compatible socket: start the Podman API
service and set `export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock`
(the `docker` CLI is still required). Use a clean committed checkout of the build
source commit recorded below and in `scripts/evidence.json` (`source_commit`),
including the committed `Cargo.lock`. A different commit can
change the embedded metadata and WASM hash even if contract logic is identical.

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

## Recorded testnet evidence

Build source commit (`source_commit`): [e89d329d7a0558d23de24c4621fd851edff1c1c5](https://github.com/tnk4on/agent-wallet/commit/e89d329d7a0558d23de24c4621fd851edff1c1c5).
The deployed WASM was built with `cargo near build reproducible-wasm` at that
commit using the image pinned in `contracts/wallet/Cargo.toml`. Its SHA-256
matches the on-chain `code_hash` (base58-encoded).

Recorded on 2026-11-15 (JST; payment timestamps are 2026-11-14 UTC). The saved-run verification took 37.71 seconds, excluding
setup and build time. `results.json` records `fresh_payment_flow: false` and
`under_60_seconds: false`; this is not a fresh payment-flow timing claim.

Owner: `agentwallet-demo-8cf55c1f7e0b.testnet`.
Wallet: `policy-62aff8.agentwallet-demo-8cf55c1f7e0b.testnet`.
Watcher: `watcher-62aff8.agentwallet-demo-8cf55c1f7e0b.testnet`.

WASM SHA-256: `90c867332bb74078d155f48f20ac46c359df5cdbcd198d21b360c8376596bd66`.

The recorded flow ended in `Enforce`: committed spend 0.0007 testnet NEAR,
reservations zero, task exception spend 0.0002, and seven agent calls used.
The automatic per-payment cap is 0.00012, automatic daily cap 0.0006, and owner
hard daily ceiling 0.01 testnet NEAR. Daily spend changes at UTC rollover;
verification checks persistent observations, policy, pending outcomes and task spend.

### Recorded transactions

All outcomes are finalized. `poisoned-unknown` is an expected contract rejection;
the other transactions succeeded, including downstream payment delivery where applicable.
The claim table above uses the transaction names below.

| Transaction | Explorer |
| --- | --- |
| `create-wallet` | [651D5UwWvN91YD7BA2wF3q2gCsPYAhiy5E7js3X7z8TZ](https://testnet.nearblocks.io/txns/651D5UwWvN91YD7BA2wF3q2gCsPYAhiy5E7js3X7z8TZ) |
| `create-watcher` | [H7DnMp5iVvKt5yKHNVEQnGHpi94pdKr57jHhAC5tP1uq](https://testnet.nearblocks.io/txns/H7DnMp5iVvKt5yKHNVEQnGHpi94pdKr57jHhAC5tP1uq) |
| `add-agent-key` | [2XddnBeAnYE23CwJpfZUrUTC8D9ph4zEWRo3rRtQotxb](https://testnet.nearblocks.io/txns/2XddnBeAnYE23CwJpfZUrUTC8D9ph4zEWRo3rRtQotxb) |
| `add-watcher-key` | [HF1z4ng4VfTZGmdRgipRNvucic8JizezreCjtY4RywGJ](https://testnet.nearblocks.io/txns/HF1z4ng4VfTZGmdRgipRNvucic8JizezreCjtY4RywGJ) |
| `deploy-init` | [8hqvoihnnhNhy8KowHxS25udaynwXMc9HTXr8KDdxTW8](https://testnet.nearblocks.io/txns/8hqvoihnnhNhy8KowHxS25udaynwXMc9HTXr8KDdxTW8) |
| `register-agent` | [4BktjwA7m2KvyPSnXG2hLLcgoUxq1Cmo6jBjXjjwg2PU](https://testnet.nearblocks.io/txns/4BktjwA7m2KvyPSnXG2hLLcgoUxq1Cmo6jBjXjjwg2PU) |
| `observe-1` | [4nzPKquRTK3MsHwxLpo9NB7PRKVQNnEunfT9kzaKmUzp](https://testnet.nearblocks.io/txns/4nzPKquRTK3MsHwxLpo9NB7PRKVQNnEunfT9kzaKmUzp) |
| `observe-2` | [EaeBZ9re2edXuUN91jDxdoYDUK2GRB9k7oFtsZ9yqieW](https://testnet.nearblocks.io/txns/EaeBZ9re2edXuUN91jDxdoYDUK2GRB9k7oFtsZ9yqieW) |
| `observe-3` | [37SB7LUvK44jJuS5W4MgYjDDbaNFdioHXvc64yT6f6HK](https://testnet.nearblocks.io/txns/37SB7LUvK44jJuS5W4MgYjDDbaNFdioHXvc64yT6f6HK) |
| `observe-4` | [BJvEd8u5q97QE9rQe3mNrR8JTr5i6qx9ePjF8M5hd6tv](https://testnet.nearblocks.io/txns/BJvEd8u5q97QE9rQe3mNrR8JTr5i6qx9ePjF8M5hd6tv) |
| `observe-5` | [GFtuwcaNZNMm1UKVusjYWMEwQKmWBrDY3oBZjuwYMX7y](https://testnet.nearblocks.io/txns/GFtuwcaNZNMm1UKVusjYWMEwQKmWBrDY3oBZjuwYMX7y) |
| `propose` | [8qfUH54Lu8EjmPjz1iK5r7SQGdWQPmoTtAQcaJn4AZi1](https://testnet.nearblocks.io/txns/8qfUH54Lu8EjmPjz1iK5r7SQGdWQPmoTtAQcaJn4AZi1) |
| `approve` | [42qVe7EArYmR8y9uwiCkifKeQk9KsDusfNvDwwcKzidb](https://testnet.nearblocks.io/txns/42qVe7EArYmR8y9uwiCkifKeQk9KsDusfNvDwwcKzidb) |
| `poisoned-unknown` | [4VASqy1PkWZdMh26JLCtM5xrLpffjk7bCdvKLkGLHTxU](https://testnet.nearblocks.io/txns/4VASqy1PkWZdMh26JLCtM5xrLpffjk7bCdvKLkGLHTxU) |
| `large-40x` | [RHbFxCi89i5umUmsv3UZGsQKg2fSFfvDdfrak8zXr89](https://testnet.nearblocks.io/txns/RHbFxCi89i5umUmsv3UZGsQKg2fSFfvDdfrak8zXr89) |
| `watcher-1` | [2DBD842NfU4DiXZNAbp7fsh1moqWehpCxgfwgXi2uJbu](https://testnet.nearblocks.io/txns/2DBD842NfU4DiXZNAbp7fsh1moqWehpCxgfwgXi2uJbu) |
| `legitimate-bump` | [4T1yu5FRDmSyf7K46yzsatKQ5mjxxYkQRtZChFbagCq2](https://testnet.nearblocks.io/txns/4T1yu5FRDmSyf7K46yzsatKQ5mjxxYkQRtZChFbagCq2) |
| `watcher-2` | [F2fxG5Bk8Hx4NoioiL6p1X3vWqG4KWQtj1LpRNRssKRx](https://testnet.nearblocks.io/txns/F2fxG5Bk8Hx4NoioiL6p1X3vWqG4KWQtj1LpRNRssKRx) |

The machine-readable expectations are in [scripts/evidence.json](scripts/evidence.json).
The supplied `results.json` contains the 12 payment/policy transaction hashes;
saved public RPC outcomes supply the six setup transaction hashes. The verifier
checks all 18 transactions plus four state/code checks (22 total).
