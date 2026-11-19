import assert from 'node:assert/strict';
import test from 'node:test';
import { paginate } from './paginate.js';

const items = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'];
const pageSize = 4;

test('page 1 returns the first page', () => {
  assert.deepEqual(paginate(items, 1, pageSize), ['a', 'b', 'c', 'd']);
});

test('page 2 returns the second page', () => {
  assert.deepEqual(paginate(items, 2, pageSize), ['e', 'f', 'g', 'h']);
});

test('the final partial page keeps its remaining items', () => {
  assert.deepEqual(paginate(items, 3, pageSize), ['i', 'j']);
});
