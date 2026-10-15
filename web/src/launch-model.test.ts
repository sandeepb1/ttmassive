import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseLaunch, sameLaunch, startSession } from './launch-model.ts';

test('a session in the address is read, decoded and validated', () => {
  assert.equal(parseLaunch('https://x.test/#claude-todo').session, 'claude-todo');
  assert.equal(parseLaunch('/#a%5Fb').session, 'a_b');
  assert.equal(parseLaunch('/#has space').session, null);
  assert.equal(parseLaunch('/#%E0%A4%A').session, null); // a broken escape
  assert.equal(parseLaunch('/').session, null);
});

test('shortcuts name an agent, and only the two we know', () => {
  assert.equal(parseLaunch('/?new=codex').newAgent, 'codex');
  assert.equal(parseLaunch('/?new=claude#main').newAgent, 'claude');
  assert.equal(parseLaunch('/?new=rm').newAgent, null);
});

test('resume is a flag', () => {
  assert.equal(parseLaunch('/?resume').resume, true);
  assert.equal(parseLaunch('/').resume, false);
});

test('the address wins over where you were', () => {
  assert.equal(startSession('b', 'a', ['a', 'b']), 'b');
});

test('an icon launch with no address goes back to the last session if it still exists', () => {
  assert.equal(startSession(null, 'claude-todo', ['main', 'claude-todo']), 'claude-todo');
});

test('a last session that is gone is not recreated: back to main', () => {
  assert.equal(startSession(null, 'old', ['main']), 'main');
});

test('with no list to check against, do not guess', () => {
  assert.equal(startSession(null, 'claude-todo', null), 'main');
});

test('a stored name that is not a valid id is ignored', () => {
  assert.equal(startSession(null, '../etc', ['../etc']), 'main');
});

test('the same launch twice is recognised', () => {
  assert.equal(sameLaunch(parseLaunch('/?new=codex#a'), parseLaunch('/?new=codex#a')), true);
  assert.equal(sameLaunch(parseLaunch('/?new=codex#a'), parseLaunch('/#a')), false);
});
