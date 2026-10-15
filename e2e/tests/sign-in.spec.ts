// Signing in to a private server with a username or an e-mail, wrong passwords, and where a
// signed-in session lands.

import type { Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, test, type Account, type Server } from './fixtures';

/** Opens the sign-in form of the saved server. */
async function openSignIn(page: Page, server: Server) {
  await addServer(page, server);
  await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
  return page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
}

/** Signs in through the form with `login` and `password`. */
async function submit(page: Page, server: Server, login: string, password: string) {
  const form = await openSignIn(page, server);
  await form.getByLabel('Username or e-mail').fill(login);
  await form.getByLabel('Password', { exact: true }).fill(password);
  await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
  return form;
}

/** The session is up: the account menu names the user and the status bar is connected. */
async function expectSignedIn(page: Page, server: Server, account: Account) {
  await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/`));
}

test.describe('sign-in', () => {
  test('signs in with the username and lands on the world', async ({ page, server, account }) => {
    await submit(page, server, account.username, account.password);
    await expectSignedIn(page, server, account);
  });

  test('signs in with the e-mail address', async ({ page, server, account }) => {
    await submit(page, server, account.email, account.password);
    await expectSignedIn(page, server, account);
  });

  test('rejects a wrong password and keeps the form', async ({ page, server, account }) => {
    const form = await submit(page, server, account.username, `${account.password}-wrong`);
    await expect(form.getByRole('alert').filter({ hasText: /\S/ })).toHaveText(
      'Wrong name or password.',
    );
    await expect(form.getByLabel('Username or e-mail')).toHaveValue(account.username);
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });

  test('asks for the name and password before contacting the server', async ({ page, server }) => {
    const form = await openSignIn(page, server);
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expect(form.getByRole('alert').filter({ hasText: /\S/ })).toHaveText(
      'Enter your username or e-mail and your password',
    );
  });
});
