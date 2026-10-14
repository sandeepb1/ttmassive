// Sidebar states: waiting, stopped (can resume), helper agents, tags and notes, a kept project.
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';

const url = process.env.TTM_URL ?? 'http://localhost:5199';
const pw = process.env.TTM_PW ?? '';
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const out = process.argv[2] ?? 'sidebar-shots';
mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ executablePath: exe });
for (const [sname, viewport] of Object.entries({ tablet: { width: 1024, height: 900 }, phone: { width: 390, height: 844 } })) {
  for (const theme of ['midnight', 'daylight']) {
    const ctx = await browser.newContext({ viewport, deviceScaleFactor: 2, hasTouch: sname === 'phone' });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => console.log('pageerror', e.message));
    await page.addInitScript((t) => localStorage.setItem('ttm.theme', JSON.stringify({ auto: false, theme: t, light: 'daylight', dark: 'midnight' })), theme);
    await page.route(/\/api\/sessions$/, async (route) => {
      const res = await route.fetch();
      const list = await res.json();
      const base = list[0];
      const mk = (o) => ({ ...base, ...o });
      await route.fulfill({ json: [
        mk({ id: 'main', agent: 'claude', preset: 'claude', activity: 'waiting', status: 'running', idle_secs: 3, preview: 'Tick **Media playback** only.\n- **Media playback:** the app plays `mp3` and `ogg` files from the [docs](https://example.com), then stops', branch: 'main', running_subagents: 2, tags: ['infra', 'urgent'], note: 'Cloudflare tunnel work', yolo: true, pinned: true, project: '/Users/me/ttmassive', cwd: '/Users/me/ttmassive/web', attached: 1 }),
        mk({ id: 'codex-api', agent: 'codex', preset: 'codex', activity: 'working', status: 'running', idle_secs: 0, project: '/Users/me/ttmassive', cwd: '/Users/me/ttmassive', preview: null, running_subagents: 0, tags: [], note: null, yolo: false, pinned: false, branch: 'main' }),
        mk({ id: 'old-claude', agent: 'claude', preset: 'claude', activity: null, status: 'stopped', resumable: true, has_agent_session: true, idle_secs: 7200, project: '/Users/me/cedarwing', cwd: '/Users/me/cedarwing', preview: null, running_subagents: 0, tags: [], note: null, yolo: false, pinned: false, branch: null }),
        mk({ id: 'scratch', agent: null, status: 'exited', exit_code: 1, idle_secs: 90, project: '/Users/me/cedarwing', cwd: '/Users/me/cedarwing', preview: null, running_subagents: 0, tags: [], note: null, yolo: false, pinned: false, branch: null }),
      ] });
    });
    await page.goto(url);
    await page.fill('#pw', pw);
    await page.click('#login-form button[type=submit]');
    await page.waitForSelector('#app:not([hidden])');
    await page.waitForTimeout(1200);
    if (sname === 'phone') await page.locator('.p-menu').locator('visible=true').first().click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${out}/${sname}-${theme}-sidebar-states.png` });
    await ctx.close();
  }
}
await browser.close();
