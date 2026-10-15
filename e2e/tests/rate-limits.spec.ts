// Rate limits: when the map-stats limit holds the world's overviews back, the status bar says
// so and when they resume; once the limit is lifted, the Rate limits panel's "Check again"
// resumes them at once.
//
// The local server has no per-token limits, so the test rewrites the map-stats responses
// (`page.route`): first a refusal (HTTP 429) carrying `X-RateLimit-Remaining: 0` and a reset in
// half an hour, as screeps.com sends once a token's 60 requests per hour are used; then the
// server's own answers with healthy headers, as after a reset on screeps.com.

import type { Request, Route } from '@playwright/test';
import { expect, signIn, test } from './fixtures';

/** The `X-RateLimit-*` headers of a window of 60 with `remaining` left, resetting at `reset`. */
function limitHeaders(remaining: number, reset: number): Record<string, string> {
  return {
    'x-ratelimit-limit': '60',
    'x-ratelimit-remaining': String(remaining),
    'x-ratelimit-reset': String(reset),
  };
}

/** Lets a cross-origin page (direct mode) read the response and its rate-limit headers. */
function corsHeaders(request: Request, exposed = ''): Record<string, string> {
  const origin = request.headers()['origin'];
  const expose = [exposed, 'X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset']
    .filter(Boolean)
    .join(', ');
  return origin
    ? { 'access-control-allow-origin': origin, 'access-control-expose-headers': expose }
    : {};
}

test.describe('rate limits', () => {
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
    await expect(loading).toContainText(
      /Owners 0 \/ [\d\u202f]+ · waiting for the rate limit, resumes in (30|31) min$/,
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

  test('the panel says when no limit is known yet', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await page.getByRole('button', { name: `Account: ${account.username}` }).click();
    await page.getByRole('menuitem', { name: 'Rate limits' }).click();
    const panel = page.getByRole('dialog', { name: 'Rate limits' });
    await expect(panel).toContainText('No endpoint has reported a rate limit in this session yet.');
    await expect(panel.getByRole('button', { name: 'Check again' })).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
  });
});
