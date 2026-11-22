import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DNS_HISTORY_LIMIT, pathStateId, probeSummary, pushHistory, verdictId, verdictTone } from './dnsReport.ts';

const path = (over) => ({ state: 'ok', ms: 12, rcode: 0, answers: ['192.0.2.1'], ttl: 60, note: null, ...over });
const probe = (over = {}) => ({
  domain: 'www.example.com', record_type: 'A', at_ms: 1700000000000, engine: 'mihomo',
  core_running: true, core_pid: 4242, core_version: '1.19.32', dns_mode: 'custom',
  system: path({ state: 'ok' }), kernel: path({ state: 'nxdomain', rcode: 3, answers: [], ttl: null }),
  verdict: 'system_only', hosts_override: false, unknown_fields: ['upstream', 'cache', 'kernel_time'],
  ...over,
});

test('verdict tone never reports an unobserved path as passed', () => {
  assert.equal(verdictTone('both_ok'), 'ok');
  assert.equal(verdictTone('system_only'), 'bad');
  assert.equal(verdictTone('kernel_unavailable'), 'warn');
  assert.equal(verdictTone('nonsense'), 'bad');
  assert.equal(pathStateId('weird'), 'error');
  assert.equal(verdictId('nonsense'), 'both_fail');
});

test('summary carries the facts and leaves unknown fields visible', () => {
  const text = probeSummary(probe());
  assert.match(text, /Domain: www\.example\.com A/);
  assert.match(text, /System: ok 12ms rcode=0 answers=192\.0\.2\.1 ttl=60/);
  assert.match(text, /Kernel: nxdomain 12ms rcode=3/);
  assert.match(text, /Verdict: system_only/);
  assert.match(text, /Unknown fields: upstream, cache, kernel_time/);
  assert.doesNotMatch(text, /secret|token|password/i);
});

test('hosts override is stated explicitly instead of being inferred from answers', () => {
  assert.match(probeSummary(probe({ hosts_override: true })), /Hosts override: yes/);
  assert.doesNotMatch(probeSummary(probe()), /Hosts override/);
});

test('history stays bounded and keeps the newest entry first', () => {
  let history = [];
  for (let i = 0; i < DNS_HISTORY_LIMIT + 3; i++) {
    history = pushHistory(history, { probe: probe({ domain: `h${i}.example` }), revision: 1 });
  }
  assert.equal(history.length, DNS_HISTORY_LIMIT);
  assert.equal(history[0].probe.domain, `h${DNS_HISTORY_LIMIT + 2}.example`);
  assert.equal(pushHistory([], { probe: probe(), revision: 0 }).length, 1);
});
