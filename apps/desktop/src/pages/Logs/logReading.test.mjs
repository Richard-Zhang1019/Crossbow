import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LogReadingWindow } from './logReading.ts';

test('hover keeps the reading target stable even as live history rolls over', () => {
  const view = new LogReadingWindow();
  const initial = [{text:'first'}, {text:'second'}];
  const live = [initial[1], {text:'new'}];
  assert.equal(view.hold('pointer', true, initial), initial);
  assert.equal(view.hold('pointer', true, live), initial);
  assert.equal(view.hold('pointer', false, live), null);
  assert.equal(view.hold('pointer', true, live), live);
});

test('leaving the mouse does not release a keyboard copy target; clearing removes old rows', () => {
  const view = new LogReadingWindow();
  const rows = [{}];
  view.hold('pointer', true, rows);
  view.hold('focus', true, rows);
  assert.equal(view.hold('pointer', false, []), rows);
  assert.deepEqual(view.clear(), []);
  assert.deepEqual(view.hold('focus', true, rows), []);
  assert.equal(view.hold('focus', false, rows), null);
});

test('identical log text has distinct keys and retained rows keep their key after trimming', () => {
  const view = new LogReadingWindow();
  const first = {text:'same'}, second = {text:'same'};
  const key = view.key(second);
  assert.notEqual(view.key(first), key);
  assert.equal(view.key(second), key);
});
