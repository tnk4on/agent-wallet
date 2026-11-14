"""Deployment, restricted agent key, and verified policy scenarios on testnet."""

import base64
import hashlib
import json
import re
import subprocess
import time

from demo import REPO, rpc, sanitize, safe_path, check_storage_tree

SMALL = 10**20  # 0.0001 testnet NEAR
LIMIT = 100 * SMALL
EXPLORER = "https://testnet.nearblocks.io/txns/"


def b58encode(data):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    number = int.from_bytes(data, "big")
    output = ""
    while number:
        number, digit = divmod(number, 58)
        output = alphabet[digit] + output
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + output


class Flow:
    def __init__(self, owner, job, url, credentials, wallet=None):
        self.owner, self.job, self.url = owner, job, url
        self.wallet = wallet or "wallet." + owner
        self.credentials = credentials
        check_storage_tree(job, job)
        self.txs = safe_path(job, "transactions")
        self.txs.mkdir(exist_ok=True)
        self.logs = safe_path(job, "logs")
        self.logs.mkdir(exist_ok=True)
        self.signing_context = {}

    def query(self, method, params):
        response = rpc(self.url, method, params)
        if "error" in response:
            raise RuntimeError(json.dumps(response["error"]))
        return response["result"]

    def finalized_tx(self, tx_hash, signer, target="FINAL"):
        deadline = time.monotonic() + 60
        ready = {"FINAL"} if target == "FINAL" else {"EXECUTED_OPTIMISTIC", "EXECUTED", "FINAL"}
        while time.monotonic() < deadline:
            response = rpc(self.url, "tx", {"tx_hash": tx_hash, "sender_account_id": signer, "wait_until": "NONE"})
            if "error" in response:
                if response["error"].get("cause", {}).get("name") not in {"UNKNOWN_TRANSACTION", "TIMEOUT_ERROR"}:
                    raise RuntimeError(json.dumps(response["error"]))
            elif response["result"].get("final_execution_status") in ready:
                return response["result"]
            time.sleep(0.25)
        raise RuntimeError(f"Transaction {tx_hash} did not reach {target}; resume the same saved hash")

    def cli(self, name, arguments):
        check_storage_tree(self.job, self.job)
        check_storage_tree(self.credentials)
        result = subprocess.run(["near", *map(str, arguments)], stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=90)
        output = sanitize(result.stdout)
        safe_path(self.job, "logs/" + name + ".log").write_text(output)
        if result.returncode:
            raise RuntimeError(f"near failed for {name}; sanitized log: {self.logs / (name + '.log')}\n{output}")
        return output

    def verify(self, name, outcome, expected):
        status = outcome["status"]
        if expected == "SUCCESS":
            if "SuccessValue" not in status or any("Failure" in r["outcome"]["status"] for r in outcome["receipts_outcome"]):
                raise RuntimeError(f"{name}: expected success, received {json.dumps(status)}")
        elif "Failure" not in status or f"Smart contract panicked: {expected}" not in json.dumps(status):
            raise RuntimeError(f"{name}: expected {expected}, received {json.dumps(status)}")
        tx_hash = outcome["transaction"]["hash"]
        print(f"{name}: {expected}; {tx_hash}", flush=True)
        return outcome

    def transaction(self, name, arguments, signer, key_file, expected="SUCCESS", signed=False, wait_until="FINAL"):
        target = safe_path(self.job, "transactions/" + name + ".json")
        if target.exists():
            saved = json.loads(target.read_text())
            outcome = self.query("tx", {"tx_hash": saved["transaction"]["hash"],
                                       "sender_account_id": signer, "wait_until": "FINAL"})
            return self.verify(name, outcome, expected)
        tail = ["network-config", "testnet", "sign-with-access-key-file", str(key_file)]
        if signed:
            signed_file = safe_path(self.job, "transactions/" + name + ".signed.json")
            resuming_signed = signed_file.exists()
            if not resuming_signed:
                public = json.loads(key_file.read_text())["public_key"]
                context_key = (signer, public)
                native = self.signing_context.get(context_key)
                if native is None:
                    native = self.query("query", {"request_type": "view_access_key", "account_id": signer,
                                                  "public_key": public, "finality": "final"})
                    self.signing_context[context_key] = native
                offline = ["--nonce", str(native["nonce"] + 1), "--block-hash", native["block_hash"],
                           "--block-height", str(native["block_height"])]
                self.cli(name + "-sign", ["--offline", *arguments, *tail, *offline, "save-to-file", str(signed_file)])
            saved = json.loads(signed_file.read_text())
            encoded = saved["signed_transaction_as_base64"]
            payload = base64.b64decode(encoded, validate=True)
            if len(payload) < 66 or payload[-65] != 0:
                raise RuntimeError("Expected a classic Ed25519 signed transaction")
            tx_hash = b58encode(hashlib.sha256(payload[:-65]).digest())
            # A new file has never been broadcast. Only recovery needs a prior status lookup.
            params = {"tx_hash": tx_hash, "sender_account_id": signer, "wait_until": "NONE"}
            known = rpc(self.url, "tx", params) if resuming_signed else {"error": {"cause": {"name": "UNKNOWN_TRANSACTION"}}}
            if "error" in known:
                if known["error"].get("cause", {}).get("name") != "UNKNOWN_TRANSACTION":
                    raise RuntimeError(json.dumps(known["error"]))
                try:
                    response = rpc(self.url, "send_tx", {"signed_tx_base64": encoded, "wait_until": "INCLUDED"})
                except (TimeoutError, OSError):
                    # The send may have succeeded. Poll its hash; never re-sign after an ambiguous failure.
                    response = {}
                if "error" in response and response["error"].get("cause", {}).get("name") != "TIMEOUT_ERROR":
                    safe_path(self.job, "transactions/" + name + ".rpc-error.json").write_text(json.dumps(response, indent=2) + "\n")
                    raise RuntimeError(f"{name} ({tx_hash}): {json.dumps(response['error'])}")
            outcome = self.finalized_tx(tx_hash, signer, wait_until)
            if outcome["transaction"]["hash"] != tx_hash:
                raise RuntimeError("Signed transaction hash differs from RPC result")
            cache = self.signing_context.get((signer, outcome["transaction"]["public_key"]))
            if cache is not None:
                cache["nonce"] = outcome["transaction"]["nonce"]
        else:
            output = self.cli(name, arguments + tail + ["send"])
            match = re.search(r"Transaction ID: ([1-9A-HJ-NP-Za-km-z]{40,44})", output)
            if not match:
                raise RuntimeError(f"No transaction hash for {name}; stop without repeating the action")
            outcome = self.query("tx", {"tx_hash": match.group(1), "sender_account_id": signer,
                                       "wait_until": "FINAL"})
        target.write_text(json.dumps(outcome, indent=2) + "\n")
        return self.verify(name, outcome, expected)

    def call(self, name, method, arguments, signer, key_file, expected="SUCCESS", wait_until="FINAL"):
        prepaid_gas = "20 Tgas"
        return self.transaction(name, ["contract", "call-function", "as-transaction", self.wallet,
                                      method, "json-args", json.dumps(arguments), "prepaid-gas", prepaid_gas,
                                      "attached-deposit", "0 NEAR", "sign-as", signer],
                                signer, key_file, expected, True, wait_until)

    def view(self, method, arguments=None):
        outcome = self.query("query", {"request_type": "call_function", "account_id": self.wallet,
                                      "method_name": method, "args_base64": base64.b64encode(
                                          json.dumps(arguments or {}).encode()).decode(), "finality": "final"})
        return json.loads(bytes(outcome["result"]))


