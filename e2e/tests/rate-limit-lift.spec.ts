// Lifted rate limits on screeps.com API-token sessions: while the token's limits are lifted the
// status bar and the Rate limits panel say until when, ten minutes before the end a toast
// offers to lift them again, and once they are not lifted an endpoint running low brings a
// prompt to lift them (screeps.com's reset page) or to leave it (`Not now`). Password sessions
// and private servers never see any of it.
//
// The local server is no screeps.com: the session is seeded as a screeps.com-like profile
// (`kind: 'official'`, signed in with an API token) and the test rewrites what screeps.com
// would answer (`page.route`): `GET /api/user/auth-token` lists the token (its first 8
// characters) with `noRatelimitUntil`, map-stats responses carry low `X-RateLimit-*` headers or
// a refusal (HTTP 429), and screeps.com's reset page is a stand-in that posts its success
// message to the app, as the real one does once its captcha is solved.
//
// The local server rotates the session token on every authenticated request. The rewritten
// token list therefore names a token the server just issued (one `GET /api/auth/me` with the
// request's headers) and hands it over through `X-Token`, so the app compares the list with the
// token it holds right then.

import type { Page, Request, Route } from '@playwright/test';
import { expect, Session, test } from './fixtures';
import type { Account, Server } from './fixtures';

/** The `X-RateLimit-*` headers of a window of 60 with `remaining` left, resetting at `reset`. */
function limitHeaders(remaining: number, reset: number): Record<string, string> {
  return {
    'x-ratelimit-limit': '60',
    'x-ratelimit-remaining': String(remaining),
    'x-ratelimit-reset': String(reset),
  };
}

/** Lets a cross-origin page (direct mode) read the response and the headers it needs. */
function corsHeaders(request: Request, exposed = ''): Record<string, string> {
  const origin = request.headers()['origin'];
  const expose = [exposed, 'X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset']
    .filter(Boolean)
    .join(', ');
  return origin
    ? { 'access-control-allow-origin': origin, 'access-control-expose-headers': expose }
    : {};
}

/**
 * Seeds `server` as a screeps.com-like server (`kind`) with `account` signed in, through an API
 * token (`apiToken`) or a password, and opens its world (like the fixtures' `signIn`).
 */
async function signInAs(
  page: Page,
  server: Server,
  account: Account,
  kind: 'official' | 'private',
  apiToken: boolean,
): Promise<void> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const me = await session.me();
  const profiles = {
    version: 2,
    servers: [
      {
        name: 'Local e2e',
        url: server.url,
        kind,
        login: account.username,
        user: { id: me._id, username: me.username, api_token: apiToken },
      },
    ],
  };
  await page.addInitScript(
    ({ profiles, url, token }) => {
      if (sessionStorage.getItem('overseer.e2e.seeded')) return;
      sessionStorage.setItem('overseer.e2e.seeded', '1');
      localStorage.setItem('overseer.profiles', JSON.stringify(profiles));
      localStorage.setItem(`overseer.secret.token:${url}`, token);
    },
    { profiles, url: server.url, token: session.token },
  );
  await page.goto(`/#/${server.slug}/`);
}

/** What the rewritten screeps.com answers; the test changes it as it goes. */
interface Fake {
  /** The token's `noRatelimitUntil` (milliseconds since the Unix epoch), `null` for none. */
  lift: number | null;
  /** Requests of the token list so far. */
  tokenLists: number;
  /** Map-stats requests so far. */
  mapStats: number;
}

/**
 * Rewrites the token list (`fake.lift`), map-stats responses (the server's answer with
 * `remaining` of 60 left, or a refusal when `remaining` is 0) and screeps.com's reset page.
 */
