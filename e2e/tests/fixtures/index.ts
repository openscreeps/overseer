// Playwright fixtures of the Overseer UI tests: `import { test, expect } from './fixtures';`.
//
// Every target:
// - `target` (worker): what the tests run against (`OVERSEER_E2E_TARGET`): `openscreeps`, the
//   disposable world `cargo xtask e2e` starts, or `official`, screeps.com
//   (`cargo xtask e2e --target official`, read-only). Tests that need a disposable world (the
//   admin API, created players, registration, account changes, intents) are tagged
//   `@openscreeps`; the official run leaves them out.
// - `mode` (worker option): how the app is served. `direct`: a plain static server, the app
//   talks to the game server cross-origin. `proxy`: `overseer-proxy --static` serves the app
//   same-origin in front of the game server. Each Playwright project sets it and its baseURL;
//   the official run has the `proxy` project only (screeps.com sends no CORS headers).
// - `server` (worker): the server under test ({@link Server}; `admin` on `openscreeps` only).
// - `user`: the account `signedInPage` is signed in as ({@link User}): a fresh `player` on
//   `openscreeps`, the API token's account on `official`.
// - `home`: a room that shows the user's presence ({@link Home}): the player's room on
//   `openscreeps`, the account's start room (its own room when it has one) on `official`.
// - `signedInPage`: `page` already signed in as `user` (a password session on `openscreeps`, the
//   API token on `official`), on the world of the server (`/#/<slug>/`). On `official` it
//   answers "Not now" whenever the "Rate limit running low" prompt opens (the account's real
//   budget runs low across runs).
// - `apiToken`: the `official` account's API token (`undefined` on `openscreeps`). Never print
//   it or assert on it: the official run records no traces, screenshots or videos.
// - `cleanup`: work for after the test's browser context has closed (the app never sees it).
// - `context`/`page`: with `OVERSEER_E2E_SLOW=<factor>` (see `playwright.config.ts`), every page
//   of the context, popups included, runs its CPU `<factor>` times slower and waits
//   `<factor>` × 125 ms longer for every response (the game server on the CI runner answers in
//   0.5 to 1.5 s, while it shares the runner's four cores with two browsers). On `official`,
//   the context blocks every request that may change something (`isMutating`: anything but
//   `GET`, `HEAD`, `OPTIONS` and the read-only POSTs of `READ_ONLY_POSTS`) and fails the test
//   that sent one.
//
// `openscreeps` only (tag the test `@openscreeps`):
// - `account`: a fresh registered account without a spawn.
// - `player`: a fresh account with a spawn and a small bot running (creeps move), removed with
//   everything it owns once the test's browser context has closed. The same account as `user`.
// - `signIn(page, server, account)` seeds a signed-in session the way the app stores it (profile
//   and token in localStorage) and opens the server's world; the UI then resumes it.
// - `addServer`, `createPlayer`, `register`, `removeUser`, `adminOf` (the admin API).

import { test as base, expect, type Page } from '@playwright/test';
import {
  api,
  createPlayer,
  currentTarget,
  isMutating,
  liveServer,
  liveToken,
  register,
  removeUser,
  Session,
  SERVER_NAME,
} from './api';
import type { Account, Player, Server, Target } from './api';

export * from './api';
export { expect };

/** How the app under test is served. */
export type Mode = 'direct' | 'proxy';

/** A signed-in account. */
export interface User {
  id: string;
  username: string;
}

/** A room that shows the user's presence. */
export interface Home {
  /** Room name, e.g. `W6S2`. */
  room: string;
  /** Its shard on a sharded server (screeps.com); `null` on a single-shard server. */
  shard: string | null;
  /** The app's route of the room: `/#/<slug>/<shard or ->/<room>`, for `page.goto`. */
  path: string;
}

/** Who `signedInPage` signs in as, and where they are. */
interface Presence {
  user: User;
  home: Home;
  /** The account's session token or API token. */
  token: string;
  /** The created player (`openscreeps` only). */
  player?: Player;
}

