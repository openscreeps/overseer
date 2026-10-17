// The console: the bot's output live, expressions and their results, the command history
// across reloads, how output is rendered (a safe subset of HTML, never markup), filters by kind
// and text, clear, auto-scroll and its pause, the shard selector, and the console window.
//
// Running an expression posts to the server, so every test that runs one (or needs the
// player's bot to log) is `@openscreeps`.

import type { Page } from '@playwright/test';
import { expect, signIn, test } from './fixtures';
import { consoleOutput as output, selectTab } from './support/room';

/** Opens the world at `path` on the Console tab. */
async function openConsole(page: Page, path: string) {
  await page.goto(path);
  await selectTab(page, 'Console');
}

/**
 * The ticks of the bot's `tick N` lines the output shows, read from the rendered text (one
 * line per output line: the text content runs a line's tick into the next line's time).
 */
async function ticks(page: Page): Promise<number[]> {
  const text = await output(page).evaluate((element) => (element as HTMLElement).innerText);
  return [...text.matchAll(/tick (\d+)$/gm)].map((match) => Number(match[1]));
}

/** The highest `tick N` the output shows. */
async function lastTick(page: Page): Promise<number> {
  return Math.max(0, ...(await ticks(page)));
}

/** Runs `expression` from the console input (Enter). */
async function run(page: Page, expression: string) {
  const input = page.getByRole('textbox', { name: 'Console expression' });
  await input.fill(expression);
  await input.press('Enter');
  await expect(input).toHaveValue('');
}

