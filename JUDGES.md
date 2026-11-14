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
A historical transaction does not prove every adversarial variant. Use **V** to
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
- Recorded tasks/keys expire; evidence verification proves historical outcomes,
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

## Recorded testnet evidence

This evidence predates the reproducible container build switch. It remains
historical evidence only; source-to-deployment reproducibility must be checked
after the operator records the build source commit used for the deployed WASM and regenerates
`scripts/evidence.json`, the accounts, WASM hash, timings and transaction links below.
Build source commit (`source_commit`): `<SOURCE_COMMIT>` — to be filled by the
operator here and in `scripts/evidence.json`; no source match is established yet.

Recorded on 2026-11-15. The five-step payment flow took 52.21 seconds, excluding
setup and build time. This measurement does not include tool installation.

Owner: `agentwallet-demo-0c2d08eef013.testnet`.
Wallet: `policy-204786.agentwallet-demo-0c2d08eef013.testnet`.
Watcher: `watcher-204786.agentwallet-demo-0c2d08eef013.testnet`.

WASM SHA-256: `87a5f9186d7213f6094961adc8bb0ddc191bd22f8dfa0e336a26bf4fa45da06f`.

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
| `create-wallet` | [2FUKHQ6QPi7g6UQea6CTX5PbJhby9XrQn5Q7iFsq5eLR](https://testnet.nearblocks.io/txns/2FUKHQ6QPi7g6UQea6CTX5PbJhby9XrQn5Q7iFsq5eLR) |
| `create-watcher` | [5vJjx488BWB3u7UWidj8N3AtpGuZ6cYJ7RvfKFob9mhP](https://testnet.nearblocks.io/txns/5vJjx488BWB3u7UWidj8N3AtpGuZ6cYJ7RvfKFob9mhP) |
| `add-agent-key` | [4ky5adiVpuZEw4L5Q5DitWT3VAEmWmhSqg4qzRNqFbCT](https://testnet.nearblocks.io/txns/4ky5adiVpuZEw4L5Q5DitWT3VAEmWmhSqg4qzRNqFbCT) |
| `add-watcher-key` | [6pcuqUn1zUgr1TFFPmzqDPcZVV6L3U3HeTeWE5hDHdqt](https://testnet.nearblocks.io/txns/6pcuqUn1zUgr1TFFPmzqDPcZVV6L3U3HeTeWE5hDHdqt) |
| `deploy-init` | [ADDxLkKpEPAmoF9PFsnDxPHhKpXNT3XH4Vyj7u7XfzWj](https://testnet.nearblocks.io/txns/ADDxLkKpEPAmoF9PFsnDxPHhKpXNT3XH4Vyj7u7XfzWj) |
| `register-agent` | [3cBsW1pTc7pAX4AYfWzhbuCdtgXiBjtH9HtxUirMQwDG](https://testnet.nearblocks.io/txns/3cBsW1pTc7pAX4AYfWzhbuCdtgXiBjtH9HtxUirMQwDG) |
| `observe-1` | [G2ig4UjPeqyYmNDFKBxiLPpSGr9ti2DcwZ6ZmCKcX54F](https://testnet.nearblocks.io/txns/G2ig4UjPeqyYmNDFKBxiLPpSGr9ti2DcwZ6ZmCKcX54F) |
| `observe-2` | [4NrMxhVD1YfZigvPxUGKGgaRsweJ2FBSxQVyTBWyBr4E](https://testnet.nearblocks.io/txns/4NrMxhVD1YfZigvPxUGKGgaRsweJ2FBSxQVyTBWyBr4E) |
| `observe-3` | [HiXkuheBmk4FZRE1uTRYJDkhRUVjkbJbMntXiHPzLjAc](https://testnet.nearblocks.io/txns/HiXkuheBmk4FZRE1uTRYJDkhRUVjkbJbMntXiHPzLjAc) |
| `observe-4` | [2QPCN8aFCWXqSSKDaaVrVjevHm6FBaQ1e2dMhbbd5uG1](https://testnet.nearblocks.io/txns/2QPCN8aFCWXqSSKDaaVrVjevHm6FBaQ1e2dMhbbd5uG1) |
| `observe-5` | [8prbNekFJhJA2CWcLScb7JNULBtR3dcH78699AbFY2nt](https://testnet.nearblocks.io/txns/8prbNekFJhJA2CWcLScb7JNULBtR3dcH78699AbFY2nt) |
| `propose` | [1P3jHpimj45xKccQW1ivyB9Jq6m94iDFkeeThg1YPdT](https://testnet.nearblocks.io/txns/1P3jHpimj45xKccQW1ivyB9Jq6m94iDFkeeThg1YPdT) |
| `approve` | [TqGPNR4WV7nGwvYXYACZrwQ9z9ZRnMNfMiheggS5uAY](https://testnet.nearblocks.io/txns/TqGPNR4WV7nGwvYXYACZrwQ9z9ZRnMNfMiheggS5uAY) |
| `poisoned-unknown` | [FvQWM7CVW79Qtw1xJDwkky6f25NDJmKfNpwwj3G53D5L](https://testnet.nearblocks.io/txns/FvQWM7CVW79Qtw1xJDwkky6f25NDJmKfNpwwj3G53D5L) |
| `large-40x` | [8yXPD23bnP5ivwm7akRTe41toH1cPFnqomRjv1ujzV2W](https://testnet.nearblocks.io/txns/8yXPD23bnP5ivwm7akRTe41toH1cPFnqomRjv1ujzV2W) |
| `watcher-1` | [EDXsFiLf9aAi8151V2sDrjqi6wC4Tq1XhPvj3d3bnkAH](https://testnet.nearblocks.io/txns/EDXsFiLf9aAi8151V2sDrjqi6wC4Tq1XhPvj3d3bnkAH) |
| `legitimate-bump` | [EXzG9ZS4SVfD2MDS13rhUbcmNthCrXaHoQ8zxZD1kpUr](https://testnet.nearblocks.io/txns/EXzG9ZS4SVfD2MDS13rhUbcmNthCrXaHoQ8zxZD1kpUr) |
| `watcher-2` | [F36PJWBxTRZGBiXdVFYB7QiadmmpZJq94V2JCPQkfmxS](https://testnet.nearblocks.io/txns/F36PJWBxTRZGBiXdVFYB7QiadmmpZJq94V2JCPQkfmxS) |

The machine-readable expectations are in [scripts/evidence.json](scripts/evidence.json).