/** How many times slower the pages run (`OVERSEER_E2E_SLOW`); 1 is full speed. */
const SLOWDOWN = Math.max(1, Number(process.env.OVERSEER_E2E_SLOW ?? 1) || 1);

/** The slowdown of each page, applied once. */
const slowed = new WeakMap<Page, Promise<void>>();

/** Slows `page` down by {@link SLOWDOWN}: CPU and network (Chrome DevTools Protocol). */
function slowDown(page: Page): Promise<void> {
  let applied = slowed.get(page);
  if (!applied) {
    applied = (async () => {
      const cdp = await page.context().newCDPSession(page);
      await cdp.send('Emulation.setCPUThrottlingRate', { rate: SLOWDOWN });
      await cdp.send('Network.enable');
      await cdp.send('Network.emulateNetworkConditions', {
        offline: false,
        latency: SLOWDOWN * 125,
        downloadThroughput: -1,
        uploadThroughput: -1,
      });
    })();
    slowed.set(page, applied);
  }
  return applied;
}

/** The {@link Home} of `room` on `server`. */
function homeOf(server: Server, room: string, shard: string | null): Home {
  return { room, shard, path: `/#/${server.slug}/${shard ?? '-'}/${room}` };
}

/**
 * The API token's account and start room on screeps.com, asked once per worker (two GET
 * requests): screeps.com's rate limits are per account.
 */
let official: Promise<Presence> | undefined;

function officialPresence(server: Server, token: string): Promise<Presence> {
  official ??= (async () => {
    const { body: me } = await api<{ _id: string; username: string }>(server.url, 'api/auth/me', {
      token,
    });
    const { body: start } = await api<{ room: string[] }>(server.url, 'api/user/world-start-room', {
      token,
    });
    // `shard3/W6S2` on a sharded server.
    const [shard, room] = start.room[0]?.includes('/')
      ? (start.room[0].split('/') as [string, string])
      : [null, start.room[0]];
    if (!room) throw new Error('screeps.com names no start room for the account');
    return { user: { id: me._id, username: me.username }, home: homeOf(server, room, shard), token };
  })();
  return official;
}

/**
 * Seeds the app's storage of `page` with `server` saved and `user` signed in with `token` (the
 * profile in `overseer.profiles`, the token in `overseer.secret.token:<url>`), once per tab
 * (reloads keep what the app wrote since), and opens the server's world.
 */
async function seed(
  page: Page,
  server: Server,
  user: User,
  token: string,
  login: string | null,
): Promise<void> {
  const profiles = {
    version: 2,
    servers: [
      {
        name: server.name,
        url: server.url,
        kind: server.kind,
        login,
        user: {
          id: user.id,
          username: user.username,
          ...(server.kind === 'official' ? { api_token: true } : {}),
        },
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
    { profiles, url: server.url, token },
  );
  await page.goto(`/#/${server.slug}/`);
}

/**
 * Adds `server` as a private server through the servers screen (`#/`), named `name`: the "Add
 * server" dialog closes and a toast confirms.
 */
export async function addServer(page: Page, server: Server, name = SERVER_NAME): Promise<void> {
  await page.goto('/#/');
  await page
    .getByRole('region', { name: 'Servers' })
    .getByRole('button', { name: 'Add server' })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Add server' });
  await dialog.getByLabel('Name', { exact: true }).fill(name);
  await dialog.getByLabel('Address', { exact: true }).fill(server.url);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('status').filter({ hasText: `Added ${name}` })).toBeVisible();
}

/**
 * Seeds the app's storage of `page` with `server` saved and `account` signed in with a fresh
 * session token, and opens the server's world (`openscreeps` only: signs in with the password).
 */
export async function signIn(page: Page, server: Server, account: Account): Promise<void> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const me = await session.me();
  await seed(page, server, { id: me._id, username: me.username }, session.token, account.username);
}

