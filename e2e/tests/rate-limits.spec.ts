// Rate limits: the Rate limits panel (account menu) lists what the server reported; when the
// map-stats limit holds the world's overviews back, the status bar says so and when they
// resume; once the limit is lifted, the panel's "Check again" resumes them at once.
//
// The local server has no per-token limits, so the paused-load test rewrites the map-stats
// responses (`page.route`): first a refusal (HTTP 429) carrying `X-RateLimit-Remaining: 0` and
// a reset in half an hour, as screeps.com sends once a token's 60 requests per hour are used;
// then the server's own answers with healthy headers, as after a reset on screeps.com.

import type { Route } from '@playwright/test';
import { expect, signIn, test } from './fixtures';
import { accountMenu, corsHeaders } from './support/accounts';

/** The `X-RateLimit-*` headers of a window of 60 with `remaining` left, resetting at `reset`. */
function limitHeaders(remaining: number, reset: number): Record<string, string> {
  return {
    'x-ratelimit-limit': '60',
    'x-ratelimit-remaining': String(remaining),
    'x-ratelimit-reset': String(reset),
  };
}

test.describe('rate limits', () => {
  // screeps.com reports per-token limits on its endpoints; OpenScreeps has none.
  test('the panel lists the limits the server reported, or says there are none', async ({
    signedInPage: page,
    user,
    target,
  }) => {
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await accountMenu(page, user.username, 'Rate limits');
    const panel = page.getByRole('dialog', { name: 'Rate limits' });
    await expect(panel.getByRole('button', { name: 'Close rate limits' })).toBeFocused();
    if (target === 'official') {
      // The world's overviews ask for map stats at once.
      const row = panel.getByRole('row', { name: /^POST \/api\/game\/map-stats/ });
      await expect(row.getByRole('cell')).toHaveText([
        '60',
        /^\d+$/,
        /^(in \d+ (s|min)|in \d+ h( \d+ min)?|already reset)$/,
      ]);
      await expect(panel.getByRole('columnheader')).toHaveText([
        'Endpoint',
        'Limit',
        'Remaining',
        'Resets',
      ]);
    } else {
      await expect(panel).toContainText('No endpoint has reported a rate limit in this session yet.');
      await expect(panel.getByRole('table')).toHaveCount(0);
    }
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
  });
});

test.describe('rate limits', { tag: '@openscreeps' }, () => {
  // These tests hold the overview loads back: the world starts uncached.
  test.use({ cache: 'cold' });
  test('a paused overview load says so and resumes after Check again', async ({
    page,
    server,
    account,
  }) => {
    const reset = Math.floor(Date.now() / 1000) + 30 * 60;
    let healthy = false;
    let requests = 0;
    await page.route('**/api/game/map-stats', async (route: Route) => {
      const request = route.request();
      if (request.method() !== 'POST') return route.continue();
      requests += 1;
      if (!healthy) {
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
          ...limitHeaders(59, reset),
        },
      });
    });
    await signIn(page, server, account);

    const status = page.getByRole('contentinfo', { name: 'Status' });
    const loading = status.getByTitle('Loading the world');
    // Owners pause; the portal scan (no map stats) goes on beside them. The live room under
    // the map's centre may have its overview already: from the live room, not from map stats.
    await expect(loading).toContainText(
      /Owners [01] \/ [\d\u202f]+ · Portals [\d\u202f]+ \/ [\d\u202f]+ · waiting for the rate limit, resumes in (30|31) min$/,
      { timeout: 30_000 },
    );
    // Private servers have no reset page.
    await expect(status.getByRole('button', { name: 'Reset limit' })).toHaveCount(0);

    // The panel lists the limit the refusal reported.
    await page.getByRole('button', { name: `Account: ${account.username}` }).click();
    await page.getByRole('menuitem', { name: 'Rate limits' }).click();
    const panel = page.getByRole('dialog', { name: 'Rate limits' });
    await expect(panel).toBeVisible();
    const row = panel.getByRole('row', { name: /POST \/api\/game\/map-stats/ });
    await expect(row.getByRole('cell')).toHaveText(['60', '0', /^in (30|31) min$/]);
    await expect(panel).toContainText('loading resumes in');
    await expect(panel.getByRole('button', { name: 'Reset on screeps.com' })).toHaveCount(0);
    // Nothing more was asked while paused.
    expect(requests).toBe(1);

    // The limit is lifted: one request goes out at once and the overviews complete.
    healthy = true;
    await panel.getByRole('button', { name: 'Check again' }).click();
    await expect(loading).toHaveCount(0, { timeout: 30_000 });
    expect(requests).toBe(2);
    await expect(row.getByRole('cell')).toHaveText(['60', '59', /^in (30|31) min$/]);
    await expect(panel.getByRole('button', { name: 'Check again' })).toHaveCount(0);
    await panel.getByRole('button', { name: 'Close rate limits' }).click();
    await expect(panel).toHaveCount(0);
  });
});
