// The profile (account menu → Profile, route `#/<server>/@profile`): what it shows about the
// account (badge, levels, CPU, credits, world status, start room), its way back to the map and
// sign-out, and editing notification preferences, the e-mail address and the password (each
// checked against the server).

import type { Locator, Page } from '@playwright/test';
import { expect, register, Session, signIn, test, unique } from './fixtures';
import { accountMenu, alerts, corsHeaders, updateUser } from './support/accounts';

/** Opens the profile through the account menu and returns it. */
async function openProfile(page: Page, username: string): Promise<Locator> {
  // The session settles first (it rewrites the saved profile once connected).
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
  await accountMenu(page, username, 'Profile');
  await expect(page.getByRole('heading', { level: 1, name: username })).toBeVisible();
  return page.getByRole('region', { name: username });
}

/** The value of the profile's statistic `term` (`Credits`, `CPU limit`, …). */
async function stat(profile: Locator, term: string): Promise<Locator> {
  await expect(profile.getByRole('term').first()).toBeVisible();
  const terms = await profile.getByRole('term').allTextContents();
  const index = terms.indexOf(term);
  expect(terms, `the profile names ${term}`).toContain(term);
  return profile.getByRole('definition').nth(index);
}

/** `text` as a regular expression matching it literally. */
function literal(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** A start room as the profile shows it: `W8N3`, or `shard3 / W8N3` on a sharded server. */
const START_ROOM = /(?:(\S+) \/ )?([WE]\d+[NS]\d+)/;

test.describe('profile', () => {
  test('shows the account', async ({ signedInPage: page, server, user }) => {
    const profile = await openProfile(page, user.username);
    await expect(page).toHaveTitle(`Account · ${server.name} — Overseer`);
    await expect(page.getByRole('banner').getByText('Account', { exact: true })).toBeVisible();
    await expect(profile).toContainText(server.name);
    // The badge is an image the server drew (`badge-svg`), never markup in the page.
    const badge = profile.getByRole('img', { name: `Badge of ${user.username}` });
    await expect(badge).toHaveAttribute('src', /^data:image\/svg\+xml;base64,/);
    await expect(profile.getByRole('progressbar', { name: /^GCL progress to level \d+$/ })).toBeVisible();
    await expect(
      profile.getByRole('progressbar', { name: /^Power level progress to level \d+$/ }),
    ).toBeVisible();
    await expect(await stat(profile, 'Global Control Level')).toHaveText(/^Level \d+.*[\d,]+ \/ [\d,]+$/);
    await expect(await stat(profile, 'CPU limit')).toHaveText(/^(\d+ CPU|unknown)$/);
    await expect(await stat(profile, 'Credits')).toHaveText(/^\d+\.\d{3}$/);
    await expect(await stat(profile, 'World status')).toHaveText(
      /^(Playing|Lost every spawn|Not in the world \(place a spawn\))$/,
    );
    await expect(await stat(profile, 'Start room')).toContainText(START_ROOM);
  });

  test('goes to the start room or the map, and back', async ({
    signedInPage: page,
    server,
    user,
  }) => {
    let profile = await openProfile(page, user.username);
    const start = await stat(profile, 'Start room');
    await expect(start).toContainText(START_ROOM);
    const [, shard, room] = START_ROOM.exec((await start.textContent()) ?? '') ?? [];
    await profile.getByRole('button', { name: 'Show on the map' }).click();
    await expect(page).toHaveURL(
      new RegExp(`#/${literal(server.slug)}/${literal(shard ?? '-')}/${room}$`),
    );
    await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(room ?? '');
    await expect(profile).toHaveCount(0);

    profile = await openProfile(page, user.username);
    await expect(page).toHaveURL(new RegExp(`#/${literal(server.slug)}/@profile$`));
    await profile.getByRole('button', { name: 'Open the map' }).click();
    await expect(page).toHaveURL(new RegExp(`#/${literal(server.slug)}/`));
    await expect(page).not.toHaveURL(/@profile/);
    await expect(profile).toHaveCount(0);
    // The browser's back button returns to the profile.
    await page.goBack();
    await expect(page.getByRole('heading', { level: 1, name: user.username })).toBeVisible();
  });

  test('signs out', async ({ signedInPage: page, server, user }) => {
    const profile = await openProfile(page, user.username);
    await profile.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByRole('region', { name: `Sign in to ${server.name}` })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Account:/ })).toHaveCount(0);
  });
});

