// Export encrypted accounts only; never copy config.api_keys or plaintext tokens.
import { DatabaseSync } from 'node:sqlite';
import { readFileSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

const { values } = parseArgs({ options: {
  database: { type: 'string' }, config: { type: 'string' }, out: { type: 'string' },
} });
if (!values.database || !values.config || !values.out) {
  throw new Error('Usage: node scripts/export-legacy-accounts.mjs --database state.db --config config.json --out accounts.json');
}
const config = JSON.parse(readFileSync(values.config, 'utf8'));
const db = new DatabaseSync(values.database, { readOnly: true });
try {
  if (db.prepare('PRAGMA quick_check').get().quick_check !== 'ok') {
    throw new Error('Source database integrity check failed');
  }
  const accounts = db.prepare(`SELECT channel_id, uid, nickname, enabled,
    priority_override, weight_override, max_inflight, proxy_slot,
    credential, added_at, source FROM accounts ORDER BY channel_id, uid`).all();
  const usedChannels = new Set(accounts.map(account => account.channel_id));
  const channels = (config.channels ?? []).filter(channel => usedChannels.has(channel.id)).map(channel => {
    if (channel.adapter !== 'workbuddy') throw new Error('Only WorkBuddy migration is supported');
    const { realm, outbound_identity } = channel.options ?? {};
    if (!['cn', 'intl'].includes(realm) || !['workbuddy', 'codebuddy'].includes(outbound_identity)) {
      throw new Error('Unsupported WorkBuddy identity in source config');
    }
    return { id: channel.id, name: channel.display_name || channel.id,
      adapter: channel.adapter, enabled: channel.enabled, priority: channel.priority,
      weight: channel.weight, options: { realm, outbound_identity } };
  });
  if (channels.length !== usedChannels.size || new Set(channels.map(channel => channel.id)).size !== channels.length) {
    throw new Error('Missing or duplicate source channel configuration');
  }
  if (accounts.some(account => account.proxy_slot)) throw new Error('Proxy-bound accounts require an explicit proxy migration');
  if (new Set(accounts.map(account => account.uid)).size !== accounts.length) {
    throw new Error('Duplicate source UIDs require manual resolution');
  }
  const bundle = { version: 1, channels, accounts: accounts.map(({ proxy_slot, ...account }) => account) };
  // Exclusive creation prevents accidentally replacing a previous export.
  writeFileSync(values.out, JSON.stringify(bundle, null, 2) + '\n', { encoding: 'utf8', mode: 0o600, flag: 'wx' });
  console.log(JSON.stringify({ accounts: accounts.length, channels: channels.map(channel => channel.id), credentials: 'source ciphertext only' }));
} finally {
  db.close();
}
