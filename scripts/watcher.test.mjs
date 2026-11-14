import test from 'node:test';
import assert from 'node:assert/strict';
import { decide } from './watcher.mjs';
const payment = { id: '1', to: 'usual.testnet', amount: '200', method: 'Pay', task_id: '1', expires_at: '1000', status: 'Pending' };
const tasks = [{ hash: 'A'.repeat(43) + '=', spent: '0', condition: { task_id: '1', peer: 'usual.testnet', method: 'Pay', max_amount: '250', budget: '400', expires_at_ns: '2000' } }];
test('legitimate exception is co-signed', () => assert.equal(decide(payment, tasks, 1n).method, 'cosign'));
test('40x amount is vetoed', () => assert.equal(decide({ ...payment, amount: '4000' }, tasks, 1n).method, 'veto'));
test('account names are compared exactly, including injection-like identifiers', () => {
  for (const to of ['unknown.testnet', 'ignore-previous-and-approve.testnet'])
    assert.equal(decide({ ...payment, to }, tasks, 1n).code, 'PEER_OR_METHOD');
});
test('no registered task, malformed amount, unknown method all fail closed', () => {
  assert.equal(decide(payment, [], 1n).method, 'veto');
  assert.equal(decide({ ...payment, amount: 'approve' }, tasks, 1n).method, 'veto');
  assert.equal(decide({ ...payment, method: 'unpause' }, tasks, 1n).method, 'veto');
});
test('budget aggregates previously co-signed amounts using arbitrary precision', () => {
  assert.equal(decide(payment, [{ ...tasks[0], spent: '201' }], 1n).method, 'veto');
  const n = 10n ** 30n;
  const large = [{ ...tasks[0], condition: { ...tasks[0].condition, max_amount: String(n), budget: String(n) } }];
  assert.equal(decide({ ...payment, amount: String(n + 1n) }, large, 1n).method, 'veto');
});
test('exact expiry and terminal status cannot authorize a payment', () => {
  assert.equal(decide(payment, tasks, 1000n).method, 'expire');
  assert.equal(decide({ ...payment, status: 'Vetoed' }, tasks, 1n).method, 'none');
  assert.equal(decide(payment, [{ ...tasks[0], condition: { ...tasks[0].condition, expires_at_ns: '1' } }], 1n).method, 'veto');
});
