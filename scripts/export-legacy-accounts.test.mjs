import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

test('exports ciphertext without downstream keys; rejects duplicate UIDs and proxy bindings', () => {
  const dir = mkdtempSync(join(tmpdir(), 'relayai-export-test-'));
  const database = join(dir, 'state.db');
  const config = join(dir, 'config.json');
  const out = join(dir, 'accounts.json');
  const script = fileURLToPath(new URL('./export-legacy-accounts.mjs', import.meta.url));
  const db = new DatabaseSync(database);
  try {
    db.exec(`CREATE TABLE accounts (channel_id TEXT, uid TEXT, nickname TEXT, enabled INTEGER,
      priority_override INTEGER, weight_override INTEGER, max_inflight INTEGER, proxy_slot TEXT,
      credential TEXT, added_at INTEGER, source TEXT)`);
    db.prepare('INSERT INTO accounts VALUES (?,?,?,?,?,?,?,?,?,?,?)')
      .run('wb-cn', 'test-uid', '测试账号', 1, null, null, 2, null, 'encrypted-fixture', 1700000000000, 'oauth');
    writeFileSync(config, JSON.stringify({ api_keys: ['must-not-export'], channels: [{ id: 'wb-cn',
      display_name: '国内版', adapter: 'workbuddy', enabled: true, priority: 100, weight: 1,
      options: { realm: 'cn', outbound_identity: 'workbuddy', unexpected_secret: 'must-not-export-either' } }] }));
    const before = readFileSync(database);
    const run = () => spawnSync(process.execPath, [script, '--database', database, '--config', config, '--out', out], { encoding: 'utf8' });
    const result = run();
    assert.equal(result.status, 0, result.stderr);
    const text = readFileSync(out, 'utf8');
    assert.ok(!text.includes('must-not-export'));
    assert.equal(JSON.parse(text).accounts[0].credential, 'encrypted-fixture');
    assert.equal(JSON.parse(text).accounts[0].nickname, '测试账号');
    assert.deepEqual(readFileSync(database), before);
    assert.notEqual(run().status, 0, 'must refuse overwriting an export');
    rmSync(out);
    db.exec("INSERT INTO accounts SELECT 'other-channel', uid, nickname, enabled, priority_override, weight_override, max_inflight, proxy_slot, credential, added_at, source FROM accounts");
    assert.notEqual(run().status, 0);
    assert.equal(existsSync(out), false);
    db.exec("DELETE FROM accounts WHERE channel_id='other-channel'; UPDATE accounts SET proxy_slot='proxy'");
    assert.notEqual(run().status, 0);
    assert.equal(existsSync(out), false);
  } finally {
    db.close();
    rmSync(dir, { recursive: true });
  }
});