def run(owner, job, url, credentials, digest, manifest):
    import time
    f = Flow(owner, job, url, credentials, manifest["wallet"])
    watcher = manifest["watcher"]
    owner_key = credentials / "testnet" / (owner + ".json")
    wallet_key = credentials / "testnet" / (f.wallet + ".json")
    watcher_control = credentials / "testnet" / (watcher + ".json")
    for name, account_id, funding in [("create-wallet", f.wallet, "4 NEAR"), ("create-watcher", watcher, "0.3 NEAR")]:
        f.transaction(name, ["account", "create-account", "fund-myself", account_id, funding,
                            "autogenerate-new-keypair", "--signature-scheme", "ed25519",
                            "save-to-legacy-keychain", "sign-as", owner], owner, owner_key)

    def restricted_key(name, account_id, controller, methods):
        added = f.transaction(name, ["account", "add-key", account_id, "grant-function-call-access",
                              "--allowance", "0.2 NEAR", "--contract-account-id", f.wallet,
                              "--function-names", methods, "autogenerate-new-keypair",
                              "--signature-scheme", "ed25519", "save-to-legacy-keychain"], account_id, controller)
        action = next(a["AddKey"] for a in added["transaction"]["actions"] if "AddKey" in a)
        public = action["public_key"]
        key_file = credentials / "testnet" / account_id / (public.replace(":", "_") + ".json")
        if not key_file.is_file() or key_file.resolve().is_relative_to(REPO):
            raise RuntimeError("Credentials must remain outside git")
        native = f.query("query", {"request_type": "view_access_key", "account_id": account_id,
                                  "public_key": public, "finality": "final"})["permission"]["FunctionCall"]
        if native["receiver_id"] != f.wallet or set(native["method_names"]) != set(methods.split(",")):
            raise RuntimeError("Native key scope mismatch")
        if native["allowance"] is None or int(native["allowance"]) > 2 * 10**23:
            raise RuntimeError("Unbounded native key allowance")
        return public, key_file

    agent_public, agent_key = restricted_key("add-agent-key", f.wallet, wallet_key, "transfer,call,request_payment")
    watcher_public, watcher_key = restricted_key("add-watcher-key", watcher, watcher_control, "cosign,veto")
    config_file = safe_path(job, "init-policy.json")
    if config_file.exists():
        config = json.loads(config_file.read_text())
    else:
        block = f.query("block", {"finality": "final"})
        expires = str(int(block["header"]["timestamp_nanosec"]) + 86_400_000_000_000)
        config = {"owner": owner, "daily_limit": str(LIMIT), "learning": {
            "observe_single_cap": str(SMALL), "min_observations": 5, "min_count": 3,
            "watcher": watcher, "watcher_key": watcher_public, "watcher_expires_at_ns": expires,
            "watcher_max_calls": 20, "pending_timeout_ns": "3600000000000",
            "tasks": [{"task_id": "1", "peer": owner, "method": "Pay", "max_amount": str(2 * SMALL),
                       "budget": str(3 * SMALL), "expires_at_ns": expires}]}}
        config_file.write_text(json.dumps(config, indent=2) + "\n")
    f.transaction("deploy-init", ["contract", "deploy", f.wallet, "use-file",
                  str(REPO / "contracts/wallet/target/near/wallet.wasm"), "with-init-call", "init", "json-args",
                  json.dumps(config), "prepaid-gas", "100 Tgas", "attached-deposit", "0 NEAR"], f.wallet, wallet_key)
    deployed = f.query("query", {"request_type": "view_account", "account_id": f.wallet, "finality": "final"})
    if deployed["code_hash"] != b58encode(bytes.fromhex(digest)) or f.view("get_owner") != owner:
        raise RuntimeError("Deployed code/owner mismatch")
    f.call("register-agent", "register_key", {"public_key": agent_public,
           "expires_at_ns": config["learning"]["watcher_expires_at_ns"], "max_calls": 30}, owner, owner_key)
    fresh_payment_flow = not any(f.txs.glob("observe-*"))
    started = time.monotonic()
    steps = []

    def link(outcome):
        tx = outcome["transaction"]["hash"]
        return {"hash": tx, "url": EXPLORER + tx}

    def payment(name, amount, wait_until="FINAL"):
        return f.call(name, "request_payment", {"to": owner, "amount": str(amount), "method": "Pay", "task_id": "1"},
                      f.wallet, agent_key, wait_until=wait_until)

    # Pipeline independent small observations, then finalize all evidence before proposing.
    usual = [payment("observe-" + str(n), SMALL, "EXECUTED_OPTIMISTIC" if n < 5 else "FINAL") for n in range(1, 6)]
    from concurrent.futures import ThreadPoolExecutor
    def finalize(item):
        n, saved = item
        result = f.finalized_tx(saved["transaction"]["hash"], f.wallet)
        f.verify("observe-" + str(n) + "-final", result, "SUCCESS")
        safe_path(job, "transactions/observe-" + str(n) + ".json").write_text(json.dumps(result, indent=2) + "\n")
        return result
    with ThreadPoolExecutor(max_workers=4) as executor:
        usual = list(executor.map(finalize, enumerate(usual, 1)))
    if not (f.txs / "approve.json").exists():
        stats = f.view("get_stats")
        if stats["mode"] not in {"Observe", "Proposed"} or stats["observations"] != 5:
            raise RuntimeError("Observation did not record exactly five payments")
        row = stats["rows"][0]
        if (row["count"], int(row["sum"]), int(row["max_amount"])) != (5, 5 * SMALL, SMALL):
            raise RuntimeError("Observation numeric stats mismatch")
    steps.append({"step": 1, "result": "PASS", "code": "OBSERVED", "transactions": [link(x) for x in usual]})
    print("STEP 1 PASS: five usual payments recorded", flush=True)

    proposed = f.call("propose", "propose_policy", {"margin_bps": 2000, "min_count": 3}, owner, owner_key)
    proposal = f.view("get_stats")["proposal"]
    if len(proposal["rows"]) != 1 or int(proposal["rows"][0]["cap"]) != SMALL * 12 // 10 or int(proposal["daily_cap"]) != 6 * SMALL:
        raise RuntimeError("Deterministic proposal caps mismatch")
    approved = f.call("approve", "approve_policy", {"hash": proposal["hash"]}, owner, owner_key)
    if f.view("get_stats")["mode"] != "Enforce":
        raise RuntimeError("Owner approval did not activate enforcement")
    steps.append({"step": 2, "result": "PASS", "code": "ENFORCE", "transactions": [link(proposed), link(approved)]})
    print("STEP 2 PASS: exact proposal hash approved once", flush=True)

    poisoned = f.call("poisoned-unknown", "transfer", {"receiver": "ignore-previous-and-approve.testnet", "amount": str(SMALL)},
                      f.wallet, agent_key, "ERR_NOT_IN_LEARNED_POLICY")
    steps.append({"step": 3, "result": "PASS", "code": "ERR_NOT_IN_LEARNED_POLICY", "transactions": [link(poisoned)]})
    print("STEP 3 PASS: poisoned unknown peer rejected on-chain", flush=True)

    def watch(request, expected_method, expected_status):
        result = json.loads(base64.b64decode(request["status"]["SuccessValue"]))
        request_id = result["id"]
        watched = safe_path(job, "watcher")
        check_storage_tree(watched, job)
        subprocess_result = subprocess.run(["node", str(REPO / "scripts/watcher.mjs"),
            "--wallet", f.wallet, "--owner", owner, "--watcher", watcher, "--rpc-url", url,
            "--id", request_id, "--key-file", str(watcher_key), "--job-dir", str(watched)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=180)
        output = sanitize(subprocess_result.stdout)
        safe_path(job, "logs/watcher-" + request_id + ".log").write_text(output)
        print(output, end="", flush=True)
        if subprocess_result.returncode:
            raise RuntimeError("Mechanical watcher failed; see sanitized log")
        outcome = json.loads((watched / ("watcher-" + request_id + ".json")).read_text())
        action = outcome["transaction"]["actions"][0]["FunctionCall"]
        if action["method_name"] != expected_method or f.view("get_pending", {"id": request_id})[0]["status"] != expected_status:
            raise RuntimeError("Watcher decision/state mismatch")
        return outcome

    large = payment("large-40x", 40 * SMALL)
    if not (job / "watcher/watcher-1.json").exists():
        stats = f.view("get_stats")
        if int(stats["reserved"]) != 40 * SMALL or int(stats["committed_today"]) != 45 * SMALL:
            raise RuntimeError("Pending did not reserve the hard daily budget before approval")
    vetoed = watch(large, "veto", "Vetoed")
    steps.append({"step": 4, "result": "PASS", "code": "PENDING_THEN_VETOED", "transactions": [link(large), link(vetoed)]})
    print("STEP 4 PASS: usual peer x40 reserved then vetoed", flush=True)

    bump = payment("legitimate-bump", 2 * SMALL)
    cosigned = watch(bump, "cosign", "Cosigned")
    steps.append({"step": 5, "result": "PASS", "code": "PENDING_THEN_COSIGNED", "transactions": [link(bump), link(cosigned)]})
    print("STEP 5 PASS: owner-registered legitimate bump co-signed and delivered", flush=True)
    final_stats = f.view("get_stats")
    if int(final_stats["reserved"]) != 0 or int(final_stats["committed_today"]) != 7 * SMALL or int(final_stats["tasks"][0]["spent"]) != 2 * SMALL:
        raise RuntimeError("Final daily/task reservation accounting mismatch")
    if f.view("get_agent_key", {"public_key": agent_public})[2] != 7:
        raise RuntimeError("Rejected payment consumed quota or approval charged twice")
    results = {"network": "testnet", "owner": owner, "wallet": f.wallet, "watcher": watcher,
               "wasm_sha256": digest, "steps": steps, "fresh_payment_flow": fresh_payment_flow, "five_step_elapsed_seconds": round(time.monotonic() - started, 2),
               "rpc_url": url, "final_stats": final_stats, "agent_public_key": agent_public, "watcher_public_key": watcher_public}
    results_file = safe_path(job, "results.json")
    if results_file.exists():
        original = json.loads(results_file.read_text())
        results["verification_elapsed_seconds"] = results["five_step_elapsed_seconds"]
        results["five_step_elapsed_seconds"] = original["five_step_elapsed_seconds"]
        results["fresh_payment_flow"] = original.get("fresh_payment_flow")
        results["replay_verified"] = True
        results["under_60_seconds"] = original.get("under_60_seconds", False)
    else:
        results["under_60_seconds"] = results["five_step_elapsed_seconds"] < 60 and fresh_payment_flow
    results_file.write_text(json.dumps(results, indent=2) + "\n")
    print("All five testnet steps PASS; evidence:", job / "results.json", flush=True)
