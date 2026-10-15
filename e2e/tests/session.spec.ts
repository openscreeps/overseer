// A signed-in session: resuming after a reload, keeping the rotated token, signing out, and
// which origin the app talks to in each serving mode.

import { expect, SERVER_NAME, signIn, test } from './fixtures';

test.describe('session', () => {
  test('resumes after a reload', async ({ signedInPage: page, player, server }) => {
    const account = page.getByRole('button', { name: `Account: ${player.username}` });
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(account).toBeVisible();
    await expect(status).toContainText('Connected');
    await page.reload();
    await expect(account).toBeVisible();
    await expect(status).toContainText('Connected');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/`));
  });

  test('keeps the account menu open while the session resumes', async ({
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
    await signIn(page, server, account);
    const button = page.getByRole('button', { name: `Account: ${account.username}` });
    await button.click();
    const signOut = page.getByRole('menuitem', { name: 'Sign out' });
    await expect(signOut).toBeVisible();
    await expect.poll(() => calls).toBe(2);
    const answered = page.waitForResponse(
      (response) => new URL(response.url()).pathname.endsWith('/api/auth/me'),
    );
    release();
    await answered;
    // The app saved the profiles again (same account); the menu is still there and open.
    await page.waitForTimeout(500);
    await expect(button).toHaveAttribute('aria-expanded', 'true');
    await expect(signOut).toBeVisible();
  });

  test('keeps the rotated session token', async ({ signedInPage: page, player, server }) => {
    const key = `overseer.secret.token:${server.url}`;
    await expect(page.getByRole('button', { name: `Account: ${player.username}` })).toBeVisible();
    const seeded = await page.evaluate((key) => localStorage.getItem(key), key);
    // The server replaces the token with every authenticated reply; the app stores the latest.
    await expect
      .poll(() => page.evaluate((key) => localStorage.getItem(key), key))
      .not.toBe(seeded);
  });

  test('signs out', async ({ signedInPage: page, player, server }) => {
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await page.getByRole('button', { name: `Account: ${player.username}` }).click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Not connected');
    expect(
      await page.evaluate((key) => localStorage.getItem(key), `overseer.secret.token:${server.url}`),
    ).toBeNull();
    // Signed out stays signed out after a reload.
    await page.goto('/#/');
    await page.reload();
    await expect(
      page.getByRole('region', { name: 'Servers' }).getByRole('listitem').filter({ hasText: server.url }),
    ).toContainText('Signed out');
    await expect(page.getByRole('button', { name: `Sign in ${SERVER_NAME}` })).toBeVisible();
  });

  test('talks to the server directly or through the proxy that serves it', async ({
    page,
    account,
    server,
    mode,
    baseURL,
  }) => {
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
});
