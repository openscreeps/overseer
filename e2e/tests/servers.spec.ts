// The servers screen: the built-in screeps.com entry, adding (through the "Add server" dialog)
// and removing a private server, and the list surviving a reload.

import { addServer, expect, SERVER_NAME, test } from './fixtures';

test.describe('servers', () => {
  test('lists screeps.com and adds, keeps and removes a private server', async ({ page, server }) => {
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    await expect(servers.getByRole('button', { name: 'Sign in screeps.com' })).toBeVisible();

    await addServer(page, server);
    const entry = servers.getByRole('listitem').filter({ hasText: server.url });
    await expect(entry).toContainText(SERVER_NAME);
    await expect(entry).toContainText('Signed out');

    await page.reload();
    await expect(entry.getByRole('button', { name: `Sign in ${SERVER_NAME}` })).toBeVisible();

    await entry.getByRole('button', { name: `Remove ${SERVER_NAME}` }).click();
    await expect(servers.getByRole('listitem').filter({ hasText: server.url })).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: `Removed ${SERVER_NAME}` })).toBeVisible();
    await page.reload();
    await expect(servers.getByRole('button', { name: 'Sign in screeps.com' })).toBeVisible();
    await expect(servers.getByRole('listitem').filter({ hasText: server.url })).toHaveCount(0);
  });

  test('adds servers through a compact dialog', async ({ page, server }) => {
    await page.goto('/#/');
    const servers = page.getByRole('region', { name: 'Servers' });
    const open = servers.getByRole('button', { name: 'Add server' });
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    // The form is not on the page until asked for.
    await expect(dialog).toBeHidden();
    await expect(page.getByLabel('Address', { exact: true })).toBeHidden();

    // Escape (or Cancel) closes it; it opens empty again.
    await open.click();
    await expect(dialog).toBeVisible();
    await dialog.getByLabel('Name', { exact: true }).fill('Half typed');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await open.click();
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('');

    // A rejected address keeps the dialog open with the reason.
    await dialog.getByLabel('Name', { exact: true }).fill('Broken');
    await dialog.getByLabel('Address', { exact: true }).fill('not a url');
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog.getByRole('alert').filter({ hasText: /\S/ })).toBeVisible();
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toBeHidden();
    await expect(servers.getByRole('listitem').filter({ hasText: 'Broken' })).toHaveCount(0);

    // The Season preset needs no fields; a toast confirms, the list shows it.
    await open.click();
    await dialog.getByLabel('Server', { exact: true }).selectOption({ label: 'Season (screeps.com)' });
    await expect(dialog.getByLabel('Address', { exact: true })).toBeHidden();
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('status').filter({ hasText: 'Added Season' })).toBeVisible();
    await expect(servers.getByRole('listitem').filter({ hasText: 'screeps.com/season/' })).toBeVisible();

    // And a private server.
    await addServer(page, server);
    await expect(servers.getByRole('listitem').filter({ hasText: server.url })).toBeVisible();
  });

  test('opens the sign-in form of a saved server', async ({ page, server }) => {
    await addServer(page, server);
    await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(form.getByLabel('Username or e-mail')).toBeVisible();
    await form.getByRole('button', { name: 'Choose another server' }).click();
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });
});
