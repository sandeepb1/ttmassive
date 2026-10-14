// Right bar tabs over the canned conversation: overview, files, to-do and search results.
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';
import { installChatFixture } from './chat-fixture.mjs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'rightbar-shots';
mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ executablePath: exe });
for (const theme of ['midnight', 'daylight']) {
  const ctx = await browser.newContext({ viewport: { width: 1500, height: 900 }, deviceScaleFactor: 1.5 });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
  await installChatFixture(page);
  await page.goto(url);
  await page.fill('#pw', pw);
  await page.click('#login-form button[type=submit]');
  await page.waitForSelector('#app:not([hidden])');
  await page.waitForTimeout(1500);
  if (!(await page.locator('#right').isVisible())) await page.locator('[aria-label="Session details"]').locator('visible=true').first().click();
  await page.waitForTimeout(600);
  const tabs = page.locator('#right .rb-tab');
  const n = await tabs.count();
  for (let i = 0; i < n; i++) {
    const t = tabs.nth(i);
    const name = ((await t.getAttribute('aria-label')) ?? `tab${i}`).replace(/\W+/g, '-').toLowerCase();
    if (!(await t.isVisible())) continue;
    await t.click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${out}/${theme}-${i}-${name}.png`, clip: { x: 1500 - 340, y: 0, width: 340, height: 900 } });
  }
  await page.fill('#right .rb-search-input', 'tunnel');
  await page.waitForTimeout(900);
  await page.screenshot({ path: `${out}/${theme}-search.png`, clip: { x: 1500 - 340, y: 0, width: 340, height: 900 } });
  await ctx.close();
}
await browser.close();