async function fakeScreeps(page: Page, fake: Fake, remaining: number, reset: number): Promise<void> {
  await page.route('**/api/user/auth-token', async (route: Route) => {
    const request = route.request();
    if (request.method() !== 'GET') return route.continue();
    fake.tokenLists += 1;
    // A token the server issues right now, which the app holds once this response arrives.
    const me = await route.fetch({
      url: request.url().replace(/api\/user\/auth-token.*$/, 'api/auth/me'),
      method: 'GET',
    });
    const token = me.headers()['x-token'] ?? request.headers()['x-token'] ?? '';
    const entry: Record<string, unknown> = {
      _id: 'k1',
      description: 'Overseer e2e',
      full: true,
      token: token.slice(0, 8),
    };
    if (fake.lift !== null) entry.noRatelimitUntil = fake.lift;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      headers: { ...corsHeaders(request), 'x-token': token },
      body: JSON.stringify({ ok: 1, tokens: [{ _id: 'k0', full: false, token: 'ffffffff' }, entry] }),
    });
  });
  await page.route('**/api/game/map-stats', async (route: Route) => {
    const request = route.request();
    if (request.method() !== 'POST') return route.continue();
    fake.mapStats += 1;
    if (remaining === 0) {
      return route.fulfill({
        status: 429,
        headers: { ...corsHeaders(request), ...limitHeaders(0, reset) },
        body: 'Too Many Requests',
      });
    }
    const response = await route.fetch();
    // The body comes decoded: drop what described the encoded one.
    const {
      'content-encoding': _encoding,
      'content-length': _length,
      ...headers
    } = response.headers();
    return route.fulfill({
      response,
      headers: {
        ...headers,
        ...corsHeaders(request, headers['access-control-expose-headers']),
        ...limitHeaders(remaining, reset),
      },
    });
  });
  // The reset page: success at once (the real one after its captcha), then it closes.
  await page.context().route('https://screeps.com/**', (route: Route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: '<!doctype html><title>Reset</title><script>window.opener.postMessage("screepsTokenSuccess", "*"); window.close();</script>',
    }),
  );
}

/** Opens the reset page with `open` and checks it got the token's first 8 characters only. */
async function resetPage(page: Page, open: () => Promise<void>): Promise<void> {
  const popup = page.waitForEvent('popup');
  await open();
  const reset = await popup;
  expect(reset.url()).toMatch(
    /^https:\/\/screeps\.com\/a\/#!\/account\/auth-tokens\/noratelimit\?token=[0-9a-f]{8}$/,
  );
}

