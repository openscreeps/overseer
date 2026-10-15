// The servers screen: the built-in screeps.com entry, adding and removing a private server,
// and the list surviving a reload.

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
    await page.reload();
    await expect(servers.getByRole('button', { name: 'Sign in screeps.com' })).toBeVisible();
    await expect(servers.getByRole('listitem').filter({ hasText: server.url })).toHaveCount(0);
  });

  test('opens the sign-in form of a saved server', async ({ page, server }) => {
    await addServer(page, server);
    await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(form.getByLabel('Username or e-mail')).toBeVisible();
    await form.getByRole('button', { name: '← Choose another server' }).click();
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });
});
