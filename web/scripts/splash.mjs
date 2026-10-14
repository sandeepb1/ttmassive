// Render the iOS launch images (apple-touch-startup-image) into public/splash/ and print the <link> tags for
// index.html. iOS shows a white or black screen at launch unless the page names an image for the exact device size.
// Usage: node scripts/splash.mjs   (needs playwright-core and a Chromium path in CHROMIUM, see UI.md)
import { chromium } from 'playwright-core';
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = process.env.TTM_WEB ?? join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'public', 'splash');
mkdirSync(out, { recursive: true });
const exe = process.env.CHROMIUM ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell`;

// Portrait CSS size and pixel ratio of each device class that can install a web app.
const DEVICES = [
  ['iphone-16-pro-max', 440, 956, 3],
  ['iphone-16-pro', 402, 874, 3],
  ['iphone-15-pro-max', 430, 932, 3],
  ['iphone-15-pro', 393, 852, 3],
  ['iphone-14-plus', 428, 926, 3],
  ['iphone-13', 390, 844, 3],
  ['iphone-x', 375, 812, 3],
  ['iphone-11-pro-max', 414, 896, 3],
  ['iphone-11', 414, 896, 2],
  ['iphone-se', 375, 667, 2],
  ['ipad-pro-12', 1024, 1366, 2],
  ['ipad-pro-11', 834, 1194, 2],
  ['ipad-air', 820, 1180, 2],
  ['ipad-10', 810, 1080, 2],
  ['ipad-mini', 744, 1133, 2],
];

const BG = '#0b0d10'; // the manifest's background_color
const icon = readFileSync(join(root, 'public', 'icon.svg'), 'utf8');
const browser = await chromium.launch({ executablePath: exe });
const links = [];
for (const [name, w, h, dpr] of DEVICES) {
  const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: dpr });
  const size = Math.round(Math.min(w, h) * 0.24);
  await page.setContent(`<body style="margin:0;background:${BG};display:grid;place-items:center;height:100vh"><div style="width:${size}px;height:${size}px">${icon}</div><style>svg{width:100%;height:100%}</style></body>`);
  await page.screenshot({ path: join(out, `${name}.png`) });
  await page.close();
  links.push(
    `    <link rel="apple-touch-startup-image" href="/splash/${name}.png" media="(device-width: ${w}px) and (device-height: ${h}px) and (-webkit-device-pixel-ratio: ${dpr}) and (orientation: portrait)" />`,
  );
}
await browser.close();
console.log(links.join('\n'));
