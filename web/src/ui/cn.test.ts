import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cn, tv } from './cn.ts';

test('custom text sizes and colours do not conflict', () => {
  assert.equal(cn('text-md', 'text-foreground'), 'text-md text-foreground');
  assert.equal(cn('text-2xs text-muted-foreground', 'text-md'), 'text-muted-foreground text-md');
});

test('custom control heights merge with each other but not with colours', () => {
  assert.equal(cn('h-ctl', 'h-ctl-sm'), 'h-ctl-sm');
  assert.equal(cn('h-ctl bg-primary', 'px-3'), 'h-ctl bg-primary px-3');
});

test('later classes win and falsy parts are dropped', () => {
  assert.equal(cn('px-3', false, 'px-4', null), 'px-4');
  assert.equal(cn('rounded-lg', 'rounded-xl'), 'rounded-xl');
});

test('variants merge through the same config', () => {
  const t = tv({ base: 'text-md', variants: { tone: { a: 'text-foreground', b: 'text-primary' } } });
  assert.equal(t({ tone: 'a', class: 'text-sm' }), 'text-foreground text-sm');
});
