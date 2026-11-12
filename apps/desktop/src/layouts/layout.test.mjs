import {test} from 'node:test';
import assert from 'node:assert/strict';
import {friendlyKind} from './layout.ts';
test('friendly labels preserve unknown protocols',()=>{
 assert.equal(friendlyKind('URLTest',k=>k),'layout.auto');assert.equal(friendlyKind('Vless',k=>k),'VLESS');assert.equal(friendlyKind('FutureProto',k=>k),'FutureProto');
});
