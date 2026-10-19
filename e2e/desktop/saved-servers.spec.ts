// The saved servers are in the profiles file that the desktop app and the CLI share
// (`%APPDATA%\<identifier>\profiles.json`; the CLI's `overseer servers`, `login` and `logout`):
// what the CLI changes shows when the app's window gets the focus back, and what the app
// changes is in the CLI's list. The list that an earlier app kept in the page's `localStorage`
// moves into the file once. The user coming back to the window is its `focus` event.

import { existsSync, readFileSync } from 'node:fs';
import { expect, open, overseer, PROFILES_FILE, test } from './fixtures';
import type { Page } from '@playwright/test';

/** The user comes back to the window. */
const focus = (page: Page) => page.evaluate(() => window.dispatchEvent(new FocusEvent('focus')));

/** The entry of the server with base URL `url` in the server list. */
const entry = (page: Page, url: string) =>
  page.getByRole('region', { name: 'Servers' }).getByRole('listitem').filter({ hasText: url });

/** The base URLs in the profiles file, in its order. */
function fileUrls(): string[] {
  const file = readFileSync(PROFILES_FILE, 'utf8');
  const document: { servers?: { url?: unknown }[] } = JSON.parse(file);
  return (document.servers ?? []).map((server) => String(server.url));
}

test('the app and the CLI share the server list', async ({ app }) => {
  // Nothing contacts these addresses: saving a server only lists it.
  const outside = 'http://127.0.0.1:9/';
  const inside = 'http://127.0.0.1:7/';
  const page = await app.launch();
  await open(page, '#/');
  await expect(entry(page, 'https://screeps.com/')).toBeVisible();

  await overseer(['servers', 'add', outside, '--name', 'Outside']);
  await focus(page);
  await expect(entry(page, outside)).toContainText('Outside');

  await page
    .getByRole('region', { name: 'Servers' })
    .getByRole('button', { name: 'Add server' })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Add server' });
  await dialog.getByLabel('Name', { exact: true }).fill('Inside');
  await dialog.getByLabel('Address', { exact: true }).fill(inside);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Added Inside' })).toBeVisible();
  await expect.poll(() => overseer(['servers'])).toContain(inside);
  // The app's change kept the CLI's.
  expect(fileUrls()).toEqual(['https://screeps.com/', outside, inside]);

  await overseer(['servers', 'remove', 'Inside']);
  await focus(page);
  await expect(entry(page, inside)).toHaveCount(0);
  await expect(entry(page, outside)).toBeVisible();
});

test("moves an earlier app's list from the page's storage into the profiles file", async ({ app }) => {
  const url = 'http://127.0.0.1:9/';
  const page = await app.launch();
  await open(page, '#/');
  await expect(entry(page, 'https://screeps.com/')).toBeVisible();
  // Loading the list writes no file.
  expect(existsSync(PROFILES_FILE)).toBe(false);
  await page.evaluate((url) => {
    const servers = [{ name: 'Earlier', url, kind: 'private', login: 'player' }];
    localStorage.setItem('overseer.profiles', JSON.stringify({ version: 2, servers }));
  }, url);

  // A new start of the page (`open` with the same hash would not load it again).
  await page.reload();
  await expect(entry(page, url)).toContainText('Earlier');
  expect(await page.evaluate(() => localStorage.getItem('overseer.profiles'))).toBeNull();
  expect(fileUrls()).toEqual(['https://screeps.com/', url]);
  expect(await overseer(['servers'])).toContain(url);
});

test('a sign-in with the CLI opens in the app; signing out with the CLI signs the app out', async ({
  app,
  server,
  credentials,
  target,
}) => {
  test.skip(target === 'official', 'the CLI would reach screeps.com outside the request guard');
  if (credentials.kind !== 'password') throw new Error('the disposable worlds give password accounts');
  const page = await app.launch();
  await open(page, '#/');
  await overseer(['servers', 'add', server.url, '--name', server.name]);
  await overseer(['login', server.name, '--email', credentials.login], `${credentials.password}\n`);

  await focus(page);
  const item = entry(page, server.url);
  await expect(item).toContainText(`Signed in as ${credentials.username}`);
  await item.getByRole('button', { name: `Open ${server.name}` }).click();
  const account = page.getByRole('button', { name: `Account: ${credentials.username}` });
  await expect(account).toBeVisible({ timeout: 30_000 });

  await overseer(['logout', server.name]);
  await focus(page);
  await expect(account).toHaveCount(0);
  await expect(
    page.getByRole('region', { name: `Sign in to ${server.name}` }),
  ).toContainText('You signed out in another program');
});
