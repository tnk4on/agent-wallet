# AI reviewer instructions

This is a NEAR testnet policy-wallet prototype. Review [README.md](README.md)
and [JUDGES.md](JUDGES.md). Keep contract and script behavior unchanged when editing documentation.
Do not infer prompt-injection prevention from the structured payment demo.

Run these commands in order from the repository root. Timings are estimates;
a first Rust build needs dependency downloads and may take several minutes.
The verifier needs Python 3.9+ and internet; the full local test suite needs Python 3.11+
(`scripts/demo_test.py` imports `tomllib`), Node 22+
and Rust 1.93.0 (pinned by `contracts/wallet/rust-toolchain.toml`).

1. Read-only public evidence (~1 minute, hard deadline 3 minutes):
   ```bash
   bash scripts/verify.sh
   ```
   Expect exit 0 and `[PASS] all 22 checks`: 4 state/code checks + 18 transactions.
   RPC outages are failures, not passes. No keys or faucet needed.
2. Contract tests (~1–5 seconds warm; allow 2–10 minutes cold):
   ```bash
   (cd contracts/wallet && cargo test --locked)
   ```
   Expect `57 passed; 0 failed`, plus 0 doc tests.
3. Watcher tests (usually <1 second):
   ```bash
   node --test scripts/watcher.test.mjs
   ```
   Expect 6 tests passed, 0 failed.
4. Verifier tests (usually <1 second):
   ```bash
   python3 scripts/verify_test.py
   ```
   Expect TAP `1..12`, all `ok`.
5. Demo storage tests (offline, usually <1 second):
   ```bash
   python3 scripts/demo_test.py
   ```
   Expect 12 tests passed (10 storage + 2 build prerequisites), 0 failed.
6. Formatting and lint (~seconds warm; allow several minutes cold):
   ```bash
   (cd contracts/wallet && cargo fmt --check)
   (cd contracts/wallet && cargo clippy --all-targets -- -D warnings)
   ```
   Expect exit 0, no formatting differences, no clippy warnings.

The deployed testnet WASM was built with `cargo near build reproducible-wasm` at
[e89d329d7a0558d23de24c4621fd851edff1c1c5](https://github.com/tnk4on/agent-wallet/commit/e89d329d7a0558d23de24c4621fd851edff1c1c5).
Its SHA-256 is `90c867332bb74078d155f48f20ac46c359df5cdbcd198d21b360c8376596bd66`,
matching the on-chain `code_hash` (base58-encoded).

## Optional full signing demo (not required for judging)

Needs Python 3.11+, cargo-near 0.22.0,
near-cli-rs 0.30.1, the WASM target, a clean committed checkout, and the Docker CLI
with a running container runtime (Docker, or Podman via `DOCKER_HOST`).
Also needs initialized testnet CLI configuration
using the default `~/.near-credentials` location, which must resolve to external
credential backing storage below. An absent location is created as a symlink.
Optional `AGENT_WALLET_ENV` names a local script that puts tools on PATH.
Job files and credentials must resolve under `AGENT_WALLET_DEMO_ROOT`
(default `~/.agent-wallet-demo`), outside Git. Do not inspect or print credentials. It is not a portable fresh-OS demo.
Use an existing funded generic account matching `agentwallet-demo-<12 hex>.testnet`;
it needs more than 4.3 testnet NEAR for wallet/watcher funding plus storage/gas.
With `DEMO_OWNER` set to that account, run (~1 minute warm, longer to compile):

```bash
mkdir -p -m 700 "${AGENT_WALLET_DEMO_ROOT:-$HOME/.agent-wallet-demo}"
DEMO_JOB_DIR="$(mktemp -d "${AGENT_WALLET_DEMO_ROOT:-$HOME/.agent-wallet-demo}/judge.XXXXXX")"
bash scripts/demo.sh "$DEMO_OWNER" --existing-owner --rpc-url https://test.rpc.fastnear.com --job-dir "$DEMO_JOB_DIR"
```

Expect `STEP 1 PASS` through `STEP 5 PASS` and `All five testnet steps PASS`.
Rerun with the same account, job directory and source revision to verify/resume saved transactions
only on the same UTC day. Daily committed spend resets at UTC rollover, so the
final 0.0007 NEAR check can fail afterward.
Omitting the account and `--existing-owner` requests faucet funding instead;
faucet/RPC failure must be reported as such. This optional demo submits testnet
transactions. The key-free verifier above only reads previously recorded evidence.

## Do not run

Nothing mainnet: no mainnet deployments, transactions or account changes.
Do not run deploy/undeploy workflows, access secret/key/token stores, print
credentials, use AWS/SSH, create `.env` files, weaken tests, change git hooks/config,
run `git push` or `gh`. Stage/commit only when explicitly requested.

## Secret scanning

Before a requested commit, run `gitleaks git . --redact` and
`gitleaks dir . --redact`; both must report no leaks.
