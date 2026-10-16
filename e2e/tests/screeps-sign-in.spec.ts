// Signing in to screeps.com (and Season): an API token (the default) or an e-mail and a
// password, the checks made before contacting the server, a rejected token, and the links to
// screeps.com's own pages (token page, registration, password recovery).
//
// The form of the built-in screeps.com entry needs no server until it is sent, so those tests
// run on every target. Signing in through it on the disposable server saves that server as a
// screeps.com-like profile (`kind: 'official'`), which shows the same form; its session tokens
// serve as API tokens (both go in `X-Token`).

import type { Page } from '@playwright/test';
import { expect, test, SERVER_NAME, type Server } from './fixtures';
import { accountMenu, alerts, seed, sessionOf } from './support/accounts';

/** Opens the sign-in form of the built-in screeps.com entry. */
async function openScreepsSignIn(page: Page) {
  await page.goto('/#/');
  await page.getByRole('button', { name: 'Sign in screeps.com' }).click();
  await expect(page).toHaveURL(/#\/screeps\.com\/$/);
  return page.getByRole('region', { name: 'Sign in to screeps.com' });
}

/** Saves the disposable server as a signed-out screeps.com-like server and opens its form. */
async function openLocalScreepsSignIn(page: Page, server: Server, login?: string) {
  await seed(page, [{ name: SERVER_NAME, url: server.url, kind: 'official', login }]);
  await page.goto(`/#/${server.slug}/`);
  return page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
}

test.describe('screeps.com sign-in', () => {
  test('offers an API token or an e-mail and password, and checks they are filled', async ({
    page,
  }) => {
    const form = await openScreepsSignIn(page);
    const requests: string[] = [];
    page.on('request', (request) => requests.push(new URL(request.url()).pathname));
    await expect(form).toContainText('https://screeps.com/');
    const method = form.getByRole('group', { name: 'Sign-in method' });
    const tokenTab = method.getByRole('button', { name: 'API token' });
    const passwordTab = method.getByRole('button', { name: 'E-mail and password' });
    const submit = form.getByRole('button', { name: 'Sign in', exact: true });
    // No in-app registration on screeps.com.
    await expect(form.getByRole('group', { name: 'Account' })).toHaveCount(0);

    await expect(tokenTab).toHaveAttribute('aria-pressed', 'true');
    await expect(form.getByLabel('API token')).toHaveAccessibleDescription(
      "Create a token with full access on your screeps.com account page. It is kept in this browser's storage.",
    );
    await submit.click();
    await expect(alerts(form)).toHaveText('Enter your API token');
    await form.getByLabel('API token').fill('   ');
    await submit.click();
    await expect(alerts(form)).toHaveText('Enter your API token');

    await passwordTab.click();
    await expect(passwordTab).toHaveAttribute('aria-pressed', 'true');
    await expect(tokenTab).toHaveAttribute('aria-pressed', 'false');
    await expect(form.getByLabel('API token')).toHaveCount(0);
    await form.getByLabel('E-mail').fill('someone@e2e.invalid');
    await submit.click();
    await expect(alerts(form)).toHaveText('Enter your e-mail and password');
    await expect(form.getByLabel('Password', { exact: true })).toBeVisible();
    expect(requests.filter((path) => path.includes('/api/'))).toEqual([]);
  });

  test("opens screeps.com's pages for tokens, registration and password recovery", async ({
    page,
    context,
  }) => {
    // The pages are screeps.com's: stand-ins answer instead.
    await context.route('https://screeps.com/a/**', (route) =>
      route.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html><title>screeps.com</title>' }),
    );
    const form = await openScreepsSignIn(page);
    for (const [link, url] of [
      ['Open the API token page', 'https://screeps.com/a/#!/account/auth-tokens'],
      ['Register on screeps.com', 'https://screeps.com/a/#!/register'],
      ['Forgot your password?', 'https://screeps.com/a/#!/register/ask-recover'],
    ]) {
      const opened = context.waitForEvent('page');
      await form.getByRole('button', { name: link }).click();
      const external = await opened;
      await expect(external).toHaveURL(url);
      await external.close();
    }
    // The app stays where it was.
    await expect(form).toBeVisible();
  });

  test('signs in with an API token', { tag: '@openscreeps' }, async ({ page, server, account }) => {
    const { token } = await sessionOf(server, account);
    const form = await openLocalScreepsSignIn(page, server);
    await form.getByLabel('API token').fill(token);
    await form.getByRole('button', { name: 'Sign in', exact: true }).click();
    const accountButton = page.getByRole('button', { name: `Account: ${account.username}` });
    await expect(accountButton).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/`));
    // Kept: a reload resumes it.
    await page.reload();
    await expect(accountButton).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  });

  test('rejects a token the server does not know', { tag: '@openscreeps' }, async ({
    page,
    server,
  }) => {
    const form = await openLocalScreepsSignIn(page, server);
    await form.getByLabel('API token').fill('0123456789abcdef0123456789abcdef');
    await form.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(alerts(form)).toHaveText('The token was rejected.');
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });

  test('signs in with an e-mail and password, and fills the e-mail next time', {
    tag: '@openscreeps',
  }, async ({ page, server, account }) => {
    const form = await openLocalScreepsSignIn(page, server);
    await form.getByRole('button', { name: 'E-mail and password' }).click();
    await form.getByLabel('E-mail').fill(account.email);
    await form.getByLabel('Password', { exact: true }).fill(`${account.password}-wrong`);
    await form.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(alerts(form)).toHaveText('Wrong name or password.');

    await form.getByLabel('Password', { exact: true }).fill(account.password);
    await form.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');

    await accountMenu(page, account.username, 'Sign out');
    await expect(form.getByRole('button', { name: 'API token' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await form.getByRole('button', { name: 'E-mail and password' }).click();
    await expect(form.getByLabel('E-mail')).toHaveValue(account.email);
  });
});
