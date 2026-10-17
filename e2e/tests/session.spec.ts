// A signed-in session: resuming after a reload, keeping the rotated token, signing out, which
// origin the app talks to in each serving mode, and how a session ends without the user: a
// stored token the server no longer takes, a session revoked while open, and a server that
// cannot be reached.

import type { Request } from '@playwright/test';
import { expect, SERVER_NAME, signIn, test } from './fixtures';
import {
  accountMenu,
  alerts,
  revokeSession,
  seed,
  sessionOf,
  storedToken,
  switchServer,
} from './support/accounts';

declare global {
  interface Window {
    /** How often the app saved its profiles (`overseer.profiles`) since the page loaded. */
    e2eProfileSaves?: number;
  }
}

test.describe('session', { tag: '@transport' }, () => {
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

  test("keeps the account menu open while the session's profile is saved again", {
    tag: '@openscreeps',
  }, async ({ page, account, server }) => {
    await page.addInitScript(() => {
      const setItem = Storage.prototype.setItem;
      window.e2eProfileSaves = 0;
      Storage.prototype.setItem = function (key: string, value: string) {
        if (key === 'overseer.profiles') window.e2eProfileSaves = (window.e2eProfileSaves ?? 0) + 1;
        setItem.call(this, key, value);
      };
    });
    await signIn(page, server, account);
    const world = page.getByRole('main').getByRole('group', { name: 'Zoom' });
    await expect(world).toBeVisible();
    // To screeps.com's sign-in form; the session stays live and its menu with it.
    await switchServer(page, SERVER_NAME, /^screeps\.com/);
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toBeVisible();

    // With the menu open, back (no pointer, no key): the app shows the live session again
    // and saves its profile once more (same account). The menu is still there and open.
    const button = page.getByRole('button', { name: `Account: ${account.username}` });
    await button.click();
    const signOut = page.getByRole('menuitem', { name: 'Sign out' });
    await expect(signOut).toBeVisible();
    const saves = await page.evaluate(() => window.e2eProfileSaves ?? 0);
    await page.goBack();
    await expect(world).toBeVisible();
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

  test('signing out while the account reloads keeps the sign-in form', {
    tag: '@openscreeps',
  }, async ({ page, server, account }) => {
    let hold = false;
    const { promise: held, resolve: release } = Promise.withResolvers<void>();
    const late: Request[] = [];
    await page.route('**/api/auth/me', async (route) => {
      if (hold && route.request().method() === 'GET') {
        late.push(route.request());
        await held;
      }
      await route.fallback();
    });
    await signIn(page, server, account);
    await expect(page.getByRole('main').getByRole('group', { name: 'Zoom' })).toBeVisible();

    // Saving the notification preferences makes the app read the account again: held.
    hold = true;
    await accountMenu(page, account.username, 'Profile');
    const notifications = page.getByRole('region', { name: 'Notifications' });
    await notifications.getByRole('checkbox', { name: 'Notify about new messages' }).uncheck();
    await notifications.getByRole('button', { name: 'Save notifications' }).click();
    await expect(notifications.getByRole('status')).toHaveText('Notification preferences saved.');
    await expect.poll(() => late.length).toBeGreaterThan(0);

    // Away and back: the live session's screen waits for its account.
    await switchServer(page, SERVER_NAME, /^screeps\.com/);
    await expect(page.getByRole('region', { name: 'Sign in to screeps.com' })).toBeVisible();
    await switchServer(page, 'screeps.com', new RegExp(`^${SERVER_NAME}`));
    await expect(page.getByText(`Connecting to ${SERVER_NAME}…`)).toBeVisible();
    await accountMenu(page, account.username, 'Sign out');
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(form).toBeVisible();

    // The account arrives after all (or its request ends with the session): the form stays.
    release();
    await Promise.all(late.map((request) => request.response()));
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

  test('a session revoked while open ends with the sign-in form', {
    tag: '@openscreeps',
  }, async ({ page, server, account, target }) => {
    await signIn(page, server, account);
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(status).toContainText('Connected');
    await revokeSession(page, server, account);
    // OpenScreeps closes the account's sockets at once, and the reconnecting socket's token is
    // refused. A community server keeps the socket open: the app finds out with its next refresh
    // of the token (`auth/me`), at most 25 seconds after its latest token.
    const form = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
    await expect(alerts(form)).toHaveText('Your session has expired. Sign in again.', {
      timeout: target === 'community' ? 40_000 : undefined,
    });
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
