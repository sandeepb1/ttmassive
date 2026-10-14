import { test } from 'node:test';
import assert from 'node:assert/strict';
import { countLabel, fileName, isPlainClick, step, swipeDir } from './image-viewer-model.ts';

test('a saved copy is named after the picture', () => {
  assert.equal(fileName('/api/chat-images/0102a6bfce3ee278cf4199076d105135.png'), '0102a6bfce3ee278cf4199076d105135.png');
  assert.equal(fileName('/api/uploads/8f84/shot%201.png?x=1#top'), 'shot 1.png');
  assert.equal(fileName('https://example.com/a/b/c.jpg'), 'c.jpg');
  assert.equal(fileName('/'), 'image');
  assert.equal(fileName(''), 'image');
});

test('a name from a URL can not leave the downloads folder', () => {
  assert.equal(fileName('/x/..%2F..%2Fetc%2Fpasswd'), '.._.._etc_passwd'.replace(/^\.+/, ''));
  assert.equal(fileName('/x/%2E%2E'), 'image');
  assert.equal(fileName('/x/a%5Cb.png'), 'a_b.png');
  assert.equal(fileName('/x/a%00b.png'), 'a_b.png');
  assert.equal(fileName('/x/100%.png'), '100%.png', 'a bad escape is kept, not thrown');
  assert.ok(!/[\\/]/.test(fileName('/x/..%2Fsecret')));
});

test('stepping wraps round the pictures', () => {
  assert.equal(step(0, 3, 1), 1);
  assert.equal(step(2, 3, 1), 0);
  assert.equal(step(0, 3, -1), 2);
  assert.equal(step(1, 3, -1), 0);
  assert.equal(step(0, 1, 1), 0);
  assert.equal(step(0, 0, 1), 0);
});

test('the counter shows only when there is more than one', () => {
  assert.equal(countLabel(0, 1), '');
  assert.equal(countLabel(1, 5), '2 of 5');
});

test('a swipe has to be long and mostly sideways', () => {
  assert.equal(swipeDir(-80, 5), 1, 'left goes forward');
  assert.equal(swipeDir(80, -5), -1, 'right goes back');
  assert.equal(swipeDir(-30, 0), 0, 'too short');
  assert.equal(swipeDir(-80, 70), 0, 'mostly down is a scroll, not a swipe');
  assert.equal(swipeDir(0, 200), 0);
});

test('only a plain click opens the viewer', () => {
  const base = { button: 0, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false };
  assert.equal(isPlainClick(base), true);
  assert.equal(isPlainClick({ ...base, ctrlKey: true }), false, 'Ctrl-click opens a tab as usual');
  assert.equal(isPlainClick({ ...base, metaKey: true }), false);
  assert.equal(isPlainClick({ ...base, shiftKey: true }), false);
  assert.equal(isPlainClick({ ...base, button: 1 }), false, 'middle click too');
});
