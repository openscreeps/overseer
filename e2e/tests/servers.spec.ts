// The servers screen and the top bar's server switcher: the built-in screeps.com entry, adding
// (the "Add server" dialog: Season preset, private servers, address checks) and removing
// servers, the list surviving a reload, what each entry says about its account, unknown server
// links, and switching between saved servers.
//
// Saving servers only touches the browser's storage, so the tests that need no account use an
// address nobody answers on (`OFFLINE`) and run on every target.

import type { Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, signIn, test } from './fixtures';
import { storedToken } from './support/accounts';

/** A private server address the app never has to reach in these tests. */
const OFFLINE = 'http://127.0.0.1:9/';

/** Adds a private server called `name` at `address` through the "Add server" dialog. */
async function add(page: Page, name: string, address: string): Promise<void> {
  await page
    .getByRole('region', { name: 'Servers' })
    .getByRole('button', { name: 'Add server' })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Add server' });
  await dialog.getByLabel('Name', { exact: true }).fill(name);
  await dialog.getByLabel('Address', { exact: true }).fill(address);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
}

test.describe('servers', () => {
  test('lists screeps.com and adds, keeps and removes a private server', async ({ page }) => {
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    const official = servers.getByRole('listitem').filter({ hasText: 'https://screeps.com/' });
    await expect(official).toContainText('Official server');
    await expect(official.getByRole('button', { name: 'Sign in screeps.com' })).toBeVisible();
    // The built-in server cannot be removed.
    await expect(official.getByRole('button', { name: /^Remove/ })).toHaveCount(0);

    await add(page, 'Fictional', OFFLINE);
    await expect(page.getByRole('status').filter({ hasText: 'Added Fictional' })).toBeVisible();
    const entry = servers.getByRole('listitem').filter({ hasText: OFFLINE });
    await expect(entry).toContainText('Fictional');
    await expect(entry).toContainText('Private server');
    await expect(entry).toContainText('Signed out');

    await page.reload();
    await expect(entry.getByRole('button', { name: 'Sign in Fictional' })).toBeVisible();

    await entry.getByRole('button', { name: 'Remove Fictional' }).click();
    await expect(entry).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: 'Removed Fictional' })).toBeVisible();
    await page.reload();
    await expect(servers.getByRole('button', { name: 'Sign in screeps.com' })).toBeVisible();
    await expect(entry).toHaveCount(0);
  });

  test('adds servers through a compact dialog', async ({ page }) => {
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    const open = servers.getByRole('button', { name: 'Add server' });
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    // The form is not on the page until asked for.
    await expect(dialog).toBeHidden();
    await expect(page.getByLabel('Address', { exact: true })).toBeHidden();

    // Escape, Cancel or Close closes it; it opens empty again.
    await open.click();
    await expect(dialog).toBeVisible();
    await dialog.getByLabel('Name', { exact: true }).fill('Half typed');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await open.click();
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('');
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toBeHidden();

    // A rejected address keeps the dialog open with the reason.
    await open.click();
    await dialog.getByLabel('Name', { exact: true }).fill('Broken');
    await dialog.getByLabel('Address', { exact: true }).fill('not a url');
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog.getByRole('alert')).toHaveText(/^Invalid address: /);
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toBeHidden();
    await expect(servers.getByRole('listitem').filter({ hasText: 'Broken' })).toHaveCount(0);

    // The Season preset needs no fields; a toast confirms, the list shows it.
    await open.click();
    await dialog.getByLabel('Server', { exact: true }).selectOption({ label: 'Season (screeps.com)' });
    await expect(dialog.getByLabel('Address', { exact: true })).toBeHidden();
    await expect(dialog).toContainText("screeps.com's seasonal world");
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('status').filter({ hasText: 'Added Season' })).toBeVisible();
    const season = servers.getByRole('listitem').filter({ hasText: 'https://screeps.com/season/' });
    await expect(season).toContainText('Seasonal server');
    // Only once.
    await open.click();
    await dialog.getByLabel('Server', { exact: true }).selectOption({ label: 'Season (screeps.com)' });
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog.getByRole('alert')).toHaveText(
      'https://screeps.com/season/ is already listed as Season',
    );
  });

  test('checks and completes the address of a private server', async ({ page }) => {
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    const name = dialog.getByLabel('Name', { exact: true });
    const address = dialog.getByLabel('Address', { exact: true });
    const addButton = dialog.getByRole('button', { name: 'Add', exact: true });
    await servers.getByRole('button', { name: 'Add server' }).click();

    for (const [nameText, addressText, reason] of [
      ['', OFFLINE, 'Enter a name for the server'],
      ['Example', '', 'Enter the server address'],
      ['Example', 'ftp://example.test/', 'The address must be an http:// or https:// URL'],
      ['Example', 'http://me:secret@example.test/', 'The address must not contain credentials'],
      ['Example', 'http://example.test/?shard=1', 'The address must not contain a query or fragment'],
      ['Example', 'http://example.test/#world', 'The address must not contain a query or fragment'],
      ['Example', 'https://screeps.com', 'https://screeps.com/ is already listed as screeps.com'],
    ]) {
      await name.fill(nameText);
      await address.fill(addressText);
      await addButton.click();
      await expect(dialog.getByRole('alert')).toHaveText(reason);
    }

    // A bare host and port becomes an http:// address ending in a slash; a path is kept.
    await name.fill('Example');
    await address.fill('example.test:21025');
    await addButton.click();
    await expect(dialog).toBeHidden();
    await expect(servers.getByRole('listitem').filter({ hasText: 'Example' })).toContainText(
      'http://example.test:21025/',
    );
    await add(page, 'Prefixed', 'https://example.test/season');
    await expect(servers.getByRole('listitem').filter({ hasText: 'Prefixed' })).toContainText(
      'https://example.test/season/',
    );
    // The same server again, however written, is refused.
    await add(page, 'Again', 'http://example.test:21025');
    await expect(dialog.getByRole('alert')).toHaveText(
      'http://example.test:21025/ is already listed as Example',
    );
  });

  test('opens the sign-in form of a saved server', async ({ page }) => {
    await page.goto('/#/');
    await add(page, 'Fictional', OFFLINE);
    await page.getByRole('button', { name: 'Sign in Fictional' }).click();
    await expect(page).toHaveURL(/#\/127\.0\.0\.1-9\/$/);
    const form = page.getByRole('region', { name: 'Sign in to Fictional' });
    await expect(form).toContainText(OFFLINE);
    await expect(form.getByLabel('Username or e-mail')).toBeVisible();
    await form.getByRole('button', { name: 'Choose another server' }).click();
    await expect(page).toHaveURL(/#\/$/);
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });

  test('says when a link names a server that is not saved', async ({ page }) => {
    await page.goto('/#/nowhere.test-1/W1N1');
    const alert = page.getByRole('alert').filter({ hasText: 'Unknown server' });
    await expect(alert.getByRole('heading', { name: 'Unknown server' })).toBeVisible();
    await expect(alert).toContainText(
      'No saved server is called “nowhere.test-1” here. Add it, then open the link again.',
    );
    await alert.getByRole('button', { name: 'Servers' }).click();
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });

  test('a signed-in server names its account and opens its world', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(status).toContainText('Connected');
    await page.goto('/#/');
    const entry = page
      .getByRole('region', { name: 'Servers' })
      .getByRole('listitem')
      .filter({ hasText: server.url });
    await expect(entry).toContainText(`Signed in as ${player.username}`);
    // The session stays connected while the list is shown.
    await expect(status).toContainText('Connected');
    await entry.getByRole('button', { name: `Open ${SERVER_NAME}` }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
    await expect(page.getByRole('button', { name: `Account: ${player.username}` })).toBeVisible();
    await expect(page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` })).toHaveCount(0);
  });

  test('removing a signed-in server forgets its session', { tag: '@openscreeps' }, async ({
    page,
    server,
    account,
  }) => {
    test.fail(
      true,
      'app bug: removing the live session’s server deletes its token but keeps the session ' +
        'running, so opening it again shows its world signed in (as "Account")',
    );
    await signIn(page, server, account);
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    expect(await storedToken(page, server.url)).not.toBeNull();
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    await servers.getByRole('button', { name: `Remove ${SERVER_NAME}` }).click();
    await expect(page.getByRole('status').filter({ hasText: `Removed ${SERVER_NAME}` })).toBeVisible();
    await expect.poll(() => storedToken(page, server.url)).toBeNull();

    // Added again, it is signed out.
    await addServer(page, server);
    const entry = servers.getByRole('listitem').filter({ hasText: server.url });
    await expect(entry).toContainText('Signed out');
    await entry.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
    await expect(page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` })).toBeVisible();
  });
});