/** Fails a fixture that only the disposable world has. */
function openscreepsOnly(target: Target, fixture: string): void {
  if (target !== 'openscreeps') {
    throw new Error(`\`${fixture}\` needs a disposable world: tag the test @openscreeps`);
  }
}

export const test = base.extend<
  {
    account: Account;
    player: Player;
    user: User;
    home: Home;
    signedInPage: Page;
    cleanup: (() => Promise<void>)[];
    presence: Presence;
  },
  { mode: Mode; target: Target; server: Server; apiToken: string | undefined }
>({
  mode: ['direct', { option: true, scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  target: [async ({}, use) => use(currentTarget()), { scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  server: [async ({}, use) => use(liveServer()), { scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  apiToken: [async ({}, use) => use(liveToken()), { scope: 'worker' }],
  // Set up before the browser context (which depends on it), so torn down after it closed.
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  cleanup: async ({}, use) => {
    const tasks: (() => Promise<void>)[] = [];
    await use(tasks);
    for (const task of tasks.reverse()) await task();
  },
  context: async ({ cleanup, context, target }, use) => {
    void cleanup;
    if (SLOWDOWN > 1) {
      // Popups too; a page closed before the session opens needs no slowing down.
      context.on('page', (page) => void slowDown(page).catch(() => {}));
    }
    if (target !== 'official') {
      await use(context);
      return;
    }
    // The request guard: block, and fail the test on, every request that may change something.
    // The listener also sees requests a test's own route sent on with `route.continue()`.
    const refused = new Set<string>();
    context.on('request', (request) => {
      if (isMutating(request.method(), request.url())) {
        refused.add(`${request.method()} ${new URL(request.url()).pathname}`);
      }
    });
    await context.route('**/*', (route) => {
      const request = route.request();
      return isMutating(request.method(), request.url())
        ? route.abort('blockedbyclient')
        : route.fallback();
    });
    await use(context);
    expect([...refused], 'requests that may change the screeps.com account').toEqual([]);
  },
  page: async ({ page }, use) => {
    if (SLOWDOWN > 1) await slowDown(page);
    await use(page);
  },
  account: async ({ target, server }, use) => {
    openscreepsOnly(target, 'account');
    await use(await register(server.url));
  },
  presence: async ({ target, server, apiToken, cleanup }, use) => {
    if (target === 'official') {
      if (!apiToken) throw new Error('OVERSEER_LIVE_TOKEN is not set');
      await use(await officialPresence(server, apiToken));
      return;
    }
    const player = await createPlayer(server.url);
    cleanup.push(() => removeUser(server, player.id));
    const session = await Session.signIn(server.url, player.username, player.password);
    await use({
      user: { id: player.id, username: player.username },
      home: homeOf(server, player.room, null),
      token: session.token,
      player,
    });
  },
  player: async ({ target, presence }, use) => {
    openscreepsOnly(target, 'player');
    if (!presence.player) throw new Error('no player was created');
    await use(presence.player);
  },
  user: async ({ presence }, use) => use(presence.user),
  home: async ({ presence }, use) => use(presence.home),
  signedInPage: async ({ page, server, presence, target }, use) => {
    if (target === 'official') {
      // The account's real budget runs low across runs: the prompt to lift the limits (on the
      // screeps.com website) would cover the page. "Not now" only dismisses it for the session;
      // the app prompts once per endpoint, so another prompt may follow at once (no waiting
      // for the dialog to stay hidden).
      await page.addLocatorHandler(
        page.getByRole('dialog', { name: 'Rate limit running low' }),
        (dialog) => dialog.getByRole('button', { name: 'Not now' }).click(),
        { noWaitAfter: true },
      );
    }
    const login = presence.player?.username ?? null;
    await seed(page, server, presence.user, presence.token, login);
    await use(page);
  },
});
