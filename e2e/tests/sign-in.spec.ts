// Signing in to a private server with a username or an e-mail, wrong passwords, the name kept
// for the next sign-in, where a signed-in session lands, and password-protected servers (the
// server password asked for, stored, and sent again on resume).
//
// The disposable server has no server password; the tests that need one make every `api/*`
// request without it fail as such a server answers (`protectWithServerPassword`).

import type { Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, signIn, test, type Account, type Server } from './fixtures';
import { accountMenu, alerts, protectWithServerPassword } from './support/accounts';

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

test.describe('sign-in', { tag: '@openscreeps' }, () => {
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
    await expect(alerts(form)).toHaveText('Wrong name or password.');
    await expect(form.getByLabel('Username or e-mail')).toHaveValue(account.username);
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });

  test('asks for the name and password before contacting the server', async ({ page, server }) => {
    const form = await openSignIn(page, server);
    // The form shows once the server is discovered: nothing goes out after that.
    await expect(form.getByLabel('Username or e-mail')).toBeVisible();
    const requests: string[] = [];
    page.on('request', (request) => requests.push(new URL(request.url()).pathname));
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expect(alerts(form)).toHaveText(
      'Enter your username or e-mail and your password',
    );
    await form.getByLabel('Username or e-mail').fill('someone');
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expect(alerts(form)).toHaveText(
      'Enter your username or e-mail and your password',
    );
    expect(requests.filter((path) => path.includes('/api/'))).toEqual([]);
  });

  test('fills the name of the last sign-in after signing out', async ({ page, server, account }) => {
    await submit(page, server, account.email, account.password);
    await expectSignedIn(page, server, account);
    await accountMenu(page, account.username, 'Sign out');
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(form.getByLabel('Username or e-mail')).toHaveValue(account.email);
    await expect(form.getByLabel('Password', { exact: true })).toHaveValue('');
  });
});

test.describe('server password', { tag: '@openscreeps' }, () => {
  test('is asked for when the server wants one, then kept for the next visits', async ({
    page,
    server,
    account,
  }) => {
    const sent = await protectWithServerPassword(page, 'open-sesame');
    const form = await openSignIn(page, server);
    // Asked before the form: the server cannot be discovered without it.
    const serverPassword = form.getByLabel('Server password', { exact: true });
    await expect(serverPassword).toHaveAccessibleDescription(
      "This server is password-protected; ask its operator for the password.",
    );
    await expect(form.getByLabel('Username or e-mail')).toHaveCount(0);
    const continueButton = form.getByRole('button', { name: 'Continue', exact: true });
    await continueButton.click();
    await expect(alerts(form)).toHaveText('Enter the server password');

    await serverPassword.fill('wrong');
    await continueButton.click();
    await expect.poll(() => sent.includes('wrong')).toBe(true);
    await expect(alerts(form)).toHaveText('incorrect server password');
    await expect(form.getByLabel('Username or e-mail')).toHaveCount(0);

    await serverPassword.fill('open-sesame');
    await continueButton.click();
    await form.getByLabel('Username or e-mail').fill(account.username);
    await expect(serverPassword).toHaveCount(0);
    await form.getByLabel('Password', { exact: true }).fill(account.password);
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expectSignedIn(page, server, account);

    // Stored: resuming after a reload sends it again.
    const before = sent.length;
    await page.reload();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    expect(sent.slice(before).length).toBeGreaterThan(0);
    expect(sent.slice(before).every((given) => given === 'open-sesame')).toBe(true);
  });

  test('given when adding the server, signing in needs nothing more', async ({
    page,
    server,
    account,
  }) => {
    const sent = await protectWithServerPassword(page, 'open-sesame');
    await page.goto('/#/');
    await page
      .getByRole('region', { name: 'Servers' })
      .getByRole('button', { name: 'Add server' })
      .click();
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    await dialog.getByLabel('Name', { exact: true }).fill(SERVER_NAME);
    await dialog.getByLabel('Address', { exact: true }).fill(server.url);
    const field = dialog.getByLabel('Server password (optional)');
    await expect(field).toHaveAccessibleDescription(
      "Only for password-protected servers; kept in this browser's storage.",
    );
    await field.fill('open-sesame');
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog).toBeHidden();

    await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await form.getByLabel('Username or e-mail').fill(account.username);
    await form.getByLabel('Password', { exact: true }).fill(account.password);
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expectSignedIn(page, server, account);
    expect(sent).not.toContain('');
    await expect(form.getByLabel('Server password', { exact: true })).toHaveCount(0);
  });

  test('a saved session without it asks for it on resume', async ({ page, server, account }) => {
    await protectWithServerPassword(page, 'open-sesame');
    await signIn(page, server, account);
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(alerts(form)).toHaveText(
      'This server needs its server password.',
    );
    await expect(form.getByLabel('Server password', { exact: true })).toBeVisible();
    await expect(form.getByRole('button', { name: 'Continue', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });
});
