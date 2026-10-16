// The console: the bot's output live, expressions and their results, the command history
// across reloads, filters, clear, and the console pop-out window.

import type { Page } from '@playwright/test';
import { expect, test } from './fixtures';

/** Opens the world on the player's room with the Console tab. */
async function openConsole(page: Page, slug: string, room: string) {
  await page.goto(`/#/${slug}/-/${room}`);
  await page.getByRole('tab', { name: /Console/ }).click();
  await expect(page.getByRole('tab', { name: /Console/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
}

/** The console output. */
const output = (page: Page) => page.getByRole('log', { name: 'Console output' });

/** The highest `tick N` the output shows. */
async function lastTick(page: Page): Promise<number> {
  const text = (await output(page).textContent()) ?? '';
  const ticks = [...text.matchAll(/tick (\d+)/g)].map((match) => Number(match[1]));
  return ticks.length ? Math.max(...ticks) : 0;
}

test.describe('console', () => {
  test('shows the bot live, runs an expression and recalls it after a reload', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openConsole(page, server.slug, player.room);
    await expect(output(page)).toContainText(/tick \d+/, { timeout: 30_000 });
    const first = await lastTick(page);
    await expect.poll(() => lastTick(page), { timeout: 30_000 }).toBeGreaterThan(first);

    const input = page.getByRole('textbox', { name: 'Console expression' });
    const expression = `'e2e-' + (6 * 7)`;
    await input.fill(expression);
    await input.press('Enter');
    await expect(input).toHaveValue('');
    await expect(output(page).locator('[data-kind="command"]')).toContainText(expression);
    await expect(output(page).locator('[data-kind="result"]')).toContainText('e2e-42', {
      timeout: 30_000,
    });

    // Shift+Enter adds a line instead of running.
    await input.fill('1');
    await input.press('Shift+Enter');
    await input.pressSequentially('+ 1');
    await expect(input).toHaveValue('1\n+ 1');
    await input.fill('');

    await page.reload();
    const again = page.getByRole('textbox', { name: 'Console expression' });
    await expect(page.getByRole('tab', { name: /Console/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await again.click();
    await again.press('ArrowUp');
    await expect(again).toHaveValue(expression);
    await again.press('ArrowDown');
    await expect(again).toHaveValue('');
  });

  test('filters by kind and text, and clears', async ({ signedInPage: page, server, player }) => {
    await openConsole(page, server.slug, player.room);
    await expect(output(page)).toContainText(/tick \d+/, { timeout: 30_000 });

    const filter = page.getByRole('searchbox', { name: 'Filter output' });
    await filter.fill('no line says this');
    await expect(output(page)).toContainText('No lines match the filter.');
    await filter.fill('');
    await expect(output(page)).toContainText(/tick \d+/);

    const log = page.getByRole('button', { name: 'Log', exact: true });
    await log.click();
    await expect(log).toHaveAttribute('aria-pressed', 'false');
    await expect(output(page).locator('[data-kind="log"]')).toHaveCount(0);
    await log.click();
    await expect(output(page).locator('[data-kind="log"]').first()).toBeVisible();

    const lines = output(page).locator('[data-kind]');
    await expect.poll(() => lines.count(), { timeout: 30_000 }).toBeGreaterThan(3);
    const before = await lines.count();
    await page.getByRole('button', { name: 'Clear console' }).click();
    expect(await lines.count()).toBeLessThan(before);
    // New output arrives after the clear.
    await expect(output(page)).toContainText(/tick \d+/, { timeout: 30_000 });
  });

  test('pops the console out into a live window', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openConsole(page, server.slug, player.room);
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.getByRole('button', { name: 'Pop out console' }).click(),
    ]);
    await expect(popup).toHaveURL(new RegExp(`#/${server.slug}/-/console/window$`));
    await expect(popup.getByRole('heading', { level: 1 })).toContainText('Console');
    const log = popup.getByRole('log', { name: 'Console output' });
    await expect(log).toContainText(/tick \d+/, { timeout: 30_000 });
    const first = await lastTick(popup);
    await expect.poll(() => lastTick(popup), { timeout: 30_000 }).toBeGreaterThan(first);
    await popup.close();
  });
});
