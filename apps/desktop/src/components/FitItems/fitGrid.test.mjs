import {test} from 'node:test';
import assert from 'node:assert/strict';
import {fitGrid, fitRowHeight} from './fitGrid.ts';
test('fits 3 or 4 columns and rows according to actual space',()=>{
 assert.deepEqual(fitGrid(830,400,101,200,4,76,4),{columns:4,rows:4,count:16,pages:7});
 assert.deepEqual(fitGrid(620,225,101,200,4,76,4),{columns:3,rows:2,count:6,pages:17});
 assert.equal(fitGrid(620,300,101,200,4,76,4).rows,3);
});
test('reserves pager height only when required; empty and small lists stay valid',()=>{
 assert.equal(fitGrid(620,244,9,200,4,76,4).pages,1);
 assert.equal(fitGrid(620,244,10,200,4,76,4).rows,2);
 assert.equal(fitGrid(0,0,0,200,4,76,4).pages,1);
 assert.equal(fitGrid(400,400,2,200,4,76,4).rows,2);
});

test('small lists and sparse last pages keep natural card height',()=>{
 assert.equal(fitRowHeight(600,76,2,1,8,8),76);
 assert.equal(fitRowHeight(900,76,1,1,1,1),76);
 assert.equal(fitRowHeight(600,76,4,7,5,16),76);
 assert.equal(fitRowHeight(600,76,4,7,15,16),76);
});
test('only full paginated pages fill extra space, with bounded row height',()=>{
 assert.equal(fitRowHeight(240,76,2,17,6,6),96);
 assert.equal(fitRowHeight(1000,76,4,7,16,16),114);
 assert.equal(fitRowHeight(100,76,1,20,4,4),76);
});
