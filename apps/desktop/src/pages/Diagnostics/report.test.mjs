import { test } from 'node:test';
import assert from 'node:assert/strict';
import { faultProbeText, faultEvidenceState, faultEvidenceText, faultLayerStats, dedupeObservedTargets } from './report.ts';

const ok = { class: 'ok', httpStatus: 204 };
const fault = {
  mode: 'sysproxy', startedAtMs: 1700000000000, resolvedAtMs: null,
  recovery: null, superseded: false, legacy: false,
  profileId: 'secret-profile', mixedPort: 7898, mixedPortOk: true,
  sysproxyObserved: ['private-service → token:password@host:7898'],
  controller: ok, directPath: 'unverified',
  directCn: { primary: ok, review: null }, directGlobal: { primary: ok, review: null },
  proxyHttps: { primary: { class: 'timeout', httpStatus: null }, review: { class: 'tls', httpStatus: null } },
};
test('fault summary separates the failed samples from the timestamped recovery probe', () => {
  const recovered = {
    ...fault, resolvedAtMs: 1700000001000,
    recovery: { atMs: 1700000001000, proxyHttps: { primary: ok, review: null } },
  };
  const summary = faultEvidenceText(recovered);
  assert.match(summary, /Proxy HTTPS: primary=timeout; review=tls/);
  assert.match(summary, /State: recovered \(verified\)/);
  assert.match(summary, /Recovery: 2023-11-14T22:13:21.000Z\nRecovery probe: primary=ok 204/);
  assert.match(summary, /physical path unverified/);
  assert.match(summary, /node unknown/);
  for (const sensitive of ['secret-profile', 'private-service', 'token', 'password', '@host']) {
    assert.equal(summary.includes(sensitive), false);
  }
});
test('a successful independent check at capture preserves the primary HTTP status', () => {
  for (const status of [403, 429]) {
    const limited = {
      ...fault, resolvedAtMs: 1700000001000,
      proxyHttps: { primary: { class: 'http_status', httpStatus: status }, review: ok },
    };
    assert.equal(faultEvidenceState(limited), 'reachable');
    const summary = faultEvidenceText(limited);
    assert.match(summary, /State: reachable during initial observation/);
    assert.ok(summary.includes(`Proxy HTTPS: primary=http_status ${status}; review=ok 204`));
    assert.doesNotMatch(summary, /Recovery probe:/);
  }
});
test('unknown categories remain unknown and historical states explain the evidence available', () => {
  assert.equal(faultProbeText({ class: 'sensitive-url', httpStatus: null }, key => key), 'fault.class.unknown');
  assert.match(faultEvidenceText(fault), /State: ongoing/);
  const superseded = faultEvidenceText({ ...fault, superseded: true });
  assert.match(superseded, /State: context changed; recovery unconfirmed/);
  assert.doesNotMatch(superseded, /Recovery probe:/);
  const legacy = faultEvidenceText({ ...fault, legacy: true, resolvedAtMs: 1700000001000 });
  assert.match(legacy, /State: recovery recorded in legacy format/);
  assert.match(legacy, /Legacy recovery timestamp: 2023-11-14T22:13:21.000Z/);
  assert.match(legacy, /Note: legacy fault-time proxy samples may have been overwritten/);
});
test('layer stats count each of the five probe layers once', () => {
  assert.deepEqual(faultLayerStats(fault), { total: 5, failed: 1 });
  const allOk = {
    ...fault,
    proxyHttps: { primary: ok, review: ok },
  };
  assert.deepEqual(faultLayerStats(allOk), { total: 5, failed: 0 });
  const mixedPortDown = { ...fault, mixedPortOk: false };
  assert.deepEqual(faultLayerStats(mixedPortDown), { total: 5, failed: 2 });
  const reviewFails = {
    ...fault,
    directCn: { primary: ok, review: { class: 'dns', httpStatus: null } },
  };
  assert.deepEqual(faultLayerStats(reviewFails), { total: 5, failed: 2 });
});
test('observed proxies dedupe by target when every interface shares one endpoint', () => {
  const grouped = dedupeObservedTargets([
    'Wi-Fi → 127.0.0.1:7898', 'Wi-Fi → 127.0.0.1:7898', 'Thunderbolt Bridge → 127.0.0.1:7898',
  ]);
  assert.deepEqual(grouped, [
    { iface: 'Wi-Fi', target: '127.0.0.1:7898', count: 2 },
    { iface: 'Thunderbolt Bridge', target: '127.0.0.1:7898', count: 1 },
  ]);
  assert.equal(dedupeObservedTargets(['Wi-Fi → 127.0.0.1:7898', 'Wi-Fi → 10.0.0.2:7898']), null);
  assert.equal(dedupeObservedTargets(['managed-network']), null);
  assert.equal(dedupeObservedTargets([]), null);
});
