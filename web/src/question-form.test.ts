import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Question } from './chat-model.ts';
import { initialForm, isComplete, isInstant, pickOption, toRequest, typeAnswer } from './question-form.ts';

const q = (question: string, multi: boolean, n: number): Question => ({
  header: 'H',
  question,
  multi_select: multi,
  options: Array.from({ length: n }, (_, i) => ({ label: `opt${i + 1}`, description: '' })),
});

test('a single question with one answer is instant; anything else is not', () => {
  assert.equal(isInstant([q('a', false, 2)]), true);
  assert.equal(isInstant([q('a', true, 2)]), false);
  assert.equal(isInstant([q('a', false, 2), q('b', false, 2)]), false);
});

test('single choice: picking replaces the previous pick', () => {
  const qs = [q('a', false, 3)];
  let s = initialForm(qs);
  s = pickOption(qs, s, 0, 2);
  s = pickOption(qs, s, 0, 3);
  assert.deepEqual(s, [{ choices: [3], text: '' }]);
});

test('multiple choice: picking toggles, and picks stay in option order', () => {
  const qs = [q('a', true, 4)];
  let s = initialForm(qs);
  s = pickOption(qs, s, 0, 3);
  s = pickOption(qs, s, 0, 1);
  assert.deepEqual(s[0].choices, [1, 3]);
  s = pickOption(qs, s, 0, 3);
  assert.deepEqual(s[0].choices, [1]);
});

test('picking an option that does not exist changes nothing', () => {
  const qs = [q('a', false, 2)];
  const s = initialForm(qs);
  assert.equal(pickOption(qs, s, 0, 3), s);
  assert.equal(pickOption(qs, s, 0, 0), s);
  assert.equal(pickOption(qs, s, 5, 1), s);
});

test('typing your own answer clears the pick, and picking clears the typing', () => {
  const qs = [q('a', false, 2)];
  let s = pickOption(qs, initialForm(qs), 0, 1);
  s = typeAnswer(s, 0, 'Teal');
  assert.deepEqual(s[0], { choices: [], text: 'Teal' });
  s = pickOption(qs, s, 0, 2);
  assert.deepEqual(s[0], { choices: [2], text: '' });
  // Clearing the text box keeps a pick that was made.
  s = typeAnswer(s, 0, '  ');
  assert.deepEqual(s[0].choices, [2]);
});

test('the form is complete only when every question has an answer', () => {
  const qs = [q('a', false, 2), q('b', true, 2)];
  let s = initialForm(qs);
  assert.equal(isComplete(s), false);
  s = pickOption(qs, s, 0, 1);
  assert.equal(isComplete(s), false);
  s = pickOption(qs, s, 1, 2);
  assert.equal(isComplete(s), true);
  assert.equal(isComplete([]), false);
  assert.equal(isComplete(typeAnswer(initialForm([q('a', false, 1)]), 0, '   ')), false);
});

test('the request carries picks or text, trimmed, never both', () => {
  const qs = [q('a', false, 2), q('b', true, 3), q('c', false, 2)];
  let s = initialForm(qs);
  s = pickOption(qs, s, 0, 2);
  s = pickOption(qs, s, 1, 1);
  s = pickOption(qs, s, 1, 3);
  s = typeAnswer(s, 2, '  my own  ');
  assert.deepEqual(toRequest(s), {
    answers: [
      { choices: [2], text: null },
      { choices: [1, 3], text: null },
      { choices: [], text: 'my own' },
    ],
  });
});