test.describe('profile', { tag: '@openscreeps' }, () => {
  test("shows the player's world status and the live CPU use of its code", async ({
    signedInPage: page,
    player,
  }) => {
    const profile = await openProfile(page, player.username);
    await expect(await stat(profile, 'World status')).toHaveText('Playing');
    await expect(await stat(profile, 'Start room')).toContainText(player.room);
    // The bot runs every tick: its CPU time and Memory size arrive live.
    await expect(await stat(profile, 'CPU used')).toHaveText(/^\d+\.\d{2} ms, Memory \d+\.\d KB$/);
  });

  test('shows levels, CPU limit and credits as the server counts them', async ({
    page,
    server,
    account,
  }) => {
    await updateUser(server, account.username, {
      gcl: 1_500_000,
      power: 5_000,
      credits: 1234.5,
      cpu: 37,
    });
    await signIn(page, server, account);
    const profile = await openProfile(page, account.username);
    // GCL 2 starts at 1,000,000 points, GCL 3 at 2^2.4 × 1,000,000; power level 2 at 4,000,
    // level 3 at 9,000.
    await expect(await stat(profile, 'Global Control Level')).toHaveText(
      /^Level 2.*500,000 \/ 4,278,032$/,
    );
    await expect(profile.getByRole('progressbar', { name: 'GCL progress to level 3' })).toHaveAttribute(
      'value',
      '11.7',
    );
    await expect(await stat(profile, 'Power level')).toHaveText(/^Level 2.*1,000 \/ 5,000$/);
    await expect(
      profile.getByRole('progressbar', { name: 'Power level progress to level 3' }),
    ).toHaveAttribute('value', '20.0');
    await expect(await stat(profile, 'CPU limit')).toHaveText('37 CPU');
    await expect(await stat(profile, 'Credits')).toHaveText('1234.500');
    await expect(await stat(profile, 'World status')).toHaveText('Not in the world (place a spawn)');
  });

  test("shows the account's badge", async ({ page, server, account }) => {
    const session = await Session.signIn(server.url, account.username, account.password);
    await session.call('api/user/badge', {
      badge: { type: 5, color1: '#a01b2c', color2: '#2ca01b', color3: '#1b2ca0', param: 0, flip: false },
    });
    await signIn(page, server, account);
    const profile = await openProfile(page, account.username);
    const badge = profile.getByRole('img', { name: `Badge of ${account.username}` });
    await expect(badge).toHaveAttribute('src', /^data:image\/svg\+xml;base64,/);
    const svg = Buffer.from(
      (await badge.getAttribute('src'))?.replace(/^data:image\/svg\+xml;base64,/, '') ?? '',
      'base64',
    ).toString();
    for (const color of ['#a01b2c', '#2ca01b', '#1b2ca0']) expect(svg).toContain(color);
    // It draws.
    await expect.poll(() => badge.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0);
  });

  test('saves notification preferences', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const notifications = page.getByRole('region', { name: 'Notifications' });
    const enabled = notifications.getByRole('checkbox', { name: 'Send notification e-mails' });
    const messages = notifications.getByRole('checkbox', { name: 'Notify about new messages' });
    const online = notifications.getByRole('checkbox', { name: 'Notify even while I am online' });
    const interval = notifications.getByLabel('Send interval');
    const errors = notifications.getByLabel('Script error reports');
    // The official client's defaults.
    await expect(enabled).toBeChecked();
    await expect(messages).toBeChecked();
    await expect(online).not.toBeChecked();
    await expect(interval.getByRole('option', { selected: true })).toHaveText('1 hour');
    await expect(errors.getByRole('option', { selected: true })).toHaveText('Every 30 minutes');

    await enabled.uncheck();
    await messages.uncheck();
    await online.check();
    await interval.selectOption({ label: '3 hours' });
    await errors.selectOption({ label: 'Never' });
    await notifications.getByRole('button', { name: 'Save notifications' }).click();
    await expect(notifications.getByRole('status')).toHaveText('Notification preferences saved.');

    await page.reload();
    await expect(enabled).not.toBeChecked();
    await expect(messages).not.toBeChecked();
    await expect(online).toBeChecked();
    await expect(interval.getByRole('option', { selected: true })).toHaveText('3 hours');
    await expect(errors.getByRole('option', { selected: true })).toHaveText('Never');
  });

  test('changes the e-mail address', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'E-mail' });
    const field = section.getByLabel('E-mail address');
    await expect(field).toHaveValue(account.email);
    const address = `${unique('mail')}@e2e.invalid`;
    await field.fill(address);
    await section.getByRole('button', { name: 'Save e-mail' }).click();
    await expect(section.getByRole('status')).toHaveText('E-mail address saved.');
    // The server signs the account in with the new address.
    await Session.signIn(server.url, address, account.password);
  });

  test('checks the e-mail address, and shows why the server refuses one', async ({
    page,
    server,
    account,
  }) => {
    const other = await register(server.url);
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'E-mail' });
    const field = section.getByLabel('E-mail address');
    const save = section.getByRole('button', { name: 'Save e-mail' });
    await field.fill('not-an-address');
    await expect(field).toHaveAccessibleDescription('Enter a valid e-mail address');
    await expect(field).toHaveAttribute('aria-invalid', 'true');
    await save.click();
    await expect(alerts(section).last()).toHaveText('Enter a valid e-mail address');

    await field.fill(other.email);
    await expect(field).toHaveAttribute('aria-invalid', 'false');
    await save.click();
    await expect(alerts(section)).toHaveText('email already exists');
    await expect(section.getByRole('status').filter({ hasText: /\S/ })).toHaveCount(0);
    await Session.signIn(server.url, account.email, account.password);
  });

  test('changes the password and stays signed in', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'Password' });
    const password = unique('new-pass-');
    await section.getByLabel('Current password').fill(account.password);
    await section.getByLabel('New password', { exact: true }).fill(password);
    await section.getByLabel('Repeat the new password').fill(password);
    await section.getByRole('button', { name: 'Change password' }).click();
    // Private servers revoke every session of the account: the app signs in again and says so.
    await expect(page.getByRole('alert').filter({ hasText: 'Password changed' })).toHaveText(
      /Password changed; signed in again with the new password\./,
    );
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected');
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();

    await Session.signIn(server.url, account.username, password);
    await expect(Session.signIn(server.url, account.username, account.password)).rejects.toThrow();
  });

  test('checks the new password before sending it', async ({ page, server, account }) => {
    const sent: string[] = [];
    page.on('request', (request) => {
      if (new URL(request.url()).pathname.endsWith('/api/user/password')) sent.push(request.method());
    });
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'Password' });
    const fresh = section.getByLabel('New password', { exact: true });
    const repeat = section.getByLabel('Repeat the new password');
    const change = section.getByRole('button', { name: 'Change password' });
    await expect(fresh).toHaveAccessibleDescription('8 to 1024 characters.');
    await section.getByLabel('Current password').fill(account.password);

    await fresh.fill('short');
    await expect(fresh).toHaveAccessibleDescription(/Use at least 8 characters/);
    await change.click();
    await expect(alerts(section).last()).toHaveText('Use at least 8 characters');

    await fresh.fill('long-enough-1');
    await repeat.fill('long-enough-2');
    await expect(repeat).toHaveAccessibleDescription('The passwords do not match');
    await change.click();
    await expect(alerts(section).last()).toHaveText('The passwords do not match');
    expect(sent).toEqual([]);
    await Session.signIn(server.url, account.username, account.password);
  });

  // On the community server Overseer checks the current password by signing in with it
  // first: screepsmod-auth would answer a wrong one, then erase the password and crash.
  test('rejects a wrong current password', async ({ page, server, account }) => {
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'Password' });
    await section.getByLabel('Current password').fill(`${account.password}-wrong`);
    await section.getByLabel('New password', { exact: true }).fill('long-enough-1');
    await section.getByLabel('Repeat the new password').fill('long-enough-1');
    await section.getByRole('button', { name: 'Change password' }).click();
    await expect(alerts(section)).toHaveText('the current password is wrong');
    await expect(page.getByRole('button', { name: `Account: ${account.username}` })).toBeVisible();
    await Session.signIn(server.url, account.username, account.password);
  });

  // Accounts made through Steam or GitHub have no password until they set one: the server says
  // so in `GET /api/auth/me` (`password: false`), rewritten here.
  test('asks an account without a password only for the new one', async ({
    page,
    server,
    account,
  }) => {
    await page.route('**/api/auth/me', async (route) => {
      const request = route.request();
      if (request.method() !== 'GET') return route.fallback();
      const response = await route.fetch();
      const me = { ...((await response.json()) as Record<string, unknown>), password: false };
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
    await signIn(page, server, account);
    await openProfile(page, account.username);
    const section = page.getByRole('region', { name: 'Password' });
    await expect(section.getByLabel('New password', { exact: true })).toBeVisible();
    await expect(section.getByLabel('Repeat the new password')).toBeVisible();
    await expect(section.getByLabel('Current password')).toHaveCount(0);
  });
});
