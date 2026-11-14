"""Failure-path tests for the read-only evidence verifier (stdlib only)."""
import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import time
import unittest
import urllib.error
from unittest.mock import patch

import verify


class VerificationTests(unittest.TestCase):
    def test_deployed_code_requires_exact_sha256(self):
        code = b'contract wasm'
        result = {'code_base64': base64.b64encode(code).decode()}
        verify.check_code(result, hashlib.sha256(code).hexdigest())
        with self.assertRaisesRegex(ValueError, 'deployed WASM SHA-256'):
            verify.check_code(result, hashlib.sha256(b'older contract').hexdigest())
        with self.assertRaises(ValueError):
            verify.check_code({'code_base64': '!!!'}, hashlib.sha256(code).hexdigest())

    def setUp(self):
        event = {'standard': 'agent-wallet', 'event': 'payment',
                 'data': [{'rule': 'Cosigned', 'amount': '2'}]}
        self.expected = {'hash': 'hash', 'sender': 'watcher.testnet',
                         'receiver': 'wallet.testnet', 'status': 'SuccessValue',
                         'actions': [{'type': 'FunctionCall', 'method': 'cosign',
                                      'args': {'id': '2'}}],
                         'events': [event], 'delivery': 'peer.testnet'}
        outcome = {'executor_id': 'wallet.testnet', 'status': {'SuccessReceiptId': 'pay'},
                   'logs': ['EVENT_JSON:' + json.dumps(event, separators=(',', ':'))],
                   'receipt_ids': ['pay']}
        self.result = {'final_execution_status': 'FINAL', 'status': {'SuccessValue': ''},
                       'transaction': {'hash': 'hash', 'signer_id': 'watcher.testnet',
                                       'receiver_id': 'wallet.testnet', 'actions': [
                                           {'FunctionCall': {'method_name': 'cosign',
                                            'args': base64.b64encode(b'{"id":"2"}').decode()}}]},
                       'transaction_outcome': {'outcome': {
                           'executor_id': 'watcher.testnet', 'logs': [],
                           'status': {'SuccessReceiptId': 'call'}, 'receipt_ids': ['call']}},
                       'receipts_outcome': [{'id': 'call', 'outcome': outcome}, {
                           'id': 'pay', 'outcome': {'executor_id': 'peer.testnet',
                               'status': {'SuccessValue': ''}, 'logs': [], 'receipt_ids': []}}]}

    def test_success_requires_finality_and_downstream_success(self):
        verify.check_transaction(self.result, self.expected)
        for mutate in (
            lambda r: r.update(final_execution_status='EXECUTED_OPTIMISTIC'),
            lambda r: r['receipts_outcome'].pop(),
            lambda r: r['receipts_outcome'][1]['outcome'].update(status={'Failure': 'oops'}),
            lambda r: r['receipts_outcome'][0]['outcome'].update(logs=[]),
            lambda r: r['transaction'].update(signer_id='wrong.testnet'),
        ):
            with self.subTest(mutate=mutate):
                broken = copy.deepcopy(self.result)
                mutate(broken)
                with self.assertRaises(ValueError):
                    verify.check_transaction(broken, self.expected)

    def test_rejection_requires_exact_reason_in_status_and_receipt(self):
        self.expected.pop('delivery')
        self.expected.update(status='Failure', error='ERR_NOT_IN_LEARNED_POLICY')
        error = {'ExecutionError': 'ERR_NOT_IN_LEARNED_POLICY'}
        self.result['status'] = {'Failure': error}
        self.result['receipts_outcome'][0]['outcome']['status'] = {'Failure': error}
        verify.check_transaction(self.result, self.expected)
        self.result['receipts_outcome'][0]['outcome']['status'] = {'Failure': 'another error'}
        with self.assertRaises(ValueError):
            verify.check_transaction(self.result, self.expected)

    def test_policy_extra_rows_are_rejected(self):
        with self.assertRaises(ValueError):
            verify.subset({'rows': [1, 2]}, {'rows': [1]}, 'policy')

    def test_rpc_falls_back_for_invalid_json_and_json_rpc_errors(self):
        success = json.dumps({'result': {'ok': True}}).encode()
        for first in (io.BytesIO(b'{"error":{"code":-1}}'), io.BytesIO(b'bad json')):
            with self.subTest(first=first), patch.object(verify, 'PACE_SECONDS', 0), \
                    patch.object(verify.urllib.request, 'urlopen',
                    side_effect=[first, io.BytesIO(success)]) as request:
                self.assertEqual(verify.RpcClient().call(('query',), {}, time.monotonic() + 5), {'ok': True})
                self.assertEqual(request.call_args_list[1].args[0].full_url, verify.RPCS[1])

    def test_transient_failures_retry_then_recover(self):
        for error in (TimeoutError('slow'), urllib.error.URLError('offline'),
                      urllib.error.HTTPError(verify.RPCS[0], 429, 'limited', {}, None),
                      urllib.error.HTTPError(verify.RPCS[0], 503, 'busy', {}, None)):
            with self.subTest(error=error), patch.object(verify.time, 'sleep') as sleep, \
                    patch.object(verify.urllib.request, 'urlopen', side_effect=[
                        error, error, io.BytesIO(b'{"result":true}')]) as request:
                client = verify.RpcClient()
                self.assertTrue(client.call(('query',), {}, time.monotonic() + 10))
                self.assertEqual(request.call_count, 3)
                self.assertTrue(all(c.args[0].full_url == verify.RPCS[0]
                                    for c in request.call_args_list))
                delays = [c.args[0] for c in sleep.call_args_list]
                self.assertIn(0.5, delays)
                self.assertIn(1.0, delays)
                self.assertFalse(client.unavailable)

    def test_exhausted_endpoint_is_skipped_on_later_calls(self):
        with patch.object(verify.time, 'sleep'), patch.object(
                verify.urllib.request, 'urlopen', side_effect=[
                    TimeoutError('slow')] * 3 + [io.BytesIO(b'{"result":1}'),
                                                io.BytesIO(b'{"result":2}')]) as request:
            client = verify.RpcClient()
            self.assertEqual(client.call(('query',), {}, time.monotonic() + 10), 1)
            self.assertEqual(client.call(('query',), {}, time.monotonic() + 10), 2)
            self.assertEqual([c.args[0].full_url for c in request.call_args_list],
                             [verify.RPCS[0]] * 3 + [verify.RPCS[1]] * 2)

    def test_pruned_transaction_falls_back_without_transport_retry(self):
        with patch.object(verify.time, 'sleep'), patch.object(
                verify.urllib.request, 'urlopen', side_effect=[
                    io.BytesIO(b'{"error":{"name":"UNKNOWN_TRANSACTION"}}'),
                    io.BytesIO(b'{"error":{"name":"UNKNOWN_TRANSACTION"}}'),
                    io.BytesIO(b'{"result":true}')]) as request:
            self.assertTrue(verify.RpcClient().call(
                ('EXPERIMENTAL_tx_status', 'tx'), [], time.monotonic() + 10))
            self.assertEqual([json.loads(c.args[0].data)['method']
                              for c in request.call_args_list],
                             ['EXPERIMENTAL_tx_status', 'tx', 'EXPERIMENTAL_tx_status'])
            self.assertEqual(request.call_args.args[0].full_url, verify.RPCS[1])

    def test_pacing_applies_between_successful_calls(self):
        with patch.object(verify.time, 'monotonic', return_value=100), \
                patch.object(verify.time, 'sleep') as sleep, patch.object(
                    verify.urllib.request, 'urlopen', side_effect=[
                        io.BytesIO(b'{"result":1}'), io.BytesIO(b'{"result":2}')]):
            client = verify.RpcClient()
            client.call(('query',), {}, 110)
            client.call(('query',), {}, 110)
            sleep.assert_called_once_with(verify.PACE_SECONDS)

    def test_all_endpoints_failing_is_an_error(self):
        with patch.object(verify.time, 'sleep'), patch.object(
                verify.urllib.request, 'urlopen', side_effect=TimeoutError('slow')) as request:
            with self.assertRaisesRegex(ValueError, 'slow'):
                verify.RpcClient().call(('query',), {}, time.monotonic() + 10)
            self.assertEqual(request.call_count, len(verify.RPCS) * 3)

    def test_deadline_prevents_requests(self):
        with patch.object(verify.urllib.request, 'urlopen') as request:
            with self.assertRaisesRegex(ValueError, 'deadline'):
                verify.RpcClient().call(('query',), {}, time.monotonic() - 1)
            request.assert_not_called()

    def test_task_limit_reason_is_verified_from_amount(self):
        evidence = json.loads(Path(__file__).with_name('evidence.json').read_text())
        stats, pending = evidence['stats'], evidence['pending']
        verify.check_decisions(stats, pending, evidence)
        pending[0]['amount'] = stats['tasks'][0]['condition']['max_amount']
        evidence['pending'] = copy.deepcopy(pending)
        with self.assertRaisesRegex(ValueError, 'TASK_LIMIT amount not exceeded'):
            verify.check_decisions(stats, pending, evidence)


if __name__ == '__main__':
    # TAP provides portable, readable output for these standard-library tests.
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(VerificationTests)
    result = unittest.TestResult()
    suite.run(result)
    print('TAP version 13')
    print(f'1..{result.testsRun}')
    problems = {str(test): detail for test, detail in result.failures + result.errors}
    for i, name in enumerate(unittest.defaultTestLoader.getTestCaseNames(VerificationTests), 1):
        detail = next((v for k, v in problems.items() if k.startswith(name + ' ')), None)
        print(f'{"not ok" if detail else "ok"} {i} - {name}')
        if detail:
            print('\n'.join('# ' + line for line in detail.splitlines()))
    raise SystemExit(not result.wasSuccessful())
