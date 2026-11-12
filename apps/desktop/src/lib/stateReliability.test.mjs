import {test} from 'node:test';
import assert from 'node:assert/strict';
import {cumulativeTotals} from './stateReliability.ts';
test('overview reads cumulative core counters rather than integrating rate samples', () => {
 assert.deepEqual(cumulativeTotals({upload_total:123456,download_total:789012}),{up:123456,down:789012});
 // A remounted overview receives the same backend totals; a restarted core may return zero.
 assert.deepEqual(cumulativeTotals({upload_total:123456,download_total:789012}),{up:123456,down:789012});
 assert.deepEqual(cumulativeTotals({upload_total:0,download_total:0}),{up:0,down:0});
 assert.deepEqual(cumulativeTotals({upload_total:NaN}),{up:null,down:null});
});

import {createReadGate} from './stateReliability.ts';
test('late reads cannot replace a newer event, and unrelated fields remain readable', () => {
 const gate=createReadGate(); const core=gate.begin('core'); const proxy=gate.begin('proxy');
 gate.invalidate('core');
 assert.equal(gate.current('core',core),false);
 assert.equal(gate.current('proxy',proxy),true);
 const next=gate.begin('proxy'); assert.equal(gate.current('proxy',proxy),false);
 assert.equal(gate.current('proxy',next),true);
});

import {parsePort} from './stateReliability.ts';
test('ports require explicit decimal integers within the supported range', () => {
 for(const value of ['', '1e4', '0x2000', '7898.1', '1023', '65536', '-1']) assert.equal(parsePort(value),null);
 assert.equal(parsePort(' 7898 '),7898); assert.equal(parsePort('65535'),65535);
});

import {fmtBytes,fmtDuration} from './format.ts';
test('unknown data is distinct from zero and long durations use hours and days', () => {
 assert.equal(fmtBytes(null),'—'); assert.equal(fmtBytes(0),'0 B'); assert.equal(fmtDuration(null),'—');
 assert.equal(fmtDuration(NaN),'—'); assert.equal(fmtDuration(0),'0ms');
 assert.equal(fmtDuration(3600000),'1h 0m'); assert.equal(fmtDuration(90000000),'1d 1h');
});
