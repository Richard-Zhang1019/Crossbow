import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LogHistory } from './logHistory.ts';
const entry = (sequence) => ({sequence,type:'info',payload:'same log text'});
const snapshot = (sequence, entries=[]) => ({sequence,entries,connected:true,generation:1});
const ids = (entries) => entries.map(e=>e.sequence);

test('snapshot and events overlap without duplication or loss',()=>{
 const history=new LogHistory();
 assert.deepEqual(history.accept([entry(2),entry(3)]),[]);
 assert.deepEqual(ids(history.hydrate(snapshot(2,[entry(1),entry(2)]))),[1,2,3]);
 assert.deepEqual(ids(history.accept([entry(2),entry(3),entry(4)])),[4]);
});
test('identical messages with different sequence numbers are retained',()=>{
 const history=new LogHistory();
 assert.deepEqual(ids(history.hydrate(snapshot(2,[entry(1),entry(2)]))),[1,2]);
});
test('clear fences delayed old batches and preserves newer concurrent logs',()=>{
 const history=new LogHistory(); history.hydrate(snapshot(2,[entry(1),entry(2)]));
 history.pause(); history.accept([entry(3),entry(4)]);
 assert.deepEqual(ids(history.hydrate(snapshot(3))),[4]);
 assert.deepEqual(ids(history.accept([entry(1),entry(2),entry(3),entry(4),entry(5)])),[5]);
});
test('failed clear resumes buffered live logs and keeps previous cursor',()=>{
 const history=new LogHistory(); history.hydrate(snapshot(2,[entry(1),entry(2)]));
 history.pause(); history.accept([entry(2),entry(3)]);
 assert.deepEqual(ids(history.resume()),[3]);
 assert.deepEqual(ids(history.accept([entry(3),entry(4)])),[4]);
});
test('empty cleared snapshot retains cursor across page remount',()=>{
 const history=new LogHistory(); history.hydrate(snapshot(20));
 assert.deepEqual(history.accept([entry(19),entry(20)]),[]);
 assert.deepEqual(ids(history.accept([entry(21)])),[21]);
});
test('pending replay is bounded even when snapshot IPC is slow',()=>{
 const history=new LogHistory(); history.accept(Array.from({length:3000},(_,i)=>entry(i+1)));
 const entries=history.hydrate(snapshot(0));
 assert.equal(entries.length,2000); assert.equal(entries[0].sequence,1001);
});
