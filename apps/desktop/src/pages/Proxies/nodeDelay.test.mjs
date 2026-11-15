import assert from 'node:assert/strict';
import { test } from 'node:test';
import { comparableNodeDelay, nodeDelayValue, runNodeDelayQueue } from './nodeDelay.ts';

const success = { class: 'ok', delayMs: 20, reviewed: false, target: '', testedAtMs: 0, sampleCount: 1, detail: '' };
const nodes = Array.from({ length: 24 }, (_, i) => String(i));

test('configuration invalidation or cancellation fences in-flight work and stops dispatch', async () => {
  for (const invalidatedByBackend of [true, false]) {
    const requests = [];
    const applied = [];
    let current = true;
    const task = runNodeDelayQueue(nodes, Infinity, () => current,
      node => new Promise(resolve => requests.push({ node, resolve })),
      node => applied.push(node));
    assert.equal(requests.length, 10);
    if (invalidatedByBackend) {
      requests[0].resolve({ ...success, class: 'cancelled' });
      await Promise.resolve();
    } else {
      current = false;
    }
    for (const request of requests) request.resolve(success);
    assert.equal(await task, invalidatedByBackend);
    assert.equal(requests.length, 10);
    assert.deepEqual(applied, []);
  }
});

test('the batch deadline prevents new requests without discarding an accepted sample', async t => {
  let now = 0;
  t.mock.method(performance, 'now', () => now);
  const requests = [];
  const applied = [];
  const task = runNodeDelayQueue(nodes, 10, () => true,
    () => new Promise(resolve => requests.push(resolve)), node => applied.push(node));
  assert.equal(requests.length, 10);
  now = 10;
  for (const resolve of requests) resolve(success);
  await task;
  assert.equal(requests.length, 10);
  assert.equal(applied.length, 10);
});

test('alternate-target latency does not enter primary sorting and errors retain the previous value', () => {
  const node = { name: 'n', kind: 'HTTP', alive: true, delay_ms: 180 };
  const reviewed = { ...success, reviewed: true };
  assert.equal(nodeDelayValue(node, reviewed), 20);
  assert.ok(comparableNodeDelay(node, reviewed) > comparableNodeDelay(node));
  assert.equal(nodeDelayValue(node, { ...success, class: 'controller', previousMs: 72 }), 72);
  assert.equal(nodeDelayValue(node, { ...success, class: 'cancelled', previousMs: null }), null);
  assert.equal(nodeDelayValue(node, { ...success, class: 'timeout' }), 0);
});
