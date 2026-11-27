import assert from 'node:assert/strict';
import { test } from 'node:test';
import { CORE_RESTARTING_MARKER, isCoreRestartingError } from './restart.ts';

test('recognises the core restart window marker', () => {
  assert.equal(isCoreRestartingError(`${CORE_RESTARTING_MARKER}: Connection refused`), true);
  assert.equal(isCoreRestartingError(new Error(`retry ${CORE_RESTARTING_MARKER} later`)), true);
  assert.equal(isCoreRestartingError('Connection Failed: Connection refused'), false);
  assert.equal(isCoreRestartingError(null), false);
  assert.equal(CORE_RESTARTING_MARKER, 'core:restarting');
});
