import { test } from 'node:test';
import assert from 'node:assert/strict';
import { toolCategory, toolIconName } from './icons.ts';

test('tools map to a category that has an icon', () => {
  assert.equal(toolCategory('Bash'), 'shell');
  assert.equal(toolCategory('Write'), 'edit');
  assert.equal(toolCategory('MultiEdit'), 'edit');
  assert.equal(toolCategory('Grep'), 'search');
  assert.equal(toolCategory('Agent'), 'agent');
  assert.equal(toolCategory('Task'), 'agent');
  assert.equal(toolCategory('TodoWrite'), 'todo');
});

test('unknown tools (MCP, Codex) fall back to a plain dot', () => {
  assert.equal(toolCategory('mcp__linear__list_issues'), 'other');
  assert.equal(toolIconName('other'), 'dot');
  assert.equal(toolIconName('shell'), 'shell');
});
