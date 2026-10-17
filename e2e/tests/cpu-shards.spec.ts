// CPU per shard on the profile (screeps.com only): one field per shard, filled from the
// profile's allocation (`cpuShard`), the total against the account's CPU, Save only when the
// total matches and 12 hours passed since the last change (`cpuShardUpdatedTime`), the request
// it sends (`POST /api/user/cpu-shards`), the refreshed allocation after a change, and the
// server's refusal inline.
//
// The local server is no screeps.com: the session is seeded as a screeps.com-like profile
// (`kind: 'official'`), which forces the screeps.com contract. Only an OpenScreeps server can
// stand in for screeps.com, because OpenScreeps adds its own profile to that contract. The test
// rewrites what screeps.com would answer (`page.route`): `GET /api/auth/me` carries the
// allocation and a CPU of 100, and `POST /api/user/cpu-shards` answers as screeps.com does.
// `profile.spec.ts` checks that the section stays away on servers without the allocation.

import type { Locator, Page } from '@playwright/test';
import { expect, test } from './fixtures';
import type { Account, Server } from './fixtures';
import { accountMenu, alerts, corsHeaders, seed, sessionOf } from './support/accounts';

/** One hour, in milliseconds. */
const HOUR = 60 * 60 * 1000;

/** What the rewritten screeps.com answers; the test changes it as it goes. */
interface Fake {
  /** The CPU of each shard (`cpuShard`). */
  cpuShard: Record<string, number>;
  /** The last change (`cpuShardUpdatedTime`, milliseconds since the Unix epoch). */
  updated: number;
  /** The bodies of the allocation requests so far. */
  sent: string[];
  /** The error the server answers to an allocation request; it accepts the request without. */
  refusal?: string;
}

/**
 * Seeds `account` signed in on `server` as a screeps.com-like profile, rewrites the answers of
 * screeps.com from `fake`, and opens the profile's CPU per shard section.
 */
async function openCpuShards(
  page: Page,
  server: Server,
  account: Account,
  fake: Fake,
): Promise<Locator> {
  const { user, token } = await sessionOf(server, account);
  await seed(
    page,
    [{ name: 'Local e2e', url: server.url, kind: 'official', login: account.username, user }],
    { [server.url]: token },
  );
  await page.route('**/api/auth/me', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const response = await route.fetch();
    const me: unknown = await response.json();
    const fields = me && typeof me === 'object' ? me : {};
    return route.fulfill({
      response,
      json: { ...fields, cpu: 100, cpuShard: fake.cpuShard, cpuShardUpdatedTime: fake.updated },
    });
  });
  await page.route('**/api/user/cpu-shards', async (route) => {
    const request = route.request();
    if (request.method() === 'OPTIONS') {
      return route.fulfill({
        status: 204,
        headers: {
          ...corsHeaders(request),
          'access-control-allow-headers': '*',
          'access-control-allow-methods': 'POST',
        },
      });
    }
    const body = request.postData() ?? '';
    fake.sent.push(body);
    let reply: Record<string, unknown> = { ok: 1 };
    if (fake.refusal) {
      reply = { error: fake.refusal };
    } else {
      const sent: unknown = JSON.parse(body);
      if (sent && typeof sent === 'object' && 'cpu' in sent && sent.cpu && typeof sent.cpu === 'object') {
        fake.cpuShard = Object.fromEntries(
          Object.entries(sent.cpu).map(([shard, cpu]) => [shard, Number(cpu)]),
        );
      }
      fake.updated = Date.now();
    }
    return route.fulfill({
      status: 200,
      headers: corsHeaders(request),
      contentType: 'application/json',
      body: JSON.stringify(reply),
    });
  });
  await page.goto(`/#/${server.slug}/`);
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  await accountMenu(page, account.username, 'Profile');
  return page.getByRole('region', { name: 'CPU per shard' });
}

test.describe('CPU per shard', { tag: '@openscreeps' }, () => {
  test.skip(
    ({ target }) => target === 'community',
    "the stock community server lacks screeps.com's contract",
  );

  test('saves an allocation that totals the CPU and shows the new one', async ({
    page,
    server,
    account,
  }) => {
    const fake: Fake = { cpuShard: { shard0: 60, shard1: 40 }, updated: Date.now() - 13 * HOUR, sent: [] };
    const section = await openCpuShards(page, server, account, fake);
    const shard0 = section.getByLabel('shard0', { exact: true });
    const shard1 = section.getByLabel('shard1', { exact: true });
    const save = section.getByRole('button', { name: 'Save CPU allocation' });
    await expect(shard0).toHaveValue('60');
    await expect(shard1).toHaveValue('40');
    await expect(section).toContainText('Total: 100 of 100 CPU');
    await expect(section).toContainText('You can change it now.');
    await expect(save).toBeEnabled();

    // A total other than the account's CPU cannot be saved.
    await shard0.fill('70');
    await expect(section).toContainText('Total: 110 of 100 CPU');
    await expect(section).toContainText('Give them exactly 100 CPU.');
    await expect(save).toBeDisabled();
    await shard1.fill('x');
    await expect(shard1).toHaveAttribute('aria-invalid', 'true');
    await expect(save).toBeDisabled();
    await shard1.fill('30');
    await expect(section).toContainText('Total: 100 of 100 CPU');
    await expect(save).toBeEnabled();

    await save.click();
    await expect(section.getByRole('status').filter({ hasText: 'CPU allocation saved.' })).toBeVisible();
    expect(fake.sent).toEqual(['{"cpu":{"shard0":70,"shard1":30}}']);
    // The refreshed profile holds the new allocation and the time of the change.
    await expect(shard0).toHaveValue('70');
    await expect(shard1).toHaveValue('30');
    await expect(section).toContainText(/Next change allowed: /);
    await expect(save).toBeDisabled();
  });

  test('waits 12 hours after a change, and shows a refusal', async ({ page, server, account }) => {
    const fake: Fake = { cpuShard: { shard0: 60, shard1: 40 }, updated: Date.now() - HOUR, sent: [] };
    const section = await openCpuShards(page, server, account, fake);
    const save = section.getByRole('button', { name: 'Save CPU allocation' });
    await expect(section.getByLabel('shard0', { exact: true })).toHaveValue('60');
    await expect(section).toContainText(/Next change allowed: /);
    await expect(save).toBeDisabled();
    expect(fake.sent).toEqual([]);

    // Once the cooldown is over (here: the profile reloads with an older change), the server
    // decides; its refusal shows in the section.
    fake.updated = Date.now() - 13 * HOUR;
    fake.refusal = 'cpu change cooldown';
    await page.reload();
    await expect(save).toBeEnabled();
    await save.click();
    await expect(alerts(section)).toHaveText('cpu change cooldown');
    expect(fake.sent).toEqual(['{"cpu":{"shard0":60,"shard1":40}}']);
  });

  test('stays away with one shard: it always gets all the CPU', async ({ page, server, account }) => {
    const fake: Fake = { cpuShard: { shard0: 100 }, updated: Date.now() - 13 * HOUR, sent: [] };
    const section = await openCpuShards(page, server, account, fake);
    await expect(page.getByRole('heading', { name: account.username, level: 1 })).toBeVisible();
    await expect(page.getByText('CPU limit', { exact: true })).toBeVisible();
    await expect(section).toHaveCount(0);
  });
});
