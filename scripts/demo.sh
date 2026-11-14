#!/usr/bin/env bash
# Testnet only. Account names are arguments; credentials never enter this tree.
set -euo pipefail
set +x
umask 077
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# Optional: AGENT_WALLET_ENV may point to a local script that puts the tools on PATH.
if [[ -n "${AGENT_WALLET_ENV:-}" && -f "$AGENT_WALLET_ENV" ]]; then
  . "$AGENT_WALLET_ENV"
fi
exec python3 -B "$repo_dir/scripts/demo.py" "$@"
