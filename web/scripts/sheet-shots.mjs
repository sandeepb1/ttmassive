// Screenshots of the chat sheets (model, mode, more, layout) using the canned conversation.
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';
import { installChatFixture } from './chat-fixture.mjs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'sheet-shots';
mkdirSync(out, { recursive: true });
const sizes = { phone: { width: 390, height: 844 }, tablet: { width: 1024, height: 768 } };
const modelMenu = {
  rows: [
    { number: 1, label: 'Opus 5.5', description: 'Most capable for complex work', current: true, default: false },
    { number: 2, label: 'Sonnet 5.5', description: 'Best for everyday tasks', current: false, default: false },
    { number: 3, label: 'Haiku 5.5', description: 'Fastest for quick answers', current: false, default: false },
  ],
  effort: 'high',
};
const browser = await chromium.launch({ executablePath: exe });
for (const [sname, viewport] of Object.entries(sizes)) {
  for (const theme of ['midnight', 'daylight']) {
    const ctx = await browser.newContext({ viewport, deviceScaleFactor: 2, hasTouch: sname === 'phone' });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => console.log('pageerror', e.message));
    await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
    await installChatFixture(page);
    await page.route(/\/api\/sessions\/main\/model\/open/, (r) => r.fulfill({ json: modelMenu }));
    await page.route(/\/api\/sessions\/main\/model\/close/, (r) => r.fulfill({ status: 204 }));
    await page.goto(url);
    await page.fill('#pw', pw);
    await page.click('#login-form button[type=submit]');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForTimeout(1500);
    const tag = `${sname}-${theme}`;
    const shot = async (name, open) => {
      try {
        await open();
        await page.waitForSelector('[data-sheet]', { timeout: 4000 });
        await page.waitForTimeout(500);
        await page.screenshot({ path: `${out}/${tag}-${name}.png` });
        await page.click('[data-sheet] [aria-label="Close"]', { timeout: 4000 });
        await page.waitForSelector('[data-sheet]', { state: 'detached', timeout: 4000 });
      } catch (e) { console.log(`skip ${tag} ${name}: ${e.message.split('\n')[0]}`); }
    };
    await shot('model', () => page.locator('button', { hasText: 'Opus' }).first().click());
    await shot('mode', () => page.locator('button', { hasText: /^Default$/ }).first().click());
    await shot('more', () => page.click('[aria-label="More"]'));
    await shot('layout', () => page.locator('[aria-label="Pane layouts"]').locator('visible=true').first().click());
    await ctx.close();
  }
}
await browser.close();
