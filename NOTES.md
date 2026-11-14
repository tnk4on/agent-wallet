# Reproduction notes

Follow [README.md](README.md) for prerequisites and [AGENTS.md](AGENTS.md) for
ordered review commands. Local tests and public verification need no credentials.
Expected results: 57 Rust tests, 6 watcher tests, 12 verifier tests and 12 demo
tests (10 storage + 2 build prerequisites); formatting and clippy pass. Public verification checks 22 items (4 state/code checks + 18 transactions).

The recorded accounts, WASM SHA-256, transaction links, final state and measured
saved-run verification timing are maintained in [JUDGES.md](JUDGES.md#recorded-testnet-evidence).
The deployed testnet WASM was built with `cargo near build reproducible-wasm` at
[e89d329d7a0558d23de24c4621fd851edff1c1c5](https://github.com/tnk4on/agent-wallet/commit/e89d329d7a0558d23de24c4621fd851edff1c1c5).
Its SHA-256 is `90c867332bb74078d155f48f20ac46c359df5cdbcd198d21b360c8376596bd66`,
matching the on-chain `code_hash` (base58-encoded).

A cold Rust build may take several minutes for toolchain and dependency downloads.

The full local test suite requires Python 3.11+ because `scripts/demo_test.py`
imports `tomllib`. The public verifier alone requires Python 3.9+ and internet.
It reads deployed code, state and finalized outcomes through NEAR and FastNEAR archival RPCs, with pacing,
retries and a three-minute deadline. Unavailable RPCs produce a failing exit status.
The verifier checks the deployed WASM hash, owner, policy, task decisions, finality,
senders, actions, events, rejection reasons and downstream delivery against
[scripts/evidence.json](scripts/evidence.json). Success prints `[PASS] all 22 checks`.

Recorded tasks and keys can expire. Finalized outcomes do not establish current
permission to pay; see [known limits](JUDGES.md#known-limits).

Resume/verification of a saved signing demo with the same account, job
directory and source revision is only expected on the same UTC day. Daily committed spend resets at
UTC rollover, so the final 0.0007 NEAR check can fail afterward.