test.describe('console', { tag: '@transport' }, () => {
  test('offers the shard to run on only on sharded servers, and edits multi-line input', async ({
    signedInPage: page,
    home,
  }) => {
    await openConsole(page, home.path);
    const console = page.getByRole('region', { name: 'Console' });
    const shard = console.getByRole('combobox', { name: 'Shard' });
    if (home.shard === null) await expect(shard).toHaveCount(0);
    else await expect(shard).toHaveValue(home.shard);
    await expect(console.getByRole('button', { name: 'Run' })).toBeVisible();

    // Shift+Enter adds a line instead of running, and the input grows with it.
    const input = console.getByRole('textbox', { name: 'Console expression' });
    await input.fill('1');
    await input.press('Shift+Enter');
    await input.pressSequentially('+ 1');
    await expect(input).toHaveValue('1\n+ 1');
    await expect(input).toHaveAttribute('rows', '2');
    await input.fill('');
    await expect(input).toHaveAttribute('rows', '1');
  });

  test.describe('with the player’s bot', { tag: '@openscreeps' }, () => {
    test('shows the bot live, runs an expression and recalls it after a reload', async ({
      signedInPage: page,
      home,
    }) => {
      await openConsole(page, home.path);
      await expect(output(page)).toContainText(/tick \d+/, { timeout: 30_000 });
      const first = await lastTick(page);
      await expect.poll(() => lastTick(page), { timeout: 30_000 }).toBeGreaterThan(first);

      const expression = `'e2e-' + (6 * 7)`;
      await run(page, expression);
      await expect(output(page)).toContainText(expression);
      await expect(output(page).getByText('e2e-42', { exact: true })).toBeVisible({
        timeout: 30_000,
      });

      await page.reload();
      const again = page.getByRole('textbox', { name: 'Console expression' });
      await expect(page.getByRole('tab', { name: /^Console/ })).toHaveAttribute(
        'aria-selected',
        'true',
      );
      await again.click();
      await again.press('ArrowUp');
      await expect(again).toHaveValue(expression);
      await again.press('ArrowDown');
      await expect(again).toHaveValue('');
    });

    test('renders bold, italic, colours and breaks, and everything else as text', async ({
      signedInPage: page,
      home,
    }) => {
      await openConsole(page, home.path);
      await run(
        page,
        `'<b>bold-e2e</b> <i>italic-e2e</i> <span style="color: #ff0000">red-e2e</span><br>` +
          `next-e2e <img src=x onerror="window.e2eInjected=1"> &lt;tag&gt;'`,
      );
      const log = output(page);
      const bold = log.getByText('bold-e2e', { exact: true });
      await expect(bold).toBeVisible({ timeout: 30_000 });
      expect(Number(await bold.evaluate((element) => getComputedStyle(element).fontWeight)))
        .toBeGreaterThanOrEqual(600);
      const italic = log.getByText('italic-e2e', { exact: true });
      expect(await italic.evaluate((element) => getComputedStyle(element).fontStyle)).toBe('italic');
      const red = log.getByText('red-e2e', { exact: true });
      expect(await red.evaluate((element) => getComputedStyle(element).color)).toBe(
        'rgb(255, 0, 0)',
      );
      // After the break, on a line of its own: the image tag as text, references decoded.
      const next = log.getByText(/^next-e2e /);
      await expect(next).toHaveText('next-e2e <img src=x onerror="window.e2eInjected=1"> <tag>');
      // Both read at once: the log scrolls as the bot's lines arrive.
      const gap = await next.evaluate(
        (below, above) => below.getBoundingClientRect().top - (above?.getBoundingClientRect().bottom ?? 0),
        await red.elementHandle(),
      );
      expect(gap).toBeGreaterThanOrEqual(-1);
      await expect(log.getByRole('img')).toHaveCount(0);
      expect(await page.evaluate(() => 'e2eInjected' in window)).toBe(false);
    });

    test('filters by kind and text, and clears', async ({ signedInPage: page, home }) => {
      await openConsole(page, home.path);
      const log = output(page);
      await expect(log).toContainText(/tick \d+/, { timeout: 30_000 });
      const command = `'e2e-result-' + 7`;
      await run(page, command);
      await expect(log.getByText('e2e-result-7', { exact: true })).toBeVisible({
        timeout: 30_000,
      });
      await run(page, `throw new Error('e2e-boom')`);
      await expect(log).toContainText('Error: e2e-boom', { timeout: 30_000 });

      // Each kind hides alone and comes back.
      const kinds = page.getByRole('group', { name: 'Show' });
      for (const [kind, gone, kept] of [
        ['Log', /tick \d+/, 'e2e-result-7'],
        ['Results', 'e2e-result-7', command],
        ['Errors', 'Error: e2e-boom', 'e2e-result-7'],
        ['Commands', command, 'e2e-result-7'],
      ] as const) {
        const button = kinds.getByRole('button', { name: kind, exact: true });
        await expect(button).toHaveAttribute('aria-pressed', 'true');
        await button.click();
        await expect(button).toHaveAttribute('aria-pressed', 'false');
        await expect(log).not.toContainText(gone);
        await expect(log).toContainText(kept);
        await button.click();
        await expect(button).toHaveAttribute('aria-pressed', 'true');
        await expect(log).toContainText(gone);
      }

      const filter = page.getByRole('searchbox', { name: 'Filter output' });
      await filter.fill('E2E-RESULT');
      await expect(log).not.toContainText(/tick \d+/);
      await expect(log).toContainText('e2e-result-7');
      await filter.fill('no line says this');
      await expect(log).toHaveText('No lines match the filter.');
      await filter.fill('');
      await expect(log).toContainText(/tick \d+/);

      // Clear forgets every line shown so far; new ones keep coming.
      const before = await lastTick(page);
      await page.getByRole('button', { name: 'Clear console' }).click();
      await expect(log).not.toContainText('e2e-result-7');
      expect(Math.min(...(await ticks(page)))).toBeGreaterThan(before);
      await expect(log).toContainText(/tick \d+/, { timeout: 30_000 });
    });

    test('pauses auto-scroll, counts what arrives meanwhile and resumes', async ({
      signedInPage: page,
      home,
    }) => {
      await openConsole(page, home.path);
      const log = output(page);
      await expect(log).toContainText(/tick \d+/, { timeout: 30_000 });
      const atBottom = () =>
        log.evaluate((element) => element.scrollTop + element.clientHeight >= element.scrollHeight - 8);

      await page.getByRole('button', { name: 'Pause auto-scroll' }).click();
      // The scroll event of the app's own last scroll to the bottom may come after the click:
      // the log stays paused.
      await log.dispatchEvent('scroll');
      const resume = page.getByRole('button', { name: 'Resume auto-scroll' });
      await expect(resume).toBeVisible();
      const paused = page.getByRole('button', { name: /^Paused · / });
      await expect(paused).toHaveText(/^Paused · \d+ new lines?$/, { timeout: 30_000 });
      await paused.click();
      await expect(paused).toBeHidden();
      await expect(page.getByRole('button', { name: 'Pause auto-scroll' })).toBeVisible();

      // Output taller than the log, followed to the bottom; scrolling up pauses.
      await run(page, `'e2e-tall' + '<br>line'.repeat(80)`);
      // The result (the command's echo starts with a quote).
      await expect(log.getByText(/^e2e-tall/)).toBeVisible({ timeout: 30_000 });
      expect(await log.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
      await expect.poll(atBottom).toBe(true);
      await log.evaluate((element) => element.scrollTo({ top: 0, behavior: 'instant' }));
      await expect(resume).toBeVisible();
      // It stays where the user scrolled while lines arrive, and counts them.
      await expect(paused).toHaveText(/^Paused · \d+ new lines?$/, { timeout: 30_000 });
      expect(await log.evaluate((element) => element.scrollTop)).toBe(0);
      await resume.click();
      await expect(paused).toBeHidden();
      await expect.poll(atBottom).toBe(true);
    });

    test('pops the console out into a live window', async ({ signedInPage: page, home, server }) => {
      await openConsole(page, home.path);
      const [popup] = await Promise.all([
        page.waitForEvent('popup'),
        page.getByRole('button', { name: 'Pop out console' }).click(),
      ]);
      await expect(popup).toHaveURL(new RegExp(`#/${server.slug}/-/console/window$`));
      await expect(popup.getByRole('heading', { level: 1 })).toContainText('Console');
      await expect(popup).toHaveTitle(/^Console · .+ — Overseer$/);
      const log = output(popup);
      await expect(log).toContainText(/tick \d+/, { timeout: 30_000 });
      const first = await lastTick(popup);
      await expect.poll(() => lastTick(popup), { timeout: 30_000 }).toBeGreaterThan(first);
      // It runs expressions too.
      await run(popup, `'e2e-window-' + 1`);
      await expect(log.getByText('e2e-window-1', { exact: true })).toBeVisible({
        timeout: 30_000,
      });
      await popup.close();
    });
  });

  test(
    'says there is no output before the code logs anything',
    { tag: '@openscreeps' },
    async ({ page, server, account }) => {
      await signIn(page, server, account);
      await selectTab(page, 'Console');
      const log = output(page);
      await expect(log).toHaveText(
        "No output yet: your code's console.log lines and command results show here.",
      );
      // A command shows at once, its result only once code runs (the account has no room).
      await run(page, '20 + 22');
      await expect(log).toContainText('20 + 22');
      await expect(log).not.toContainText('No output yet');
    },
  );
});
