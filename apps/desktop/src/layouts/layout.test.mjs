import {test} from 'node:test';
import assert from 'node:assert/strict';
import {nodeResultHeight,friendlyKind} from './layout.ts';
import {fitGrid} from '../components/FitItems/fitGrid.ts';
test('sparse results use natural rows while long results reserve a pager',()=>{
 for(const width of [380,600,1000])for(const count of [1,2,8,25,400]){
  const height=nodeResultHeight(count,width);
  const grid=fitGrid(width,height,count,200,4,76,4);
  assert.ok(grid.rows*76+(grid.rows-1)*8+(grid.pages>1?40:0)<=height);
  assert.equal(grid.pages>1,count>grid.count);
 }
 assert.equal(nodeResultHeight(2,900),76);
});
test('friendly labels preserve unknown protocols',()=>{
 assert.equal(friendlyKind('URLTest',k=>k),'layout.auto');assert.equal(friendlyKind('Vless',k=>k),'VLESS');assert.equal(friendlyKind('FutureProto',k=>k),'FutureProto');
});
