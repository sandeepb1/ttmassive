// Chat surface screenshots: node scripts/chat-shots.mjs <outdir>. Same env as shots.mjs.
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';
import { installChatFixture } from './chat-fixture.mjs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'chat-shots';
mkdirSync(out, { recursive: true });
const sizes = { phone: { width: 390, height: 844 }, tablet: { width: 1024, height: 768 } };
const browser = await chromium.launch({ executablePath: exe });
for (const [sname, viewport] of Object.entries(sizes)) {
  for (const theme of ['midnight', 'daylight']) {
    const ctx = await browser.newContext({ viewport, deviceScaleFactor: 2, hasTouch: sname === 'phone' });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => console.log('pageerror', e.message));
    await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
    await installChatFixture(page);
    await page.goto(url);
    await page.fill('#pw', pw);
    await page.click('#login-form button[type=submit]');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForTimeout(1500);
    const tag = `${sname}-${theme}`;
    await page.screenshot({ path: `${out}/${tag}-1-chat-bottom.png` });
    const scroller = page.locator('.chat-scroll, .chat-stream, .chat-log').first();
    if (await scroller.count()) { await scroller.evaluate((e) => (e.scrollTop = 0)); await page.waitForTimeout(300); await page.screenshot({ path: `${out}/${tag}-2-chat-top.png` }); }
    await ctx.close();
  }
}
await browser.close();
