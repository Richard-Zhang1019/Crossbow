import { test } from 'node:test';
import assert from 'node:assert/strict';
import { faultProbeText, faultEvidenceText } from './report.ts';

const ok = { class: 'ok', httpStatus: 204 };
const fault = {
  mode: 'sysproxy', startedAtMs: 1700000000000, resolvedAtMs: null,
  profileId: 'secret-profile', mixedPort: 7898, mixedPortOk: true,
  sysproxyObserved: ['private-service → token:password@host:7898'],
  controller: ok, directPath: 'unverified',
  directCn: { primary: ok, review: null }, directGlobal: { primary: ok, review: null },
  proxyHttps: { primary: { class: 'http_status', httpStatus: 403 }, review: ok },
};
test('fault summary preserves site limitation and independent success without sensitive context', () => {
  const summary = faultEvidenceText(fault);
  assert.match(summary, /primary=http_status 403; review=ok/);
  assert.match(summary, /physical path unverified/);
  assert.match(summary, /node unknown/);
  assert.match(summary, /Recovery: unconfirmed/);
  for (const sensitive of ['secret-profile', 'private-service', 'token', 'password', '@host']) {
    assert.equal(summary.includes(sensitive), false);
  }
});
test('unknown categories remain unknown and recovery timestamp is exported', () => {
  assert.equal(faultProbeText({ class: 'sensitive-url', httpStatus: null }, key => key), 'fault.class.unknown');
  const summary = faultEvidenceText({ ...fault, resolvedAtMs: 1700000001000 });
  assert.match(summary, /Recovery: 2023-11-14T22:13:21.000Z/);
});
