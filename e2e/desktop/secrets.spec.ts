// Secrets in the system keychain (Windows Credential Manager; the test build's service
// `overseer:<identifier>`): the session token after signing in and a server's password, never
// in the page's storage; they survive a restart, and signing out or removing the server deletes
// them.

import { expect, keychain, open, signIn, statusBar, test } from './fixtures';
import type { Page } from '@playwright/test';

/** The secrets the page's own storage holds (the web build's store). */
const pageSecrets = (page: Page) =>
  page.evaluate(() => Object.keys(localStorage).filter((key) => key.startsWith('overseer.secret.')));

test.describe('keychain', () => {
  test('keeps the session across a restart; signing out deletes it', async ({
    app,
    server,
    credentials,
  }) => {
    let page = await app.launch();
    await signIn(page, server, credentials);
    await expect.poll(keychain).toEqual([`token:${server.url}`]);
    expect(await pageSecrets(page)).toEqual([]);

    page = await app.restart();
    const account = page.getByRole('button', { name: `Account: ${credentials.username}` });
    await expect(account).toBeVisible({ timeout: 30_000 });
    await expect(statusBar(page)).toContainText('Connected', { timeout: 30_000 });

    await account.click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
    await expect.poll(keychain).toEqual([]);

    page = await app.restart();
    await open(page, '#/');
    await expect(
      page
        .getByRole('region', { name: 'Servers' })
        .getByRole('listitem')
        .filter({ hasText: server.url }),
    ).toContainText('Signed out');
  });

  test("keeps a server's password and forgets it with the server", async ({ app }) => {
    // Nothing contacts this address: adding a server only saves it.
    const url = 'http://127.0.0.1:9/';
    const password = 'e2e-server-password';
    let page = await app.launch();
    await open(page, '#/');
    await page
      .getByRole('region', { name: 'Servers' })
      .getByRole('button', { name: 'Add server' })
      .click();
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    await dialog.getByLabel('Name', { exact: true }).fill('Protected');
    await dialog.getByLabel('Address', { exact: true }).fill(url);
    const field = dialog.getByLabel('Server password (optional)');
    await expect(field).toHaveAccessibleDescription(/kept in the system keychain/);
    await field.fill(password);
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Added Protected' })).toBeVisible();
    await expect.poll(keychain).toEqual([`server-password:${url}`]);
    expect(await pageSecrets(page)).toEqual([]);
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(password);

    page = await app.restart();
    await open(page, '#/');
    const remove = page.getByRole('button', { name: 'Remove Protected' });
    await expect(remove).toBeVisible();
    expect(await keychain()).toEqual([`server-password:${url}`]);
    await remove.click();
    await expect(page.getByRole('status').filter({ hasText: 'Removed Protected' })).toBeVisible();
    await expect.poll(keychain).toEqual([]);
  });
});