test.describe('server switcher', () => {
  test('names the current server and its account, and manages servers', async ({
    signedInPage: page,
    player,
  }) => {
    const switcher = page.getByRole('button', { name: `Server: ${SERVER_NAME}` });
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await switcher.click();
    const current = page.getByRole('menuitemradio', { name: new RegExp(`^${SERVER_NAME}`) });
    await expect(current).toHaveAttribute('aria-checked', 'true');
    await expect(current).toContainText(player.username);
    await page.getByRole('menuitem', { name: 'Manage servers…' }).click();
    await expect(page).toHaveURL(/#\/$/);
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Server: none chosen' })).toBeVisible();
  });

  test('switches to another server and back to the live session', { tag: '@openscreeps' }, async ({
    page,
    server,
    account,
  }) => {
    test.fail(
      true,
      'app bug: back on the live session’s server, the app still shows the sign-in form of the ' +
        'signed-out server visited before (its screen is not reset)',
    );
    await signIn(page, server, account);
    const status = page.getByRole('contentinfo', { name: 'Status' });
    const accountButton = page.getByRole('button', { name: `Account: ${account.username}` });
    await expect(status).toContainText('Connected');

    // screeps.com is signed out: its sign-in form, without contacting it.
    await page.getByRole('button', { name: `Server: ${SERVER_NAME}` }).click();
    const official = page.getByRole('menuitemradio', { name: /^screeps\.com/ });
    await expect(official).toHaveAttribute('aria-checked', 'false');
    await expect(official).toContainText('signed out');
    await official.click();
    await expect(page).toHaveURL(/#\/screeps\.com\/$/);
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Server: screeps.com' })).toBeVisible();

    // Back: the live session's world, still signed in.
    await page.getByRole('button', { name: 'Server: screeps.com' }).click();
    await page.getByRole('menuitemradio', { name: new RegExp(`^${SERVER_NAME}`) }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toHaveCount(0);
    await expect(accountButton).toBeVisible();
    await expect(status).toContainText('Connected');
  });
});
