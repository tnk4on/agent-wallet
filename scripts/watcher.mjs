#!/usr/bin/env node
// Mechanical testnet watcher. The only decisions are exact comparisons and integer arithmetic.
import { readFileSync, writeFileSync, mkdirSync, existsSync, realpathSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

// Returns fixed result codes. Strings are compared as identifiers, never interpreted.
export function decide(payment, tasks, now) {
  try {
    if (payment.status !== 'Pending') return { method: 'none', code: 'NOT_PENDING' };
    if (BigInt(payment.expires_at) <= now) return { method: 'expire', code: 'EXPIRED' };
    const task = tasks.find(t => t.condition.task_id === payment.task_id);
    if (!task || !/^[A-Za-z0-9+/]{43}=$/.test(task.hash)) return { method: 'veto', code: 'NO_TASK' };
    const c = task.condition;
    if (payment.method !== 'Pay' || c.method !== payment.method || c.peer !== payment.to)
      return { method: 'veto', code: 'PEER_OR_METHOD' };
    const amount = BigInt(payment.amount);
    if (BigInt(c.expires_at_ns) <= now || amount <= 0n || amount > BigInt(c.max_amount)
        || amount + BigInt(task.spent) > BigInt(c.budget))
      return { method: 'veto', code: 'TASK_LIMIT' };
    return { method: 'cosign', code: 'TASK_OK' };
  } catch { return { method: 'veto', code: 'INVALID_FIELDS' }; }
}

function sanitize(text) {
  return text.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/ed25519:[1-9A-HJ-NP-Za-km-z]{45,}/g, '[REDACTED]')
    .split('\n').filter(line => !/private[_ -]?key|seed[_ -]?phrase|mnemonic|secret recovery/i.test(line)).join('\n');
}
function b58(bytes) {
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  let number = BigInt('0x' + bytes.toString('hex')), output = '';
  while (number) { output = alphabet[Number(number % 58n)] + output; number /= 58n; }
  for (const byte of bytes) { if (byte !== 0) break; output = '1' + output; }
  return output;
}
async function main() {
  process.umask(0o077);
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    if (!process.argv[i].startsWith('--') || !process.argv[i + 1]) throw Error('Expected --name value arguments');
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  for (const name of ['wallet', 'owner', 'watcher', 'rpc-url', 'id', 'key-file', 'job-dir'])
    if (!args[name]) throw Error('Missing --' + name);
  for (const name of ['wallet', 'owner', 'watcher'])
    if (!/^[a-z0-9._-]+\.testnet$/.test(args[name])) throw Error('TestnetOnly');
  if (!/^\d+$/.test(args.id) || new URL(args['rpc-url']).protocol !== 'https:') throw Error('Invalid input');
  const repo = realpathSync(join(dirname(fileURLToPath(import.meta.url)), '..'));
  const keyPath = realpathSync(args['key-file']);
  const job = resolve(args['job-dir']);
  mkdirSync(job, { recursive: true, mode: 0o700 });
  const jobPath = realpathSync(job);
  if (keyPath.startsWith(repo + '/') || jobPath === repo || jobPath.startsWith(repo + '/')) throw Error('Use external credential/evidence storage');
  async function rpc(method, params) {
    for (let i = 0; i < 4; i++) {
      let response;
      try {
        response = await fetch(args['rpc-url'], { method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ jsonrpc: '2.0', id: 'mechanical-watcher', method, params }), signal: AbortSignal.timeout(5000) });
      } catch (error) {
        if (i === 3 || method === 'send_tx') throw error;
        await new Promise(r => setTimeout(r, 500 * (i + 1))); continue;
      }
      if ([408, 429, 500, 502, 503, 504].includes(response.status) && i < 3) {
        await new Promise(r => setTimeout(r, Math.min(8000, 1000 * 2 ** i))); continue;
      }
      if (!response.ok) throw Error('RPC HTTP ' + response.status);
      const data = await response.json();
      return data;
    }
  }
  async function query(method, params) {
    const data = await rpc(method, params);
    if (data.error) throw Error(JSON.stringify(data.error));
    return data.result;
  }
  async function view(method, params = {}) {
    const data = await query('query', { request_type: 'call_function', account_id: args.wallet, method_name: method,
      args_base64: Buffer.from(JSON.stringify(params)).toString('base64'), finality: 'final' });
    return JSON.parse(Buffer.from(data.result).toString());
  }
  async function finalizedTx(hash) {
    const deadline = Date.now() + 60000;
    while (Date.now() < deadline) {
      const data = await rpc('tx', { tx_hash: hash, sender_account_id: args.watcher, wait_until: 'NONE' });
      if (data.error) {
        if (!['UNKNOWN_TRANSACTION', 'TIMEOUT_ERROR'].includes(data.error.cause?.name)) throw Error(JSON.stringify(data.error));
      } else if (data.result.final_execution_status === 'FINAL') return data.result;
      await new Promise(r => setTimeout(r, 250));
    }
    throw Error('Finality timeout; resume the same saved transaction');
  }
  const [chain, owner] = await Promise.all([query('status', []), view('get_owner')]);
  if (chain.chain_id !== 'testnet') throw Error('TestnetOnly');
  if (owner !== args.owner) throw Error('OwnerMismatch');
  const prefix = join(jobPath, 'watcher-' + args.id);
  const outcomeFile = prefix + '.json';
  const signedFile = prefix + '.signed.json';
  function verifyOutcome(outcome) {
    const tx = outcome.transaction;
    const call = tx.actions.length === 1 && tx.actions[0].FunctionCall;
    if (tx.signer_id !== args.watcher || tx.receiver_id !== args.wallet || !call
        || !['cosign', 'veto'].includes(call.method_name)
        || JSON.parse(Buffer.from(call.args, 'base64').toString()).id !== args.id)
      throw Error('Watcher transaction identity mismatch');
    if (!Object.hasOwn(outcome.status, 'SuccessValue')
        || outcome.receipts_outcome.some(r => Object.hasOwn(r.outcome.status, 'Failure')))
      throw Error('Watcher transaction or downstream receipt failed');
  }
  function signedHash(saved) {
    const bytes = Buffer.from(saved.signed_transaction_as_base64, 'base64');
    if (bytes.length < 66 || bytes[bytes.length - 65] !== 0) throw Error('Expected Ed25519 signed transaction');
    return b58(createHash('sha256').update(bytes.subarray(0, -65)).digest());
  }
  if (existsSync(outcomeFile)) {
    const saved = JSON.parse(readFileSync(outcomeFile, 'utf8'));
    const checked = await finalizedTx(saved.transaction.hash);
    verifyOutcome(checked);
    console.log(JSON.stringify({ id: args.id, result: 'REPLAY_VERIFIED', hash: checked.transaction.hash })); return;
  }
  // Recover an accepted transaction even if the process stopped before persisting its outcome.
  // Check this before reading the now-terminal pending status.
  if (existsSync(signedFile)) {
    const hash = signedHash(JSON.parse(readFileSync(signedFile, 'utf8')));
    const known = await rpc('tx', { tx_hash: hash, sender_account_id: args.watcher, wait_until: 'NONE' });
    if (!known.error) {
      const checked = await finalizedTx(hash);
      verifyOutcome(checked);
      writeFileSync(outcomeFile, JSON.stringify(checked, null, 2) + '\n', { mode: 0o600 });
      console.log(JSON.stringify({ id: args.id, result: 'RECOVERED', hash })); return;
    }
    if (known.error.cause?.name !== 'UNKNOWN_TRANSACTION') throw Error(JSON.stringify(known.error));
  }
  const key = JSON.parse(readFileSync(keyPath, 'utf8'));
  const [records, stats, block, native] = await Promise.all([
    view('get_pending', { id: args.id }), view('get_stats'), query('block', { finality: 'final' }),
    query('query', { request_type: 'view_access_key', account_id: args.watcher, public_key: key.public_key, finality: 'final' }),
  ]);
  if (records.length !== 1) throw Error('Unknown payment');
  if (stats.watcher !== args.watcher) throw Error('WatcherMismatch');
  const decision = decide(records[0], stats.tasks, BigInt(block.header.timestamp_nanosec));
  if (decision.method === 'none') throw Error(decision.code);
  // Permissionless expiry is sent by the demo owner; the watcher's native key has only cosign/veto.
  if (decision.method === 'expire') throw Error('EXPIRED: call expire with a separate account');
  const permission = native.permission.FunctionCall;
  if (!permission || permission.receiver_id !== args.wallet || permission.allowance === null
      || BigInt(permission.allowance) > 2n * 10n ** 23n
      || [...permission.method_names].sort().join(',') !== 'cosign,veto') throw Error('WatcherKeyScope');
  const intentFile = prefix + '.intent.json';
  if (existsSync(intentFile)) {
    const saved = JSON.parse(readFileSync(intentFile, 'utf8'));
    if (saved.id !== args.id || saved.method !== decision.method) throw Error('Context changed; refusing to replace signed transaction');
  } else {
    if (existsSync(signedFile)) throw Error('Signed transaction is missing its intent record');
    writeFileSync(intentFile, JSON.stringify({ id: args.id, ...decision }) + '\n', { mode: 0o600 });
  }
  if (!existsSync(signedFile)) {
    const cli = spawnSync('near', ['--offline', 'contract', 'call-function', 'as-transaction', args.wallet, decision.method,
      'json-args', JSON.stringify({ id: args.id }), 'prepaid-gas', '10 Tgas', 'attached-deposit', '0 NEAR',
      'sign-as', args.watcher, 'network-config', 'testnet', 'sign-with-access-key-file', keyPath,
      '--nonce', String(BigInt(native.nonce) + 1n), '--block-hash', native.block_hash, '--block-height', String(native.block_height),
      'save-to-file', signedFile], { encoding: 'utf8', timeout: 90000, maxBuffer: 4 * 1024 * 1024 });
    writeFileSync(prefix + '.log', sanitize(cli.stdout + cli.stderr), { mode: 0o600 });
    if (cli.status !== 0) throw Error('Watcher signing failed; see sanitized watcher log');
  }
  const signed = JSON.parse(readFileSync(signedFile, 'utf8'));
  const hash = signedHash(signed);
  let sent;
  try { sent = await rpc('send_tx', { signed_tx_base64: signed.signed_transaction_as_base64, wait_until: 'INCLUDED' }); }
  catch { sent = {}; } // An ambiguous send is resolved only by the persisted hash.
  if (sent.error && sent.error.cause?.name !== 'TIMEOUT_ERROR') throw Error(JSON.stringify(sent.error));
  const outcome = await finalizedTx(hash);
  writeFileSync(outcomeFile, JSON.stringify(outcome, null, 2) + '\n', { mode: 0o600 });
  verifyOutcome(outcome);
  writeFileSync(prefix + '.decision.json', JSON.stringify({ id: args.id, method: decision.method, code: decision.code, hash }, null, 2) + '\n', { mode: 0o600 });
  console.log(JSON.stringify({ id: args.id, ...decision, hash }));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch(e => { console.error(sanitize(e.message)); process.exitCode = 1; });
