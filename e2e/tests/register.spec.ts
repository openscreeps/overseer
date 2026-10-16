// Registration on a private server: switching between the sign-in and register forms, live
// availability checks, field validation, the server's own error strings shown as they are, the
// automatic sign-in after success, and choosing the username of an account that has none.

import type { Locator, Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, signIn, test, unique, type Server } from './fixtures';
import { alerts, corsHeaders, fulfillJson } from './support/accounts';

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

test.describe('registration tabs', () => {
  test('switches between signing in and registering', async ({ page }) => {
    // Saved only: the forms need no server until something is typed or sent.
    await page.goto('/#/');
    await page
      .getByRole('region', { name: 'Servers' })
      .getByRole('button', { name: 'Add server' })
      .click();
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    await dialog.getByLabel('Name', { exact: true }).fill('Fictional');
    await dialog.getByLabel('Address', { exact: true }).fill('http://127.0.0.1:9/');
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await page.getByRole('button', { name: 'Sign in Fictional' }).click();

    const tabs = page.getByRole('group', { name: 'Account' });
    await expect(tabs.getByRole('button', { name: 'Sign in' })).toHaveAttribute('aria-pressed', 'true');
    await tabs.getByRole('button', { name: 'Register' }).click();
    const register = page.getByRole('region', { name: 'Register on Fictional' });
    await expect(register).toContainText('http://127.0.0.1:9/');
    await expect(tabs.getByRole('button', { name: 'Register' })).toHaveAttribute('aria-pressed', 'true');
    await expect(register.getByLabel('Username', { exact: true })).toHaveAccessibleDescription(
      'Letters, digits, _ and -; at least 3 characters.',
    );
    await expect(register.getByLabel('Password', { exact: true })).toHaveAccessibleDescription(
      '8 to 1024 characters.',
    );

    await tabs.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByRole('region', { name: 'Sign in to Fictional' })).toBeVisible();
    await tabs.getByRole('button', { name: 'Register' }).click();
    await register.getByRole('button', { name: 'Choose another server' }).click();
    await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
  });
});

// Each field's accessible description holds its error, its live check status and its help.
test.describe('registration', { tag: '@openscreeps' }, () => {
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

// Accounts made through Steam or GitHub have no username until they choose one. The disposable
// server only makes accounts with a username, so the test hides it from `GET /api/auth/me`
// until the app sets one, and answers `POST /api/register/set-username` itself.
test.describe('choosing a username', { tag: '@openscreeps' }, () => {
  test('asks an account without a username for one, then opens the world', async ({
    page,
    server,
    account,
  }) => {
    let chosen: string | undefined;
    const requests: Record<string, unknown>[] = [];
    await page.route('**/api/auth/me', async (route) => {
      const request = route.request();
      if (request.method() !== 'GET') return route.fallback();
      const response = await route.fetch();
      const me = (await response.json()) as Record<string, unknown>;
      if (chosen) me.username = chosen;
      else delete me.username;
      // The body is rewritten: drop what described the original one.
      const {
        'content-encoding': _encoding,
        'content-length': _length,
        ...headers
      } = response.headers();
      return route.fulfill({
        response,
        headers: { ...headers, ...corsHeaders(request, headers['access-control-expose-headers']) },
        body: JSON.stringify(me),
      });
    });
    await page.route('**/api/register/set-username', async (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      const body = route.request().postDataJSON() as Record<string, unknown>;
      requests.push(body);
      chosen = String(body.username);
      return fulfillJson(route, { ok: 1 });
    });
    await signIn(page, server, account);

    const form = page.getByRole('region', { name: 'Choose your username' });
    await expect(form).toContainText(
      'Your account has no username yet. Choose one to start playing; it cannot be changed later.',
    );
    // Away to another server and back: the form again, not the world.
    await page.getByRole('button', { name: `Server: ${SERVER_NAME}` }).click();
    await page.getByRole('menuitemradio', { name: /^screeps\.com/ }).click();
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toBeVisible();
    await page.getByRole('button', { name: 'Server: screeps.com' }).click();
    await page.getByRole('menuitemradio', { name: new RegExp(`^${SERVER_NAME}`) }).click();
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toHaveCount(0);
    await expect(form).toBeVisible();

    const username = form.getByLabel('Username', { exact: true });
    const email = form.getByLabel('E-mail (optional)');
    const save = form.getByRole('button', { name: 'Save username' });

    await save.click();
    await expect(alerts(form).last()).toHaveText('Correct the marked fields.');
    await expect(username).toHaveAccessibleDescription(/Enter a username/);
    // Taken (checked with the server), malformed, and a bad optional address.
    await username.fill(account.username);
    await expect(username).toHaveAccessibleDescription(/Already taken/);
    await username.fill('no spaces');
    await expect(username).toHaveAccessibleDescription(/Use only letters, digits, _ and -/);
    await email.fill('not-an-address');
    await expect(email).toHaveAccessibleDescription(/Enter a valid e-mail address/);
    await save.click();
    await expect(alerts(form).last()).toHaveText('Correct the marked fields.');
    expect(requests).toEqual([]);

    const name = unique('named');
    await username.fill(name);
    await expect(username).toHaveAccessibleDescription(/Available/);
    await email.fill('');
    await save.click();
    await expect(page.getByRole('button', { name: `Account: ${name}` })).toBeVisible();
    await expect(form).toHaveCount(0);
    expect(requests).toEqual([{ username: name }]);
  });
});
