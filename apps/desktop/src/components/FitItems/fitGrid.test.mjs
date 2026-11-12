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

test('13 nodes gain a third row only when the cards and pager fit', () => {
  const compact = fitGrid(900, 259, 13, 200, 4, 68, 3);
  assert.deepEqual(compact, { columns: 4, rows: 2, count: 8, pages: 2 });
  assert.equal(fitRowHeight(259, 68, 100, compact.rows, compact.pages, 2), 100);

  const regular = fitGrid(900, 260, 13, 200, 4, 68, 3);
  assert.deepEqual(regular, { columns: 4, rows: 3, count: 12, pages: 2 });
  assert.equal(fitRowHeight(260, 68, 100, regular.rows, regular.pages, 3), 68);
  assert.equal(fitRowHeight(260, 68, 100, regular.rows, regular.pages, 1), 68);
});

test('card heights absorb space within bounds and omit pager space on a single page', () => {
  assert.equal(fitRowHeight(208, 68, 100, 2, 2, 2), 80);
  assert.equal(fitRowHeight(208, 68, 100, 2, 1, 2), 100);
  assert.equal(fitRowHeight(100, 68, 100, 2, 2, 2), 68);
  assert.equal(fitRowHeight(600, 68, 100, 3, 2, 3), 100);
  assert.equal(fitRowHeight(600, 68, 100, 1, 1, 1), 68);
});
