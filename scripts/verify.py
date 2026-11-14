#!/usr/bin/env python3
"""Verify public testnet evidence. Never load credentials or submit transactions."""
import base64
import hashlib
import json
from pathlib import Path
import sys
import time
import urllib.error
import urllib.request

# Archival providers serve old tx outcomes as well as current state views.
RPCS = ('https://archival-rpc.testnet.near.org',
        'https://archival-rpc.testnet.fastnear.com')
VERIFY_SECONDS = 180
REQUEST_TIMEOUT = 6
PACE_SECONDS = 1.0
RETRY_BACKOFF = (0.5, 1.0)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def equal(actual, expected, label):
    require(actual == expected, f'{label}: expected {expected!r}, got {actual!r}')


def subset(actual, expected, label):
    """Lists are exact in length/order; dict expectations may omit volatile fields."""
    if isinstance(expected, dict):
        require(isinstance(actual, dict), f'{label}: expected object')
        for key, value in expected.items():
            require(key in actual, f'{label}: missing {key}')
            subset(actual[key], value, f'{label}.{key}')
    elif isinstance(expected, list):
        require(isinstance(actual, list), f'{label}: expected list')
        equal(len(actual), len(expected), f'{label} length')
        for i, (value, target) in enumerate(zip(actual, expected)):
            subset(value, target, f'{label}[{i}]')
    else:
        equal(actual, expected, label)


class RpcClient:
    """Serial, paced reads; quarantine exhausted transports for this invocation."""

    def __init__(self):
        self.next_request = 0
        self.unavailable = {}

    def pause(self, seconds, deadline):
        remaining = deadline - time.monotonic()
        require(remaining > seconds, 'verification deadline exceeded')
        if seconds > 0:
            time.sleep(seconds)

    def call(self, methods, params, deadline):
        errors = []
        for url in RPCS:
            if url in self.unavailable:
                errors.append(self.unavailable[url])
                continue
            for method in methods:
                body = json.dumps({'jsonrpc': '2.0', 'id': 1,
                                   'method': method, 'params': params}).encode()
                request = urllib.request.Request(url, body, {'Content-Type': 'application/json'})
                for attempt in range(len(RETRY_BACKOFF) + 1):
                    self.pause(max(0, self.next_request - time.monotonic()), deadline)
                    try:
                        with urllib.request.urlopen(request, timeout=min(
                                REQUEST_TIMEOUT, deadline - time.monotonic())) as response:
                            data = json.load(response)
                        if 'error' in data:
                            raise ValueError(json.dumps(data['error'])[:250])
                        require('result' in data, 'RPC response has no result')
                        return data['result']
                    except (OSError, ValueError, urllib.error.URLError) as exc:
                        message = f'{url} {method}: {exc}'
                        errors.append(message)
                        # Retry transient transports/429/5xx, not invalid params,
                        # pruned transactions, or unsupported RPC methods.
                        transient = isinstance(exc, (OSError, urllib.error.URLError))
                        if isinstance(exc, urllib.error.HTTPError):
                            transient = exc.code == 429 or 500 <= exc.code < 600
                        if transient and attempt < len(RETRY_BACKOFF):
                            self.pause(RETRY_BACKOFF[attempt], deadline)
                            continue
                        if transient:
                            self.unavailable[url] = message
                        break
                    finally:
                        # One second idle after each response, including errors.
                        self.next_request = time.monotonic() + PACE_SECONDS
                if url in self.unavailable:
                    break
        raise ValueError('; '.join(errors))


def view(client, evidence, method, args, deadline):
    result = client.call(('query',), {'request_type': 'call_function', 'finality': 'final',
                         'account_id': evidence['wallet'], 'method_name': method,
                         'args_base64': base64.b64encode(json.dumps(args).encode()).decode()},
                         deadline)
    return json.loads(bytes(result['result']))


def check_code(result, expected_sha256):
    code = base64.b64decode(result['code_base64'], validate=True)
    equal(hashlib.sha256(code).hexdigest(), expected_sha256, 'deployed WASM SHA-256')


