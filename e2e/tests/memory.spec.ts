// The drawer's Memory tab: the player's Memory as a tree (the bot counts ticks in
// `Memory.ticks`), Reload, the path field, a new key saved from the editor (the player's code
// applies the write on its next tick: the tab waits for it), and its deletion after a
// confirmation. Each result shows again after a page reload, from the server. The console
// completes the keys of the same Memory: a new key shows there without a second read.
//
// The tests need the player's bot and write Memory, so they are `@openscreeps`.

import type { Locator, Page } from '@playwright/test';
import { expect, test } from './fixtures';
import { selectTab } from './support/room';

/** How long a write may take to show: a few ticks of the disposable world. */
const WRITE_TIMEOUT = 45_000;

/** The Memory tab. */
const memory = (page: Page) => page.getByRole('region', { name: 'Memory' });

/** The tree of the Memory tab. */
const tree = (page: Page) => memory(page).getByRole('group', { name: 'Memory tree' });

/** The row of `key` in `tree` (named `<key> = <preview>`). */
const row = (tree: Locator, key: string, preview: RegExp | string = /.*/) =>
  tree.getByRole('button', {
    name: new RegExp(`^${key} = ${typeof preview === 'string' ? preview : preview.source}$`),
  });

/** Opens the world at `path` on the Memory tab and waits until the bot's ticks show. */
async function openMemory(page: Page, path: string) {
  await page.goto(path);
  await selectTab(page, 'Memory');
  // The bot writes `Memory.ticks` once its code runs: reload until it is there.
  await expect(async () => {
    await memory(page).getByRole('button', { name: 'Reload' }).click();
    await expect(row(tree(page), 'ticks', /\d+/)).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 30_000 });
}

/** The value of `Memory.ticks` the tree shows. */
async function ticks(page: Page): Promise<number> {
  const name = await row(tree(page), 'ticks', /\d+/).getAttribute('aria-label');
  return Number(name?.match(/= (\d+)$/)?.[1] ?? 0);
}

test.describe('memory', { tag: '@openscreeps' }, () => {
  test('shows the bot’s Memory, reloads it, and shows the value at a path', async ({
    signedInPage: page,
    home,
  }) => {
    await openMemory(page, home.path);
    const first = await ticks(page);
    // Reload reads Memory again: the bot counted more ticks meanwhile.
    await expect
      .poll(
        async () => {
          await memory(page).getByRole('button', { name: 'Reload' }).click();
          return ticks(page);
        },
        { timeout: 30_000 },
      )
      .toBeGreaterThan(first);
    await expect(memory(page).getByRole('status').filter({ hasText: /^Loaded at / })).toBeVisible();

    // The path field shows the value at a path alone; a wrong path says why.
    const path = memory(page).getByRole('searchbox', { name: 'Path' });
    await path.fill('creeps..x');
    await path.press('Enter');
    const wrong = memory(page).getByRole('alert').filter({ hasText: /no empty parts/ });
    await expect(wrong).toBeVisible();
    await path.fill('Memory.ticks');
    await path.press('Enter');
    await expect(tree(page).getByText('ticks', { exact: true })).toBeVisible();
    await expect(row(tree(page), 'ticks')).toHaveCount(0);
    await path.fill('');
    await path.press('Enter');
    await expect(row(tree(page), 'ticks', /\d+/)).toBeVisible();
  });

  test('adds a key, shows it once the code applied it, and deletes it', async ({
    signedInPage: page,
    home,
  }) => {
    await openMemory(page, home.path);
    await tree(page).getByRole('button', { name: 'Add key' }).click();
    const editor = memory(page).getByRole('region', { name: 'Value editor' });
    await editor.getByRole('textbox', { name: 'Key' }).fill('e2e');
    const value = editor.getByRole('textbox', { name: 'Value (JSON)' });

    // Text that is not JSON is not sent.
    await value.fill('{a: 1}');
    await editor.getByRole('button', { name: 'Save' }).click();
    await expect(editor.getByText(/^Not valid JSON: /)).toBeVisible();

    await value.fill('{"a": 1}');
    await editor.getByRole('button', { name: 'Save' }).click();
    const added = row(tree(page), 'e2e', '\\{1 key\\}');
    await expect(added).toBeVisible({ timeout: WRITE_TIMEOUT });
    await expect(memory(page).getByText('e2e is now in Memory.')).toBeVisible();
    // The new key is selected: the editor shows its value.
    await expect(added).toHaveAttribute('aria-current', 'true');
    await expect(editor.getByRole('heading', { name: 'e2e' })).toBeVisible();

    // The console completes the new key from the Memory the tab holds: it reads no Memory.
    const rootReads: string[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (
        request.method() === 'GET' &&
        url.pathname.endsWith('/api/user/memory') &&
        !url.searchParams.get('path')
      ) {
        rootReads.push(request.url());
      }
    });
    await selectTab(page, 'Console');
    const input = page.getByRole('textbox', { name: 'Console expression' });
    await input.pressSequentially('Memory.e2');
    await expect(
      page
        .getByRole('listbox', { name: 'Suggestions' })
        .getByRole('option', { name: 'e2e, object', exact: true }),
    ).toBeVisible();
    expect(rootReads).toEqual([]);
    await input.fill('');
    await selectTab(page, 'Memory');

    // The server has it: it shows after a page reload too, with its child.
    await page.reload();
    await expect(added).toBeVisible({ timeout: 30_000 });
    await tree(page).getByRole('button', { name: 'Children of e2e' }).click();
    await expect(row(tree(page), 'a', '1')).toBeVisible();

    // Delete asks first; Cancel keeps the key.
    await added.click();
    await editor.getByRole('button', { name: 'Delete', exact: true }).click();
    const confirm = editor.getByRole('group', { name: 'Confirm deletion' });
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirm).toHaveCount(0);
    await editor.getByRole('button', { name: 'Delete', exact: true }).click();
    await confirm.getByRole('button', { name: 'Delete for good' }).click();
    await expect(row(tree(page), 'e2e')).toHaveCount(0, { timeout: WRITE_TIMEOUT });
    await expect(editor).toHaveCount(0);

    await page.reload();
    await expect(row(tree(page), 'ticks', /\d+/)).toBeVisible({ timeout: 30_000 });
    await expect(row(tree(page), 'e2e')).toHaveCount(0);
  });
});
