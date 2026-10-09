import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tunnelStatus, type TunnelInfo } from './tunnel.ts';

const base: TunnelInfo = { provider: 'localhost.run', url: null, up_secs: null, reconnects: 0, last_error: null, expires: false };

test('a tunnel without an address says it is connecting, with the last error if there is one', () => {
  assert.equal(tunnelStatus(base), 'Connecting…');
  assert.equal(tunnelStatus({ ...base, last_error: 'ssh exited' }), 'Connecting… (ssh exited)');
});

test('a connected tunnel names its service and warns when the address will change', () => {
  const up = { ...base, url: 'https://x.lhr.life' };
  assert.equal(tunnelStatus(up), 'Via localhost.run');
  assert.match(tunnelStatus({ ...up, provider: 'pinggy', expires: true }), /changes$/);
});
