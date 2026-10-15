// The profile: what it shows about the account, and editing notification preferences, the
// e-mail address and the password (each checked against the server).

import type { Page } from '@playwright/test';
import { expect, Session, signIn, test, unique, type Account } from './fixtures';

/** Opens the profile through the account menu. */
async function openProfile(page: Page, account: Account) {
  // The session settles first (it rewrites the saved profile once connected).
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  await page.getByRole('button', { name: `Account: ${account.username}` }).click();
  await page.getByRole('menuitem', { name: 'Profile' }).click();
  await expect(page.getByRole('heading', { level: 1, name: account.username })).toBeVisible();
}

test.describe('profile', () => {
  test("shows the player's account", async ({ signedInPage: page, player }) => {
    await openProfile(page, player);
    const profile = page.getByRole('region', { name: player.username });
    await expect(profile.getByRole('img', { name: `Badge of ${player.username}` })).toBeVisible();
    await expect(profile.getByRole('progressbar', { name: /GCL progress/ })).toBeVisible();
    await expect(profile.getByRole('definition').filter({ hasText: 'CPU' }).first()).toBeVisible();
    await expect(profile).toContainText('Playing');
    await expect(profile).toContainText(player.room);
  });

  test('saves notification preferences', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account);
    const notifications = page.getByRole('region', { name: 'Notifications' });
    const messages = notifications.getByRole('checkbox', { name: 'Notify about new messages' });
    await expect(messages).toBeChecked();
    await messages.uncheck();
    await notifications.getByLabel('Send interval').selectOption({ label: '3 hours' });
    await notifications.getByRole('button', { name: 'Save notifications' }).click();
    await expect(notifications.getByRole('status')).toHaveText('Notification preferences saved.');

    await page.reload();
    await expect(messages).not.toBeChecked();
    await expect(notifications.getByLabel('Send interval')).toHaveValue(/./);
    await expect(notifications.getByLabel('Send interval').locator('option:checked')).toHaveText(
      '3 hours',
    );
  });

  test('changes the e-mail address', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account);
    const section = page.getByRole('region', { name: 'E-mail' });
    const address = `${unique('mail')}@e2e.invalid`;
    await section.getByLabel('E-mail address').fill(address);
    await section.getByRole('button', { name: 'Save e-mail' }).click();
    await expect(section.getByRole('status')).toHaveText('E-mail address saved.');
    // The server signs the account in with the new address.
    await Session.signIn(server.url, address, account.password);
  });

  test('changes the password and stays signed in', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account);
    const section = page.getByRole('region', { name: 'Password' });
    const password = unique('new-pass-');
    await section.getByLabel('Current password').fill(account.password);
    await section.getByLabel('New password', { exact: true }).fill(password);
    await section.getByLabel('Repeat the new password').fill(password);
    await section.getByRole('button', { name: 'Change password' }).click();
    // Private servers revoke every session of the account: the app signs in again and says so.
    await expect(page.getByRole('alert').filter({ hasText: 'Password changed' })).toHaveText(
      /Password changed; signed in again with the new password\./,
    );
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');

    await Session.signIn(server.url, account.username, password);
    await expect(Session.signIn(server.url, account.username, account.password)).rejects.toThrow();
  });

  test('rejects a wrong current password', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account);
    const section = page.getByRole('region', { name: 'Password' });
    await section.getByLabel('Current password').fill(`${account.password}-wrong`);
    await section.getByLabel('New password', { exact: true }).fill('long-enough-1');
    await section.getByLabel('Repeat the new password').fill('long-enough-1');
    await section.getByRole('button', { name: 'Change password' }).click();
    await expect(section.getByRole('alert').filter({ hasText: /\S/ })).toBeVisible();
    await Session.signIn(server.url, account.username, account.password);
  });
});
