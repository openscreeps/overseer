// Playwright fixtures of the Overseer UI tests: `import { test, expect } from './fixtures';`.
//
// - `mode` (worker option): how the app is served. `direct`: a plain static server, the app
//   talks to the game server cross-origin. `proxy`: `overseer-proxy --static` serves the app
//   same-origin in front of the game server. Each Playwright project sets it and its baseURL.
// - `server` (worker): the disposable game server ({@link Server}).
// - `account`: a fresh registered account without a spawn.
// - `player`: a fresh account with a spawn and a small bot running (creeps move).
// - `signedInPage`: `page` already signed in as `player`, on the world of the server.
//
// `signIn(page, server, account)` seeds a signed-in session the way the app stores it (profile
// and token in localStorage) and opens the server's world; the UI then resumes it.

import { test as base, expect, type Page } from '@playwright/test';
import { createPlayer, register, Session, liveServer } from './api';
import type { Account, Player, Server } from './api';

export * from './api';
export { expect };

/** How the app under test is served. */
export type Mode = 'direct' | 'proxy';

/** The server's display name in seeded profiles. */
export const SERVER_NAME = 'Local e2e';

/** Adds `server` as a private server through the servers screen (`#/`), named `name`. */
export async function addServer(page: Page, server: Server, name = SERVER_NAME): Promise<void> {
  await page.goto('/#/');
  const form = page.getByRole('region', { name: 'Add server' });
  await form.getByLabel('Name', { exact: true }).fill(name);
  await form.getByLabel('Address', { exact: true }).fill(server.url);
  await form.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(form.getByRole('status')).toHaveText(`Added ${server.url}`);
}

/**
 * Seeds the app's storage of `page` with `server` saved and `account` signed in (the profile in
 * `overseer.profiles`, a fresh session token in `overseer.secret.token:<url>`), once per tab
 * (reloads keep what the app wrote since), and opens the server's world.
 */
export async function signIn(page: Page, server: Server, account: Account): Promise<void> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const me = await session.me();
  const profiles = {
    version: 2,
    servers: [
      {
        name: SERVER_NAME,
        url: server.url,
        kind: 'private',
        login: account.username,
        user: { id: me._id, username: me.username },
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

export const test = base.extend<
  { account: Account; player: Player; signedInPage: Page },
  { mode: Mode; server: Server }
>({
  mode: ['direct', { option: true, scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  server: [async ({}, use) => use(liveServer()), { scope: 'worker' }],
  account: async ({ server }, use) => use(await register(server.url)),
  player: async ({ server }, use) => use(await createPlayer(server.url)),
  signedInPage: async ({ page, server, player }, use) => {
    await signIn(page, server, player);
    await use(page);
  },
});
