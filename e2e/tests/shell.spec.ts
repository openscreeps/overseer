// The shell around the pages: the status bar (no placeholders, no reflow, zoom level names),
// the top bar on a narrow window, and motion that respects `prefers-reduced-motion`.

import type { Page } from '@playwright/test';
import { expect, signIn, test } from './fixtures';

declare global {
  interface Window {
    /** Every text the status bar showed since the page loaded. */
    e2eStatusTexts?: string[];
    /** Every left edge the zoom level item had since it appeared. */
    e2eLevelLefts?: number[];
  }
}

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/**
 * Records, from the page's first script on, every text of the status bar and every position of
 * its zoom level item (a DOM observer, so no intermediate state is missed).
 */
async function recordStatusBar(page: Page): Promise<void> {
  await page.addInitScript(() => {
    window.e2eStatusTexts = [];
    window.e2eLevelLefts = [];
    new MutationObserver(() => {
      const bar = document.querySelector('footer.status-bar');
      if (!bar) return;
      const text = bar.textContent ?? '';
      if (window.e2eStatusTexts?.at(-1) !== text) window.e2eStatusTexts?.push(text);
      const level = bar.querySelector('[title="Zoom level"]');
      if (level) {
        const left = Math.round(level.getBoundingClientRect().left);
        if (window.e2eLevelLefts?.at(-1) !== left) window.e2eLevelLefts?.push(left);
      }
    }).observe(document, { childList: true, subtree: true, characterData: true });
  });
}

test.describe('shell', () => {
  test('the status bar shows no placeholders before connecting', async ({ page }) => {
    await page.goto('/#/');
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
    const bar = status(page);
    await expect(bar).toContainText('Not connected');
    await expect(bar).toContainText('Web');
    await expect(bar).not.toContainText('—');
    await expect(bar).not.toContainText('tick');
    await expect(bar).not.toContainText('frame');
    await expect(bar.getByTitle('Zoom level')).toHaveCount(0);
    await expect(bar.getByTitle('Graphics backend')).toHaveCount(0);
  });

  test('status items appear once known and never shift each other', async ({
    page,
    server,
    player,
  }) => {
    await recordStatusBar(page);
    await signIn(page, server, player);
    const bar = status(page);
    await expect(bar).toContainText('Connected', { timeout: 30_000 });
    await expect(bar.getByTitle('Zoom level', { exact: true })).toHaveText('Room', {
      timeout: 30_000,
    });
    await expect(bar.getByTitle('Game tick')).toHaveText(/^tick \d+$/, { timeout: 30_000 });
    await expect(bar.getByTitle('Graphics backend')).toHaveText(/^(WebGPU|WebGL2)$/);
    await expect(bar.getByTitle('CPU time of the last frame')).toHaveText(/^frame \d+\.\d\d ms$/);
    const { texts, lefts } = await page.evaluate(() => ({
      texts: window.e2eStatusTexts ?? [],
      lefts: window.e2eLevelLefts ?? [],
    }));
    expect(texts.length).toBeGreaterThan(1);
    for (const text of texts) {
      expect(text).not.toContain('—');
      expect(text).not.toContain('Live rooms');
    }
    // The zoom level kept its place from its (empty, reserved) start through the connection
    // settling and the first tick arriving.
    expect(lefts).toHaveLength(1);
  });

  test.describe('in a narrow window', () => {
    test.use({ viewport: { width: 560, height: 800 } });

    test('the top bar keeps every control on one row', async ({ page, server, player }) => {
      await signIn(page, server, player);
      await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
      const topbar = page.getByRole('banner');
      const controls = [
        topbar.getByRole('link', { name: 'Overseer: servers' }),
        topbar.getByRole('button', { name: /^Server: / }),
        topbar.getByRole('searchbox', { name: 'Go to room' }),
        topbar.getByRole('button', { name: 'Top-down view' }),
        topbar.getByRole('button', { name: 'Perspective view' }),
        topbar.getByRole('button', { name: 'Drawer', exact: true }),
        topbar.getByRole('button', { name: `Account: ${player.username}` }),
      ];
      const bar = await topbar.boundingBox();
      if (!bar) throw new Error('the top bar has no box');
      for (const control of controls) {
        await expect(control).toBeVisible();
        const box = await control.boundingBox();
        if (!box) throw new Error('a control has no box');
        expect(box.x).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width).toBeLessThanOrEqual(560);
        // One row: every control inside the bar's height.
        expect(box.y).toBeGreaterThanOrEqual(bar.y);
        expect(box.y + box.height).toBeLessThanOrEqual(bar.y + bar.height);
      }
      expect(await topbar.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
        true,
      );
    });
  });

  test.describe('with reduced motion', () => {
    test.use({ reducedMotion: 'reduce' });

    test('menus open without animating', async ({ page }) => {
      await page.goto('/#/');
      const duration = await page.evaluate(() =>
        getComputedStyle(document.documentElement).getPropertyValue('--duration').trim(),
      );
      expect(duration).toBe('0ms');
      await page.getByRole('button', { name: /^Server: / }).click();
      const menu = page.getByRole('menu');
      await expect(menu).toBeVisible();
      expect(await menu.evaluate((element) => getComputedStyle(element).animationDuration)).toBe(
        '0s',
      );
    });
  });
});
