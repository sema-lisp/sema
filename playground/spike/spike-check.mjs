// M1 acceptance check: load the spike page (served with COOP/COEP) in headless
// Chromium and assert cross-origin isolation + a real worker block + a
// responsive main thread. Run from the playground dir (resolves @playwright/test):
//   npx serve spike -l 8911 &   # serves spike/ with serve.json COOP/COEP headers
//   node spike/spike-check.mjs
import { chromium } from '@playwright/test';

const URL = process.env.SPIKE_URL || 'http://localhost:8911/';
const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto(URL);
await page.waitForSelector('[data-testid="out"][data-done="1"]', { timeout: 15000 });
const r = await page.evaluate(() => window.__spike);
await browser.close();

console.log(JSON.stringify(r, null, 2));

const checks = [
  ['cross-origin isolated', r.coi === true],
  ['SharedArrayBuffer available', r.hasSAB === true],
  ['worker blocked ~1000ms', r.workerBlockedMs >= 950 && r.workerBlockedMs <= 1600],
  ['Atomics.wait timed out', r.waitResult === 'timed-out'],
  ['main thread stayed responsive', r.mainTicksDuringBlock > 0],
];
let ok = true;
for (const [name, pass] of checks) {
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}`);
  ok = ok && pass;
}
console.log(ok ? '\nSPIKE PASS' : '\nSPIKE FAIL');
process.exit(ok ? 0 : 1);
