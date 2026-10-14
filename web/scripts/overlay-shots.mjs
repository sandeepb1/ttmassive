// Overlays: the prompt bar, the image viewer, the command bar and the terminal toast.
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';
import { chatResponse, installChatFixture, items } from './chat-fixture.mjs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'overlay-shots';
mkdirSync(out, { recursive: true });
const pic = 'data:image/svg+xml;utf8,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300"><rect width="400" height="300" fill="#345"/><circle cx="200" cy="150" r="80" fill="#e8a06b"/></svg>');
const browser = await chromium.launch({ executablePath: exe });
for (const theme of ['midnight', 'daylight']) {
  const ctx = await browser.newContext({ viewport: { width: 1024, height: 768 }, deviceScaleFactor: 1.5 });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
  await installChatFixture(page);
  // Replace the chat response: drop the pending question, add a picture, and show an agent prompt.
  const body = {
    ...chatResponse,
    items: [
      ...items.filter((i) => i.kind !== 'question'),
      { id: 'pic1', seq: 99, ts: null, kind: 'user', text: 'What is in this picture?', images: 1, pics: [pic] },
    ],
    seq: 99,
    screen: { activity: 'waiting', tail: [], prompt: { id: 'p1', question: 'Do you want to run cargo test?', context: ['Bash command'], choices: [{ index: 1, label: 'Yes', selected: true }, { index: 2, label: 'Yes, and do not ask again', selected: false }, { index: 3, label: 'No, tell Claude what to do differently', selected: false }] } },
  };
  await page.route(/\/api\/sessions\/main\/chat/, (r) => r.fulfill({ json: body }));
  // A slow upload keeps the "Uploading…" toast on screen long enough to photograph.
  await page.route(/\/api\/sessions\/main\/upload/, async (r) => { await new Promise((ok) => setTimeout(ok, 4000)); await r.fulfill({ json: { path: '/tmp/notes.txt' } }); });
  await page.goto(url);
  await page.fill('#pw', pw);
  await page.click('#login-form button[type=submit]');
  await page.waitForSelector('#app:not([hidden])');
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${out}/${theme}-1-prompt-bar.png` });
  try {
    await page.locator('.msg-img, .msg-pic img, img.msg-img').first().click({ timeout: 3000 });
    await page.waitForSelector('.img-viewer', { timeout: 3000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${out}/${theme}-2-image-viewer.png` });
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
  } catch (e) { console.log('skip viewer', e.message.split('\n')[0]); }
  try {
    await page.locator('[aria-label^="Commands"]').locator('visible=true').first().click({ timeout: 3000 });
    await page.waitForSelector('.cmd', { timeout: 3000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${out}/${theme}-3-command-bar.png` });
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
  } catch (e) { console.log('skip command bar', e.message.split('\n')[0]); }
  try {
    await page.locator('button', { hasText: /^Terminal$/ }).first().click({ timeout: 3000 });
    await page.waitForTimeout(800);
    await page.evaluate(() => {
      const dt = new DataTransfer();
      dt.items.add(new File(['hello'], 'notes.txt', { type: 'text/plain' }));
      (document.querySelector('.xterm-helper-textarea') ?? document.querySelector('.p-term')).dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    });
    await page.waitForTimeout(150);
    await page.screenshot({ path: `${out}/${theme}-4-terminal-toast.png` });
  } catch (e) { console.log('skip toast', e.message.split('\n')[0]); }
  await ctx.close();
}
await browser.close();
