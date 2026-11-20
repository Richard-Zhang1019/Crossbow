// Production React components, fake IPC and fictional credentials only.
// Run from repository root: node docs/reviews/issue-569/verify.mjs
import { startVite, stubSource, launchBrowser } from '../../../scripts/dev/verify-lib.mjs';
import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const out = process.env.CB569_ARTIFACT_DIR ?? path.dirname(fileURLToPath(import.meta.url));
mkdirSync(out, { recursive: true });
const before = process.argv.includes('--before');
const vite = await startVite();
const browser = await launchBrowser(true);
try {
  const context = await browser.newContext({ viewport: { width: 1120, height: 720 } });
  await context.addInitScript(stubSource());
  const page = await context.newPage();
  await page.goto(vite.url);
  await page.locator('.cb-system-foot', { hasText: 'Crossbow v' }).waitFor();
  await page.locator('.cb-primary-nav button', { hasText: '配置' }).click();
  await page.getByRole('button', { name: '添加订阅', exact: true }).click();
  await page.getByRole('dialog').waitFor();
  if (before) {
    await page.screenshot({ path: path.join(out, 'import-before.png') });
  } else {
    const raw = 'ss://YWVzLTEyOC1nY206ZXhhbXBsZS1wYXNzd29yZA@ss.example.invalid:8388#Tokyo\ntrojan://example-password@tr.example.invalid:443#Singapore';
    await page.evaluate(() => {
      window.__cb.submitted = [];
      window.__cb.setCommands({ import_profile_url: ({ url }) => {
        window.__cb.submitted.push(url);
        throw Error('Fixture import failed');
      } });
    });
    const field = page.locator('#profile-url');
    await field.fill(raw);
    await field.press('Enter');
    assert.equal(await page.evaluate(() => window.__cb.submitted.length), 0);
    await field.fill(raw);
    await field.press('Control+Enter');
    await page.waitForFunction(() => window.__cb.submitted.length === 1);
    await page.waitForFunction(() => document.querySelector('[role="dialog"] button[data-variant="primary"]')?.disabled === false);
    assert.equal(await page.evaluate(() => window.__cb.submitted[0]), raw);
    assert.equal(await field.inputValue(), raw);
    assert(await page.getByRole('dialog').isVisible());
    await page.waitForTimeout(180); // let the shared button transition settle
    await page.screenshot({ path: path.join(out, 'import-after.png') });
    await page.evaluate(() => window.__cb.emit('ui://theme', 'dark'));
    await page.waitForFunction(() => document.documentElement.classList.contains('dark'));
    await page.setViewportSize({ width: 880, height: 560 });
    await page.waitForTimeout(180);
    await page.screenshot({ path: path.join(out, 'import-dark-small.png') });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    await page.evaluate(() => window.__cb.emit('ui://lang', 'en'));
    await page.getByText('Subscription URL or share links', { exact: true }).waitFor();
    await page.waitForTimeout(180);
    await page.screenshot({ path: path.join(out, 'import-dark-small-en.png') });
    const dialog = await page.getByRole('dialog').boundingBox();
    assert(dialog && dialog.y >= 0 && dialog.y + dialog.height <= 560);
    // #584: exercise the existing chart with the shared button boundary.
    const stats = await context.newPage();
    await stats.goto(vite.url);
    await stats.locator('.cb-system-foot', { hasText: 'Crossbow v' }).waitFor();
    await stats.evaluate(() => {
      const now = new Date();
      const key = `${now.getFullYear()}-${String(now.getMonth()+1).padStart(2,'0')}-${String(now.getDate()).padStart(2,'0')}`;
      window.__cb.setCommands({ traffic_stats: { daily: [{ key, up: 1024, down: 4096 }], domains: [], nodes: [] } });
    });
    await stats.locator('.cb-primary-nav button', { hasText: '监测' }).click();
    await stats.getByRole('button', { name: '统计', exact: true }).click();
    const day = stats.locator('.cb-stats-day').first();
    await day.waitFor();
    await day.click();
    assert.equal(await day.getAttribute('aria-pressed'), 'true');
    await day.focus();
    await day.press('Enter');
    assert.equal(await day.getAttribute('aria-pressed'), 'true');
    await stats.screenshot({ path: path.join(out, 'stats-shared-button.png') });
    console.log('PASS: multiline paste, Enter/Ctrl+Enter, failed input retained, dark small/en, chart mouse/keyboard selection');
  }
} finally {
  await browser.close();
  await vite.close();
}
