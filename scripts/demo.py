#!/usr/bin/env python3
"""Headless official-faucet NEAR demo. All persisted output is sanitized."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request

REPO = Path(__file__).resolve().parent.parent
# Private storage for job files and credentials; must be outside the git tree.
STORAGE_ROOT = Path(os.environ.get("AGENT_WALLET_DEMO_ROOT", Path.home() / ".agent-wallet-demo")).expanduser().resolve()
DEFAULT_JOB = STORAGE_ROOT / "demo"


def safe_path(job, rel="."):
    """Resolve a job path without allowing writes to escape private storage."""
    storage_root = STORAGE_ROOT.resolve()
    job = Path(job).absolute()
    base = job.resolve()
    candidate = job / rel
    target = candidate.resolve()
    if (not base.is_relative_to(storage_root) or base.is_relative_to(REPO)
            or not target.is_relative_to(base) or target.is_relative_to(REPO)
            or not target.is_relative_to(storage_root)):
        raise RuntimeError("Job paths must remain inside the job directory and private storage outside git")
    # Check intermediate components too, including a symlink that leaves the
    # job before a later component points back into it.
    if not candidate.is_relative_to(job):
        raise RuntimeError("Job path must be relative to the job directory")
    current = job
    for part in candidate.relative_to(job).parts:
        current = current / part
        resolved = current.resolve()
        if (not resolved.is_relative_to(base) or resolved.is_relative_to(REPO)
                or not resolved.is_relative_to(storage_root)):
            raise RuntimeError("Job path component escapes private job storage")
    return target


def check_storage_tree(root, job=None):
    """Check existing descendants before external tools can write through them."""
    storage_root = STORAGE_ROOT.resolve()
    root = Path(root)
    target = root.resolve()
    if not target.is_relative_to(storage_root) or target.is_relative_to(REPO):
        raise RuntimeError("Storage paths must remain in private storage outside git")
    if job is not None:
        safe_path(job, root)
    if root.is_dir():
        for directory, dirs, files in os.walk(root, followlinks=False):
            for name in dirs + files:
                child = Path(directory) / name
                resolved = child.resolve()
                if (not resolved.is_relative_to(target) or resolved.is_relative_to(REPO)
                        or not resolved.is_relative_to(storage_root)):
                    raise RuntimeError("Storage descendant escapes its storage directory")


def sanitize(text):
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    # Public Ed25519 keys are at most 44 base58 characters; secrets are longer.
    text = re.sub(r"ed25519:[1-9A-HJ-NP-Za-km-z]{45,}", "[REDACTED]", text)
    return "\n".join(
        "[REDACTED credential line]"
        if re.search(r"private[_ -]?key|seed[_ -]?phrase|mnemonic|secret recovery", line, re.I)
        else line
        for line in text.splitlines()
    ) + "\n"


def rpc(url, method, params):
    request = urllib.request.Request(
        url,
        data=json.dumps({"jsonrpc": "2.0", "id": "testnet-demo", "method": method, "params": params}).encode(),
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(4):
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if error.code not in {408, 429, 500, 502, 503, 504} or attempt == 3:
                raise
            try:
                retry_after = float(error.headers.get("Retry-After", 2 ** (attempt + 1)))
            except ValueError:
                retry_after = 2 ** (attempt + 1)
            # RPC reads are safe to retry; send_tx retries contain identical
            # signed bytes and therefore retain the same nonce and hash.
            time.sleep(min(10, max(1, retry_after)))
        except (TimeoutError, urllib.error.URLError):
            if method == "send_tx" or attempt == 3:
                raise
            time.sleep(0.5 * (attempt + 1))


def main():
    parser = argparse.ArgumentParser(description="Five-step learned-policy/co-sign demo on NEAR testnet")
    parser.add_argument("owner", nargs="?", help="fresh generic agentwallet-demo-<12 random hex characters>.testnet")
    parser.add_argument("--job-dir", type=Path, default=DEFAULT_JOB)
    parser.add_argument("--rpc-url", help="HTTPS testnet RPC; chain identity is always verified")
    parser.add_argument("--existing-owner", action="store_true", help="reuse an explicitly supplied generic funded demo owner")
    args = parser.parse_args()
    if args.existing_owner and not args.owner:
        parser.error("--existing-owner requires the explicit generic owner")
    if shutil.which("docker") is None:
        parser.error("reproducible build requires the docker CLI on PATH and a running container runtime; "
                     "use Docker, or Podman with DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock")
    if subprocess.run(["git", "status", "--porcelain"], cwd=REPO, check=True,
                      capture_output=True, text=True).stdout.strip():
        parser.error("reproducible build requires a clean committed git tree; commit all source changes first")
    job = safe_path(args.job_dir)
    check_storage_tree(job, job)
    if not args.owner:
        saved = safe_path(job, "owner-account.txt")
        import secrets
        args.owner = saved.read_text().strip() if saved.exists() else "agentwallet-demo-" + secrets.token_hex(6) + ".testnet"
    if not re.fullmatch(r"agentwallet-demo-[0-9a-f]{12}\.testnet", args.owner):
        parser.error("owner must use the generic demo prefix, 12 random hex characters, and .testnet")
    job.mkdir(parents=True, exist_ok=True)
    previous_owner = safe_path(job, "owner-account.txt")
    if previous_owner.exists() and previous_owner.read_text().strip() != args.owner:
        parser.error("use a separate --job-dir for a new owner")
    wasm = REPO / "contracts/wallet/target/near/wallet.wasm"
    subprocess.run(["cargo", "near", "build", "reproducible-wasm"],
                   cwd=REPO / "contracts/wallet", check=True)
    digest = hashlib.sha256(wasm.read_bytes()).hexdigest()
    safe_path(job, "wasm-sha256.txt").write_text(digest + "\n")
    config = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "near-cli/config.toml"
    settings = tomllib.loads(config.read_text())
    network = settings["network_connection"]["testnet"]
    if args.rpc_url:
        if not args.rpc_url.startswith("https://"):
            parser.error("--rpc-url must use HTTPS")
        network["rpc_url"] = args.rpc_url
    if network["network_name"] != "testnet" or network["faucet_url"].rstrip("/") != "https://helper.nearprotocol.com/account":
        raise RuntimeError("Refusing configuration other than testnet with the official NEAR faucet")
    if rpc(network["rpc_url"], "status", [])['result']['chain_id'] != "testnet":
        raise RuntimeError("Refusing RPC whose chain_id is not testnet")
    credentials = Path.home() / ".near-credentials"
    if Path(settings["credentials_home_dir"]) != credentials:
        raise RuntimeError("near credentials must use the default ~/.near-credentials location")
    # The default home alias points at private storage under AGENT_WALLET_DEMO_ROOT,
    # so new regular files (including credentials and CLI metadata) stay there.
    if not credentials.exists() and not credentials.is_symlink():
        backing = safe_path(job, "credentials")
        backing.mkdir(mode=0o700, exist_ok=True)
        credentials.symlink_to(backing, target_is_directory=True)
    if not credentials.resolve().is_relative_to(STORAGE_ROOT) or credentials.resolve().is_relative_to(REPO):
        raise RuntimeError("Credential storage must resolve under AGENT_WALLET_DEMO_ROOT outside git")
    safe_path(job, "owner-account.txt").write_text(args.owner + "\n")
    check_storage_tree(credentials)
    manifest_path = safe_path(job, "accounts.json")
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())
        if manifest["owner"] != args.owner:
            raise RuntimeError("Account manifest owner mismatch")
    else:
        import secrets
        suffix = secrets.token_hex(3)
        manifest = {"owner": args.owner, "wallet": "policy-" + suffix + "." + args.owner,
                    "watcher": "watcher-" + suffix + "." + args.owner}
        for account_id in [manifest["wallet"], manifest["watcher"]]:
            absent = rpc(network["rpc_url"], "query", {"request_type": "view_account", "account_id": account_id, "finality": "final"})
            if "UNKNOWN_ACCOUNT" not in json.dumps(absent.get("error", {})):
                raise RuntimeError("New wallet/watcher account must not already exist")
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print("WASM SHA256:", digest, flush=True)
    print("New testnet wallet:", manifest["wallet"], flush=True)
    if args.existing_owner:
        credential = credentials / "testnet" / (args.owner + ".json")
        if not credential.is_file():
            raise RuntimeError("Explicit funding owner credentials are missing")
        public = json.loads(credential.read_text())["public_key"]
        native = rpc(network["rpc_url"], "query", {"request_type": "view_access_key", "account_id": args.owner,
                                                    "public_key": public, "finality": "final"})
        if native.get("result", {}).get("permission") != "FullAccess":
            raise RuntimeError("Explicit funding owner control key is not valid")
    else:
        command = [
            "near", "account", "create-account", "sponsor-by-faucet-service", args.owner,
            "autogenerate-new-keypair", "--signature-scheme", "ed25519",
            "save-to-legacy-keychain", "network-config", "testnet", "create",
        ]
        print("WASM SHA256:", digest, flush=True)
        print("Official faucet:", network["faucet_url"], flush=True)
        print("Attempting generic testnet account:", args.owner, flush=True)
        faucet_log = safe_path(job, "faucet.log")
        output = faucet_log.read_text() if faucet_log.exists() else ""
        if f"New account <{args.owner}> created successfully." in output:
            code = 0
            print("Resuming the recorded official-faucet account; no new faucet request.", flush=True)
        else:
            existing = rpc(network["rpc_url"], "query", {
                "request_type": "view_account", "account_id": args.owner, "finality": "final",
            })
            if "result" in existing:
                raise RuntimeError("Existing owner has no successful faucet evidence in this job directory")
            if "UNKNOWN_ACCOUNT" not in json.dumps(existing.get("error", {})):
                raise RuntimeError("Could not establish that the owner is absent; refusing a faucet retry")
            try:
                check_storage_tree(credentials)
                result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, text=True, timeout=90)
                output = sanitize(result.stdout)
                code = result.returncode
            except subprocess.TimeoutExpired as error:
                raw = error.stdout or b""
                output = sanitize(raw.decode(errors="replace") if isinstance(raw, bytes) else raw)
                output += "Headless faucet command timed out after 90 seconds; no retry performed.\n"
                code = 124
            faucet_log.write_text(output)
            print(output, end="", flush=True)
        if code or re.search(r"captcha|could not be created|failed with status", output, re.I):
            raise RuntimeError("Faucet did not complete headlessly. Stop; see sanitized faucet.log for exact blocker.")
        if f"New account <{args.owner}> created successfully." not in output:
            raise RuntimeError("Faucet did not confirm account creation; stop without deployment")
        tx_hash = re.search(r"Transaction ID: ([1-9A-HJ-NP-Za-km-z]{40,44})", output).group(1)
        faucet_tx = rpc(network["rpc_url"], "tx", {
            "tx_hash": tx_hash, "sender_account_id": "testnet", "wait_until": "FINAL",
        })
        outcome = faucet_tx.get("result", {})
        if "SuccessValue" not in outcome.get("status", {}) or outcome.get("transaction", {}).get("receiver_id") != args.owner:
            raise RuntimeError("Faucet transaction did not confirm the expected successful testnet creation")
        safe_path(job, "transactions").mkdir(exist_ok=True)
        safe_path(job, "transactions/faucet.json").write_text(json.dumps(outcome, indent=2) + "\n")
    from demo_flow import run
    run(args.owner, job, network["rpc_url"], credentials, digest, manifest)


if __name__ == "__main__":
    os.umask(0o077)
    try:
        main()
    except Exception as error:
        print(sanitize(str(error)), end="", file=sys.stderr)
        sys.exit(1)
