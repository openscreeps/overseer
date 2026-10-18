// The shell around the pages: the status bar (no placeholders, no reflow, zoom level names),
// the top bar (one row on a narrow window, the brand link, its menus' keyboard and pointer
// handling), toasts, and motion that respects `prefers-reduced-motion`.

import type { Page } from '@playwright/test';
import { expect, test } from './fixtures';
import { status } from './support/room';

declare global {
  interface Window {
    /** Every text the status bar showed since the page loaded. */
    e2eStatusTexts?: string[];
    /** Every left edge the zoom level item had since it appeared. */
    e2eLevelLefts?: number[];
  }
}

/**
 * Records, from the page's first script on, every text of the status bar and every position of
 * its zoom level item (a DOM observer, so no intermediate state is missed).
 */
async function recordStatusBar(page: Page): Promise<void> {
  await page.addInitScript(() => {
    window.e2eStatusTexts = [];
    window.e2eLevelLefts = [];
    new MutationObserver(() => {
      const bar = document.querySelector('[aria-label="Status"]');
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

/** Adds a private server named `name` (never contacted) through the "Add server" dialog. */
async function addServer(page: Page, name: string) {
  await page
    .getByRole('region', { name: 'Servers' })
    .getByRole('button', { name: 'Add server' })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Add server' });
  await dialog.getByLabel('Name', { exact: true }).fill(name);
  await dialog
    .getByLabel('Address', { exact: true })
    .fill(`http://${name.toLowerCase().replaceAll(' ', '-')}.invalid/`);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).toBeHidden();
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
    signedInPage: page,
  }) => {
    await recordStatusBar(page);
    await page.reload();
    const bar = status(page);
    await expect(bar).toContainText('Connected', { timeout: 30_000 });
    await expect(bar.getByTitle('Zoom level', { exact: true })).toHaveText('Room', {
      timeout: 30_000,
    });
    // On a sharded server the shard comes first (`shard3 · tick N`).
    await expect(bar.getByTitle('Game tick')).toHaveText(/^(\w+ · )?tick \d+$/, {
      timeout: 30_000,
    });
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

    test('the top bar keeps every control on one row', async ({ signedInPage: page, user }) => {
      await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
      const topbar = page.getByRole('banner');
      const controls = [
        topbar.getByRole('link', { name: 'Overseer: servers' }),
        topbar.getByRole('button', { name: /^Server: / }),
        topbar.getByRole('searchbox', { name: 'Go to room' }),
        topbar.getByRole('button', { name: 'Top-down view' }),
        topbar.getByRole('button', { name: 'Perspective view' }),
        topbar.getByRole('button', { name: 'Drawer', exact: true }),
        topbar.getByRole('button', { name: `Account: ${user.username}` }),
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

  test('the brand leads to the servers', async ({ signedInPage: page }) => {
    await page.getByRole('banner').getByRole('link', { name: 'Overseer: servers' }).click();
    await expect(page).toHaveURL(/#\/$/);
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });

  test('menus move with the arrow keys and close with Escape, Tab or a click elsewhere', async ({
    signedInPage: page,
    user,
  }) => {
    await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
    const servers = page.getByRole('button', { name: /^Server: / });
    const menu = page.getByRole('menu');
    const items = menu.getByRole('menuitem').or(menu.getByRole('menuitemradio'));
    await expect(servers).toHaveAttribute('aria-expanded', 'false');

    await servers.click();
    await expect(servers).toHaveAttribute('aria-expanded', 'true');
    await expect(menu).toBeVisible();
    // The current server is checked; "Manage servers…" comes last.
    await expect(menu.getByRole('menuitemradio', { checked: true })).toHaveCount(1);
    await expect(items.last()).toHaveText('Manage servers…');
    await page.keyboard.press('ArrowDown');
    await expect(items.first()).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await expect(items.last()).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(items.first()).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
    await expect(servers).toHaveAttribute('aria-expanded', 'false');
    await expect(servers).toBeFocused();

    // ↓ on the button opens the menu on its first item; Tab leaves and closes it.
    await page.keyboard.press('ArrowDown');
    await expect(menu).toBeVisible();
    await expect(items.first()).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(menu).toBeHidden();

    // A click elsewhere closes it, also on the other menu, which opens alone.
    await servers.click();
    await expect(menu).toBeVisible();
    const account = page.getByRole('button', { name: `Account: ${user.username}` });
    await account.click();
    await expect(servers).toHaveAttribute('aria-expanded', 'false');
    await expect(account).toHaveAttribute('aria-expanded', 'true');
    await expect(menu).toHaveCount(1);
    await expect(menu.getByRole('menuitem')).toHaveText(['Profile', 'Usage', 'Rate limits', 'Sign out']);
    await status(page).getByText('Connected', { exact: true }).click();
    await expect(menu).toBeHidden();
    await expect(account).toHaveAttribute('aria-expanded', 'false');
  });

  test('a toast confirms, can be dismissed and goes by itself', async ({ page }) => {
    await page.goto('/#/');
    await addServer(page, 'Toast one');
    const first = page.getByRole('status').filter({ hasText: 'Added Toast one' });
    await expect(first).toBeVisible();
    await first.getByRole('button', { name: 'Dismiss' }).click();
    await expect(first).toBeHidden();

    await addServer(page, 'Toast two');
    const second = page.getByRole('status').filter({ hasText: 'Added Toast two' });
    await expect(second).toBeVisible();
    const shown = Date.now();
    await expect(second).toBeHidden({ timeout: 15_000 });
    // Long enough to read.
    expect(Date.now() - shown).toBeGreaterThan(3_000);
  });

  test.describe('with reduced motion', () => {
    test.use({ reducedMotion: 'reduce' });

    test('menus open without animating', async ({ page }) => {
      await page.goto('/#/');
      await page.getByRole('button', { name: /^Server: / }).click();
      const menu = page.getByRole('menu');
      await expect(menu).toBeVisible();
      const motion = await menu.evaluate((element) => {
        const style = getComputedStyle(element);
        return [style.animationDuration, style.transitionDuration];
      });
      for (const duration of motion) {
        expect(duration.split(',').every((part) => Number.parseFloat(part) === 0)).toBe(true);
      }
    });
  });
});
