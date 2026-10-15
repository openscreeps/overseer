// Registration on a private server: live availability checks, field validation, the server's
// own error strings shown as they are, and the automatic sign-in after success.

import type { Locator, Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, test, unique, type Server } from './fixtures';

/** Opens the "Register" tab of the saved server's sign-in screen. */
async function openRegister(page: Page, server: Server): Promise<Locator> {
  await addServer(page, server);
  await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
  await page
    .getByRole('group', { name: 'Account' })
    .getByRole('button', { name: 'Register', exact: true })
    .click();
  return page.getByRole('region', { name: `Register on ${SERVER_NAME}` });
}

// Each field's accessible description holds its error, its live check status and its help.
test.describe('registration', () => {
  test('checks availability while typing', async ({ page, server, account }) => {
    const form = await openRegister(page, server);
    const username = form.getByLabel('Username', { exact: true });
    const email = form.getByLabel('E-mail', { exact: true });

    await username.fill(unique('free'));
    await expect(username).toHaveAccessibleDescription(/Available/);
    await username.fill(account.username);
    await expect(username).toHaveAccessibleDescription(/Already taken/);
    await expect(username).toHaveAttribute('aria-invalid', 'true');

    await email.fill(`${unique('free')}@e2e.invalid`);
    await expect(email).toHaveAccessibleDescription(/Available/);
    await email.fill(account.email);
    await expect(email).toHaveAccessibleDescription(/Already registered/);
  });

  test('validates the fields before submitting', async ({ page, server }) => {
    const form = await openRegister(page, server);
    const username = form.getByLabel('Username', { exact: true });
    const email = form.getByLabel('E-mail', { exact: true });
    const password = form.getByLabel('Password', { exact: true });
    const confirm = form.getByLabel('Repeat the password', { exact: true });

    await username.press('Enter');
    await expect(form.getByRole('alert').filter({ hasText: 'Correct the marked fields.' })).toBeVisible();
    await expect(username).toHaveAccessibleDescription(/Enter a username/);
    await expect(email).toHaveAccessibleDescription(/Enter an e-mail address/);

    await username.fill('ab');
    await expect(username).toHaveAccessibleDescription(/Use at least 3 characters/);
    await username.fill('a b!');
    await expect(username).toHaveAccessibleDescription(/Use only letters, digits, _ and -/);
    await email.fill('not-an-address');
    await expect(email).toHaveAccessibleDescription(/Enter a valid e-mail address/);
    await password.fill('short');
    await expect(password).toHaveAccessibleDescription(/Use at least 8 characters/);
    await password.fill('long-enough-1');
    await confirm.fill('long-enough-2');
    await expect(confirm).toHaveAccessibleDescription(/The passwords do not match/);
  });

  test("shows the server's error as it is", async ({ page, server, account }) => {
    const form = await openRegister(page, server);
    await form.getByLabel('Username', { exact: true }).fill(account.username);
    await form.getByLabel('E-mail', { exact: true }).fill(`${unique('new')}@e2e.invalid`);
    await form.getByLabel('Password', { exact: true }).fill('long-enough-1');
    await form.getByLabel('Repeat the password', { exact: true }).fill('long-enough-1');
    await form.getByLabel('Repeat the password', { exact: true }).press('Enter');
    await expect(
      form.getByRole('alert').filter({ hasText: 'username already exists' }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });

  test('registers and signs in automatically', async ({ page, server }) => {
    const form = await openRegister(page, server);
    const username = unique('reg');
    await form.getByLabel('Username', { exact: true }).fill(username);
    await form.getByLabel('E-mail', { exact: true }).fill(`${username}@e2e.invalid`);
    await form.getByLabel('Password', { exact: true }).fill('long-enough-1');
    await form.getByLabel('Repeat the password', { exact: true }).fill('long-enough-1');
    await form.getByLabel('Repeat the password', { exact: true }).press('Enter');
    await expect(page.getByRole('button', { name: `Account: ${username}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/`));
  });
});
