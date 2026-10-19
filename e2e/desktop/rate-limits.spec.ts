// Lifting screeps.com's rate limits in the desktop app: the reset page opens in the system
// browser, which cannot tell the app it succeeded (the web build's popup posts a message to its
// opener), so the app checks the lift again once its window gets the focus back.
//
// As in the web tests (`../tests/rate-limit-lift.spec.ts`), the local server stands in for
// screeps.com: the session is a screeps.com-like profile (`kind: 'official'`, signed in with an
// API token: seeded in the app's storage and, through the shell, in the keychain), and the test
// rewrites the token list (`GET /api/user/auth-token`) and refuses map stats (HTTP 429). The app
// reaches the server through the shell's proxy, cross-origin: rewritten answers carry the CORS
// headers the proxy would. The user coming back to the window is its `focus` event.
//
// Only an OpenScreeps server can stand in for screeps.com: the stock community server does not
// decode as screeps.com (its `GET /api/version` features have no `version`). Thus the
// `community` run skips this test. The web tests cover the community lift by request.

import type { Request, Route } from '@playwright/test';
import { expect, interceptOpener, open, Session, statusBar, test } from './fixtures';

/** Lets the app's page (cross-origin to the proxy) read a rewritten answer. */
function corsHeaders(request: Request): Record<string, string> {
  return {
    'access-control-allow-origin': request.headers()['origin'] ?? '',
    'access-control-expose-headers':
      'X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset',
  };
}

test('lifting the limits opens the reset page in the system browser, then checks on focus', {
  tag: '@openscreeps',
}, async ({ app, server, credentials, target }) => {
  test.skip(target === 'community', "the stock community server lacks screeps.com's contract");
  if (credentials.kind !== 'password') throw new Error('needs a password account');
  const session = await Session.signIn(server.url, credentials.login, credentials.password);
  const me = await session.me();
  const reset = Math.floor(Date.now() / 1000) + 30 * 60;
  const fake = { lift: null as number | null, tokenLists: 0 };
  app.hook({
    context: async (context) => {
      await context.route('**/api/user/auth-token', async (route: Route) => {
        const request = route.request();
        if (request.method() !== 'GET') return route.fallback();
        fake.tokenLists += 1;
        // A token the server issues right now, which the app holds once this answer arrives.
        const answer = await route.fetch({
          url: request.url().replace(/api\/user\/auth-token.*$/, 'api/auth/me'),
        });
        const token = answer.headers()['x-token'] ?? request.headers()['x-token'] ?? '';
        const entry: Record<string, unknown> = { _id: 'k1', full: true, token: token.slice(0, 8) };
        if (fake.lift !== null) entry.noRatelimitUntil = fake.lift;
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          headers: { ...corsHeaders(request), 'x-token': token },
          body: JSON.stringify({ ok: 1, tokens: [entry] }),
        });
      });
      await context.route('**/api/game/map-stats', (route: Route) => {
        const request = route.request();
        if (request.method() !== 'POST') return route.fallback();
        return route.fulfill({
          status: 429,
          headers: {
            ...corsHeaders(request),
            'x-ratelimit-limit': '60',
            'x-ratelimit-remaining': '0',
            'x-ratelimit-reset': String(reset),
          },
          body: 'Too Many Requests',
        });
      });
    },
  });

  const page = await app.launch();
  const profiles = {
    version: 2,
    servers: [
      {
        name: 'Local e2e',
        url: server.url,
        kind: 'official',
        login: credentials.login,
        user: { id: me._id, username: me.username, api_token: true },
      },
    ],
  };
  // The app moves a list in `localStorage` into its profiles file when it loads the page again
  // (`saved-servers.spec.ts`).
  await page.evaluate(
    async ({ profiles, key, token }) => {
      localStorage.setItem('overseer.profiles', JSON.stringify(profiles));
      await window.__TAURI__!.core.invoke('secret_set', { key, value: token });
    },
    { profiles, key: `token:${server.url}`, token: session.token },
  );
  await open(page, `#/${server.slug}/`);

  const prompt = page.getByRole('dialog', { name: 'Rate limit running low' });
  await expect(prompt).toBeVisible({ timeout: 30_000 });
  await expect(prompt).toContainText(/No map stats request is left until the limit resets/);
  const opened = await interceptOpener(page);
  fake.lift = Date.now() + 2 * 3600_000;
  await prompt.getByRole('button', { name: 'Lift limits for 2 hours' }).click();
  await expect
    .poll(opened)
    .toEqual([
      expect.stringMatching(
        /^https:\/\/screeps\.com\/a\/#!\/account\/auth-tokens\/noratelimit\?token=[0-9a-f]{8}$/,
      ),
    ]);
  // No window of the app's own: the page is in the system browser.
  expect(app.context.pages()).toHaveLength(1);

  // The user comes back from the browser: the app checks, and the lift shows.
  const lists = fake.tokenLists;
  await page.evaluate(() => window.dispatchEvent(new FocusEvent('focus')));
  await expect.poll(() => fake.tokenLists).toBeGreaterThan(lists);
  await expect(
    statusBar(page).getByTitle(/^Your API token's rate limits are lifted until /),
  ).toHaveText(/^Rate limits lifted · (2 h|1 h 59 min)$/, { timeout: 30_000 });
  await expect(prompt).toHaveCount(0);
});
