// The account's CPU and Memory usage: the status bar's usage button shows the latest run of the
// account's code against the limits, and the Usage panel (from that button or the account menu)
// shows the latest run, the average and peak of the recent runs and the CPU limit. A
// single-world server allocates no CPU to shards: the panel has no shard part.
//
// The `player` fixture's bot runs every tick, so a run arrives within a few ticks.

import type { Locator, Page } from '@playwright/test';
import { expect, test } from './fixtures';
import { accountMenu } from './support/accounts';
import { status } from './support/room';

/** The status bar's usage button, once a run arrived. */
async function usageButton(page: Page): Promise<Locator> {
  const button = status(page).getByRole('button', { name: 'CPU and Memory usage' });
  await expect(button).toBeVisible({ timeout: 30_000 });
  return button;
}

/** Checks that the Usage `panel` shows the latest run, its CPU limit and no shard part. */
async function expectRun(panel: Locator): Promise<void> {
  const cpu = panel.getByRole('region', { name: 'CPU', exact: true });
  await expect(cpu).toContainText(/\d+\.\d\d ms of \d+ CPU \(\d+ %\)/);
  await expect(cpu.getByRole('definition').nth(2)).toHaveText(/^\d+ CPU$/);
  await expect(panel.getByRole('progressbar', { name: 'CPU of the latest run' })).toBeVisible();
  const memory = panel.getByRole('region', { name: 'Memory', exact: true });
  // Below 1 KB the size reads in bytes (`38 B`), from 1 KB on in KB (`38.1 KB`).
  await expect(memory).toContainText(/(\d+ B|\d+\.\d KB) of 2048 KB \(\d+ %\)/);
  await expect(panel.getByRole('progressbar', { name: 'Memory of the latest run' })).toBeVisible();
  await expect(panel.getByRole('img', { name: 'CPU of the recent runs against the limit' })).toBeVisible();
  await expect(panel.getByRole('status')).toContainText(/Latest run \d+ s ago/);
  await expect(panel.getByRole('region', { name: 'Shards' })).toHaveCount(0);
}

test.describe('usage', { tag: '@openscreeps' }, () => {
  test('the status bar shows the latest run and opens the Usage panel', async ({
    signedInPage: page,
  }) => {
    const button = await usageButton(page);
    // A wide window shows both values against their limits, with bars.
    await expect(button).toContainText(/CPU\s*\d+\.\d \/ \d+/, { useInnerText: true });
    await expect(button).toContainText(/Memory\s*(\d+ B|\d+\.\d KB) \/ 2048 KB/, {
      useInnerText: true,
    });
    await expect(button).toHaveAttribute('title', /^Latest run: CPU \d+\.\d\d ms of \d+ CPU/);
    await button.click();
    const panel = page.getByRole('dialog', { name: 'Usage' });
    await expect(panel).toBeVisible();
    await expect(button).toHaveAttribute('aria-expanded', 'true');
    await expectRun(panel);
    // The button closes it again; a press outside and Escape close it too.
    await button.click();
    await expect(panel).toHaveCount(0);
    await button.click();
    await expect(panel).toBeVisible();
    await status(page).getByText('Connected', { exact: true }).click();
    await expect(panel).toHaveCount(0);
    await button.click();
    await expect(panel).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
  });

  test('the account menu opens the Usage panel under the top bar', async ({
    signedInPage: page,
    player,
  }) => {
    await usageButton(page);
    await accountMenu(page, player.username, 'Usage');
    const panel = page.getByRole('dialog', { name: 'Usage' });
    await expect(panel).toBeVisible();
    await expectRun(panel);
    const top = await page.getByRole('banner').boundingBox();
    const box = await panel.boundingBox();
    if (!top || !box) throw new Error('the top bar or the panel has no box');
    expect(box.y).toBeGreaterThanOrEqual(top.y + top.height);
    // One panel of the account menu at a time: Rate limits replaces it.
    await accountMenu(page, player.username, 'Rate limits');
    await expect(page.getByRole('dialog', { name: 'Rate limits' })).toBeVisible();
    await expect(panel).toHaveCount(0);
  });

  test.describe('on a phone', () => {
    test.use({ viewport: { width: 390, height: 844 } });

    test('the usage stays compact and its panel fits the screen', async ({
      signedInPage: page,
    }) => {
      const button = await usageButton(page);
      // The status bar keeps the connection and the tick; the usage shows a percentage.
      await expect(status(page).getByTitle('Game tick')).toBeVisible({ timeout: 30_000 });
      await expect(button).toHaveText(/\d+%|limit|idle/, { useInnerText: true });
      const bar = await status(page).boundingBox();
      const own = await button.boundingBox();
      if (!bar || !own) throw new Error('the status bar or the button has no box');
      expect(own.x + own.width).toBeLessThanOrEqual(390);
      await button.click();
      const panel = page.getByRole('dialog', { name: 'Usage' });
      await expectRun(panel);
      const box = await panel.boundingBox();
      if (!box) throw new Error('the panel has no box');
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width).toBeLessThanOrEqual(390);
      expect(box.y + box.height).toBeLessThanOrEqual(bar.y);
      expect(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    });
  });
});