def check_transaction(result, expected):
    equal(result['final_execution_status'], 'FINAL', 'finality')
    tx = result['transaction']
    for key, value in [('hash', expected['hash']), ('signer_id', expected['sender']),
                       ('receiver_id', expected['receiver'])]:
        equal(tx[key], value, key)
    actions = []
    for action in tx['actions']:
        kind = next(iter(action)) if isinstance(action, dict) else action
        item = {'type': kind}
        if kind == 'FunctionCall':
            call = action[kind]
            item.update(method=call['method_name'],
                        args=json.loads(base64.b64decode(call['args'])))
        actions.append(item)
    subset(actions, expected['actions'], 'actions')
    equal(set(result['status']), {expected['status']}, 'execution status')
    receipts = result['receipts_outcome']
    require(receipts, 'missing receipt outcomes')
    outcomes = [result['transaction_outcome']['outcome']] + [r['outcome'] for r in receipts]
    failures = [o['status']['Failure'] for o in outcomes if 'Failure' in o['status']]
    if expected['status'] == 'Failure':
        require(failures, 'missing failed receipt')
        for failure in [result['status']['Failure']] + failures:
            require(expected['error'] in json.dumps(failure), 'wrong rejection reason')
    else:
        require(not failures, 'downstream receipt failed')
        require(all(set(o['status']) <= {'SuccessValue', 'SuccessReceiptId'}
                    and o['status'] for o in outcomes), 'incomplete outcome')
    events = []
    payments = []
    for outcome in outcomes:
        if outcome['executor_id'] == expected['receiver']:
            for log in outcome['logs']:
                if log.startswith('EVENT_JSON:'):
                    event = json.loads(log[len('EVENT_JSON:'):])
                    events.append(event)
                    if event.get('event') == 'payment':
                        payments.append(outcome)
    subset(events, expected['events'], 'contract events')
    if 'delivery' in expected:
        # The wallet payment receipt creates the transfer; gas refunds are separate.
        delivered = [r for r in receipts if r['outcome']['executor_id'] == expected['delivery']
                     and 'SuccessValue' in r['outcome']['status']
                     and any(r['id'] in o['receipt_ids'] for o in payments)]
        require(delivered, 'missing successful downstream payment receipt')


def check_decisions(stats, pending, evidence):
    subset(pending, evidence['pending'], 'pending records')
    for decision in evidence['decisions']:
        payment = next(p for p in pending if p['id'] == decision['id'])
        task = next(t for t in stats['tasks']
                    if t['condition']['task_id'] == payment['task_id'])
        condition = task['condition']
        equal(payment['to'], condition['peer'], 'task peer')
        equal(payment['method'], condition['method'], 'task method')
        amount = int(payment['amount'])
        if decision['code'] == 'TASK_LIMIT':
            require(amount > int(condition['max_amount']), 'TASK_LIMIT amount not exceeded')
            equal(payment['status'], 'Vetoed', 'TASK_LIMIT outcome')
        else:
            equal(decision['code'], 'TASK_OK', 'decision code')
            require(amount <= int(condition['max_amount']), 'TASK_OK amount exceeded')
            require(amount <= int(task['spent']) <= int(condition['budget']),
                    'exception spend outside task budget')
            equal(payment['status'], 'Cosigned', 'TASK_OK outcome')


def main():
    if len(sys.argv) > 2 or (len(sys.argv) == 2 and sys.argv[1] in ('-h', '--help')):
        print('Usage: scripts/verify.sh [evidence.json] (Python 3.9+, public testnet RPC only)')
        return 0 if len(sys.argv) == 2 else 1
    path = Path(sys.argv[1]) if len(sys.argv) == 2 else Path(__file__).with_name('evidence.json')
    evidence = json.loads(path.read_text())
    equal(evidence['network'], 'testnet', 'network')
    require(all(evidence[k].endswith('.testnet') for k in ('owner', 'wallet', 'watcher')),
            'all accounts must be testnet')
    deadline = time.monotonic() + VERIFY_SECONDS
    client = RpcClient()
    passed = failed = 0

    def check(name, action):
        nonlocal passed, failed
        try:
            action()
            passed += 1
            print(f'[PASS] {name}', flush=True)
        except Exception as exc:
            failed += 1
            print(f'[FAIL] {name}: {exc}', flush=True)

    # Read sequentially so a single judge run never bursts at public RPCs.
    state = {}

    def state_view(method, args):
        if method not in state:
            state[method] = view(client, evidence, method, args, deadline)
        return state[method]

    if 'wasm_sha256' in evidence:
        check('deployed WASM SHA-256', lambda: check_code(client.call(('query',), {
            'request_type': 'view_code', 'finality': 'final',
            'account_id': evidence['wallet']}, deadline), evidence['wasm_sha256']))
    check('owner', lambda: equal(state_view('get_owner', {}), evidence['owner'], 'owner'))
    check('mode Enforce and learned policy',
          lambda: subset(state_view('get_stats', {}), evidence['stats'], 'get_stats'))
    check('watcher TASK_LIMIT veto / TASK_OK co-sign and task budget',
          lambda: check_decisions(state_view('get_stats', {}),
                                  state_view('get_pending', {'id': None}), evidence))
    for expected in evidence['transactions']:
        check(f"tx {expected['name']}",
              lambda e=expected: check_transaction(client.call(
                  ('EXPERIMENTAL_tx_status', 'tx'), [e['hash'], e['sender']], deadline), e))
    if failed:
        print(f'[FAIL] {failed} of {passed + failed} checks failed')
        return 1
    print(f'[PASS] all {passed} checks')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError) as error:
        print(f'[FAIL] evidence: {error}', file=sys.stderr)
        sys.exit(1)