test.describe('lifted rate limits', () => {
  test('the lift shows and ends, then a low endpoint prompts to lift it again', async ({
    page,
    server,
    account,
  }) => {
    const reset = Math.floor(Date.now() / 1000) + 30 * 60;
    const fake: Fake = { lift: Date.now() + 9 * 60_000, tokenLists: 0, mapStats: 0 };
    // 10 of 60 left: low (map stats keep 18 for near rooms), but lifted for now.
    await fakeScreeps(page, fake, 10, reset);
    await signInAs(page, server, account, 'official', true);

    const status = page.getByRole('contentinfo', { name: 'Status' });
    const lifted = status.getByTitle(/^Your API token's rate limits are lifted until /);
    await expect(lifted).toHaveText(/^Rate limits lifted · [89] min$/, { timeout: 30_000 });
    // Ten minutes or less left: the toast offers to lift them again.
    const toast = page.getByRole('status').filter({ hasText: 'Your rate limits return in' });
    await expect(toast).toContainText(/Your rate limits return in [89] min\./);
    // Map stats ran low meanwhile, and no prompt came: the limits are lifted.
    await expect.poll(() => fake.mapStats).toBeGreaterThan(0);
    const prompt = page.getByRole('dialog', { name: 'Rate limit running low' });
    await expect(prompt).toHaveCount(0);

    // The panel says until when, and offers no reset.
    await page.getByRole('button', { name: `Account: ${account.username}` }).click();
    await page.getByRole('menuitem', { name: 'Rate limits' }).click();
    const panel = page.getByRole('dialog', { name: 'Rate limits' });
    await expect(panel).toContainText(
      /Your API token's rate limits are lifted until .+ \([89] min left\)/,
    );
    await expect(panel.getByRole('button', { name: 'Reset on screeps.com' })).toHaveCount(0);
    await expect(status.getByRole('button', { name: 'Reset limit' })).toHaveCount(0);
    await panel.getByRole('button', { name: 'Close rate limits' }).click();

    // `Lift again`: the reset page succeeds and the app checks again (a short lift this time).
    fake.lift = Date.now() + 30_000;
    const lists = fake.tokenLists;
    await resetPage(page, () => toast.getByRole('button', { name: 'Lift again' }).click());
    await expect.poll(() => fake.tokenLists).toBeGreaterThan(lists);
    await expect(lifted).toHaveText(/^Rate limits lifted · \d+ s$/);

    // It ends: the app checks once more, and the low map stats prompt to lift the limits,
    // without taking the focus.
    await expect(prompt).toBeVisible({ timeout: 60_000 });
    await expect(lifted).toHaveCount(0);
    // The ending toast went with the lift.
    await expect(toast).toHaveCount(0);
    await expect(prompt.getByText(/^Only 10 of 60 map stats requests are left until the limit resets in \d+ min\.$/)).toBeVisible();
    await expect(prompt.locator(':focus')).toHaveCount(0);

    // `Lift limits for 2 hours`: the reset page, and lifted for two hours.
    fake.lift = Date.now() + 2 * 3600_000;
    await resetPage(page, () => prompt.getByRole('button', { name: 'Lift limits for 2 hours' }).click());
    await expect(prompt).toHaveCount(0);
    await expect(lifted).toHaveText(/^Rate limits lifted · (2 h|1 h 59 min)$/, { timeout: 30_000 });
  });

  test('a refusal prompts once, and Not now leaves it for the session', async ({
    page,
    server,
    account,
  }) => {
    const reset = Math.floor(Date.now() / 1000) + 30 * 60;
    const fake: Fake = { lift: null, tokenLists: 0, mapStats: 0 };
    await fakeScreeps(page, fake, 0, reset);
    await signInAs(page, server, account, 'official', true);

    const prompt = page.getByRole('dialog', { name: 'Rate limit running low' });
    await expect(prompt).toBeVisible({ timeout: 30_000 });
    await expect(prompt).toContainText(
      /No map stats request is left until the limit resets in (29|30|31) min\./,
    );
    expect(fake.tokenLists).toBeGreaterThan(0);
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(status.getByRole('button', { name: 'Reset limit' })).toBeVisible();
    await expect(status.getByTitle(/rate limits are lifted/)).toHaveCount(0);

    await prompt.getByRole('button', { name: 'Not now' }).click();
    await expect(prompt).toHaveCount(0);
    // Another refusal of the same endpoint prompts nothing.
    await page.getByRole('button', { name: `Account: ${account.username}` }).click();
    await page.getByRole('menuitem', { name: 'Rate limits' }).click();
    const requests = fake.mapStats;
    await page.getByRole('dialog', { name: 'Rate limits' }).getByRole('button', { name: 'Check again' }).click();
    await expect.poll(() => fake.mapStats).toBeGreaterThan(requests);
    await page.waitForTimeout(2_000);
    await expect(prompt).toHaveCount(0);
  });

  for (const [kind, apiToken, what] of [
    ['official', false, 'a password session'],
    ['private', true, 'a private server'],
  ] as const) {
    test(`${what} never checks or prompts`, async ({ page, server, account }) => {
      const reset = Math.floor(Date.now() / 1000) + 30 * 60;
      const fake: Fake = { lift: Date.now() + 3600_000, tokenLists: 0, mapStats: 0 };
      await fakeScreeps(page, fake, 0, reset);
      await signInAs(page, server, account, kind, apiToken);

      const status = page.getByRole('contentinfo', { name: 'Status' });
      await expect(status.getByTitle('Loading the world')).toContainText(
        'waiting for the rate limit',
        { timeout: 30_000 },
      );
      await page.waitForTimeout(2_000);
      await expect(page.getByRole('dialog', { name: 'Rate limit running low' })).toHaveCount(0);
      await expect(status.getByTitle(/rate limits are lifted/)).toHaveCount(0);
      await expect(status.getByRole('button', { name: 'Reset limit' })).toHaveCount(0);
      expect(fake.tokenLists).toBe(0);
    });
  }
});
