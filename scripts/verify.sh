#!/usr/bin/env bash
# Public testnet views and recorded outcomes only; Python 3.9+ stdlib.
set -euo pipefail
exec python3 "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/verify.py" "$@"
