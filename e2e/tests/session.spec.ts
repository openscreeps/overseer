// A signed-in session: resuming after a reload, keeping the rotated token, signing out, which
// origin the app talks to in each serving mode, and how a session ends without the user: a
// stored token the server no longer takes, a session revoked while open, and a server that
// cannot be reached.

import type { Request } from '@playwright/test';
import { expect, SERVER_NAME, signIn, test } from './fixtures';
import { accountMenu, admin, alerts, seed, sessionOf, storedToken } from './support/accounts';

declare global {
  interface Window {
    /** How often the app saved its profiles (`overseer.profiles`) since the page loaded. */
    e2eProfileSaves?: number;
  }
}

test.describe('session', () => {
  test('resumes after a reload', async ({ signedInPage: page, user, server }) => {
    const account = page.getByRole('button', { name: `Account: ${user.username}` });
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(account).toBeVisible();
    await expect(status).toContainText('Connected');
    await page.reload();
    await expect(account).toBeVisible();
    await expect(status).toContainText('Connected');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
  });

  test('keeps the account menu open while the session resumes', { tag: '@openscreeps' }, async ({
    page,
    account,
    server,
  }) => {
    // Resuming asks for the account twice: on connect (the user id), then for the username,
    // which the app saves into its profiles. Hold the second reply until the menu is open.
    let calls = 0;
    let release = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route('**/api/auth/me', async (route) => {
      if (route.request().method() === 'GET' && ++calls === 2) await held;
      await route.fallback();
    });
    await page.addInitScript(() => {
      const setItem = Storage.prototype.setItem;
      window.e2eProfileSaves = 0;
      Storage.prototype.setItem = function (key: string, value: string) {
        if (key === 'overseer.profiles') window.e2eProfileSaves = (window.e2eProfileSaves ?? 0) + 1;
        setItem.call(this, key, value);
      };
    });
    await signIn(page, server, account);
    const button = page.getByRole('button', { name: `Account: ${account.username}` });
    await button.click();
    const signOut = page.getByRole('menuitem', { name: 'Sign out' });
    await expect(signOut).toBeVisible();
    await expect.poll(() => calls).toBe(2);
    const answered = page.waitForResponse(
      (response) => new URL(response.url()).pathname.endsWith('/api/auth/me'),
    );
    const saves = await page.evaluate(() => window.e2eProfileSaves ?? 0);
    release();
    await answered;
    // The app saves the profiles again (same account); the menu is still there and open.
    await expect
      .poll(() => page.evaluate(() => window.e2eProfileSaves ?? 0))
      .toBeGreaterThan(saves);
    await expect(button).toHaveAttribute('aria-expanded', 'true');
    await expect(signOut).toBeVisible();
  });

  // screeps.com's API tokens do not rotate; session tokens of password sign-ins do.
  test('keeps the rotated session token', { tag: '@openscreeps' }, async ({
    signedInPage: page,
    player,
    server,
  }) => {
    const key = `overseer.secret.token:${server.url}`;
    await expect(page.getByRole('button', { name: `Account: ${player.username}` })).toBeVisible();
    const seeded = await page.evaluate((key) => localStorage.getItem(key), key);
    // The server replaces the token with every authenticated reply; the app stores the latest.
    await expect
      .poll(() => page.evaluate((key) => localStorage.getItem(key), key))
      .not.toBe(seeded);
  });

  test('signs out', async ({ signedInPage: page, user, server }) => {
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await accountMenu(page, user.username, 'Sign out');
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
    await expect(page.getByRole('region', { name: `Sign in to ${server.name}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Not connected');
    expect(await storedToken(page, server.url)).toBeNull();
    // Signed out stays signed out after a reload.
    await page.goto('/#/');
    await page.reload();
    await expect(
      page.getByRole('region', { name: 'Servers' }).getByRole('listitem').filter({ hasText: server.url }),
    ).toContainText('Signed out');
    await expect(page.getByRole('button', { name: `Sign in ${server.name}` })).toBeVisible();
  });

  test('talks to the server directly or through the proxy that serves it', {
    tag: '@openscreeps',
  }, async ({ page, account, server, mode, baseURL }) => {
    const requests: string[] = [];
    page.on('request', (request) => requests.push(request.url()));
    await signIn(page, server, account);
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    const api = requests.filter((url) => new URL(url).pathname.startsWith('/api/'));
    expect(api.length).toBeGreaterThan(0);
    const origin = mode === 'proxy' ? new URL(baseURL ?? '').origin : new URL(server.url).origin;
    expect(api.every((url) => new URL(url).origin === origin), api.join('\n')).toBe(true);
    if (mode === 'proxy') {
      const discovery = await page.request.get('/overseer-proxy.json');
      expect(await discovery.json()).toEqual({ upstream: server.url });
    }
  });

  test('signing out while the account still loads keeps the sign-in form', {
    tag: '@openscreeps',
  }, async ({ page, server, account }) => {
    // A session loads its account once it has begun (the second `GET /api/auth/me`, see
    // above): hold that reply until the user has signed out.
    let calls = 0;
    const { promise: held, resolve: release } = Promise.withResolvers<void>();
    let late: Request | undefined;
    await page.route('**/api/auth/me', async (route) => {
      if (route.request().method() === 'GET' && ++calls === 2) {
        late = route.request();
        await held;
      }
      await route.fallback();
    });
    await signIn(page, server, account);
    await expect.poll(() => calls).toBe(2);
    await accountMenu(page, account.username, 'Sign out');
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(form).toBeVisible();

    // The reply comes after all (or the request ends with the session): the form stays.
    release();
    await late?.response();
    await page.evaluate(() => {
      const { promise, resolve } = Promise.withResolvers<void>();
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
      return promise;
    });
    await expect(form).toBeVisible();
    await expect(page.getByText(`Connecting to ${SERVER_NAME}…`)).toHaveCount(0);
  });

  test('a stored token the server rejects asks to sign in again, name filled', {
    tag: '@openscreeps',
  }, async ({ page, server, account }) => {
    const { user } = await sessionOf(server, account);
    await seed(
      page,
      [{ name: SERVER_NAME, url: server.url, kind: 'private', login: account.username, user }],
      { [server.url]: 'not-a-token-the-server-issued' },
    );
    await page.goto(`/#/${server.slug}/`);
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(alerts(form)).toHaveText('Your session has expired. Sign in again.');
    await expect(form.getByLabel('Username or e-mail')).toHaveValue(account.username);
    // The rejected token is gone and the server shows signed out.
    expect(await storedToken(page, server.url)).toBeNull();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);

    await form.getByLabel('Password', { exact: true }).fill(account.password);
    await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  });

  test('a session revoked while open ends with the sign-in form', { tag: '@openscreeps' }, async ({
    page,
    server,
    account,
  }) => {
    await signIn(page, server, account);
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(status).toContainText('Connected');
    // A new password revokes every session of the account at once.
    await admin(server, { action: 'user-update', username: account.username, password: 'revoked-1234' });
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(alerts(form)).toHaveText('Your session has expired. Sign in again.');
    await expect(form.getByLabel('Username or e-mail')).toHaveValue(account.username);
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
    await expect(status).toContainText('Not connected');
    expect(await storedToken(page, server.url)).toBeNull();
  });

  test('an unreachable server keeps the session and offers to try again', {
    tag: '@openscreeps',
  }, async ({ page, server, account }) => {
    let down = true;
    await page.route(
      (url) => url.pathname.includes('/api/'),
      (route) => (down ? route.abort('connectionrefused') : route.fallback()),
    );
    await signIn(page, server, account);
    const failed = page.getByRole('alert').filter({ hasText: 'Cannot connect' });
    await expect(failed.getByRole('heading', { name: 'Cannot connect' })).toBeVisible();
    await expect(failed).toContainText(`Could not connect to ${SERVER_NAME}: `);
    // Not a sign-out: the token stays for the next attempt.
    expect(await storedToken(page, server.url)).not.toBeNull();

    down = false;
    await failed.getByRole('button', { name: 'Try again' }).click();
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  });
});
