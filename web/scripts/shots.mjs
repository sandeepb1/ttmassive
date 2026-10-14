// Screenshot key surfaces at phone/tablet/desktop widths in two themes, for before/after review.
// Usage: TTM_URL=http://localhost:5199 TTM_PW=... node scripts/shots.mjs <outdir>
// Needs playwright-core and a Chromium path in CHROMIUM (see the ttm test-environment notes).
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'shots';
mkdirSync(out, { recursive: true });

const sizes = { phone: { width: 390, height: 844 }, tablet: { width: 1024, height: 768 }, desktop: { width: 1440, height: 900 } };
const themes = ['midnight', 'daylight'];
const browser = await chromium.launch({ executablePath: exe });

async function step(page, name, fn) {
  try { await fn(); } catch (e) { console.log(`skip ${name}: ${e.message.split('\n').slice(0, 7).join(' | ')}`); }
}

for (const [sname, viewport] of Object.entries(sizes)) {
  for (const theme of themes) {
    const ctx = await browser.newContext({ viewport, deviceScaleFactor: 2, hasTouch: sname !== 'desktop' });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => console.log('pageerror', e.message));
    await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
    const tag = `${sname}-${theme}`;
    await page.goto(url);
    await page.waitForSelector('#pw, #app:not([hidden])');
    await page.screenshot({ path: `${out}/${tag}-1-login.png` });
    if (await page.locator('#pw').isVisible()) {
      await page.fill('#pw', pw);
      await page.click('#login-form button[type=submit]');
    }
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForTimeout(1200);
    await page.screenshot({ path: `${out}/${tag}-2-app.png` });
    await step(page, 'sidebar', async () => {
      if (sname === 'phone') await page.locator('.p-menu').locator('visible=true').first().click({ timeout: 5000 });
      await page.waitForTimeout(400);
      await page.screenshot({ path: `${out}/${tag}-3-sidebar.png` });
      await page.click('.sb-settings', { timeout: 5000 });
      await page.waitForTimeout(500);
      await page.screenshot({ path: `${out}/${tag}-5-settings.png` });
      await page.click('[data-sheet] [aria-label="Close"]', { timeout: 5000 });
      await page.waitForSelector('[data-sheet]', { state: 'detached', timeout: 5000 });
      await page.click('.sb-new', { timeout: 5000 });
      await page.waitForTimeout(500);
      await page.screenshot({ path: `${out}/${tag}-4-new-session.png` });
      await page.keyboard.press('Escape');
      await page.waitForTimeout(300);
    });
    await ctx.close();
  }
}
await browser.close();
