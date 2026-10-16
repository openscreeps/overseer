// What the world map says about its rooms before a click: hovering a room shows its tooltip
// (name, the tile once tiles matter, owner and level, status, portal destinations) and names it
// in the status bar; owners' badges are fetched for the rooms in view and kept in the cache; the
// status bar counts what is still loading; a portal room lists where its portals lead and "Go
// to" takes the map there.
//
// Untagged tests only read, around the signed-in account's `home` room. The harness's 11 × 11
// world (W5N5 to E5S5) has no portals of its own (its sector centre W5N5 has none): the portal
// test links W5N5 with the crossroads W0N0 through the server's admin API, once per world (both
// are rooms the map scans for portals); the status test closes the highway room W0S4 (no player
// can start there), which puts it outside the world's borders.

import type { Page } from '@playwright/test';
import { adminOf, api, expect, Session, test, type Server } from './fixtures';
import {
  breadcrumb,
  hovered,
  hoverTile,
  level,
  openRoom,
  status,
  tilePoint,
  worldMap,
} from './world-map';

declare global {
  interface Window {
    /** Every text the status bar's loading item showed since the page loaded. */
    e2eProgress?: string[];
  }
}

/** The portal pair of the portal test. */
const PORTAL_ROOM = 'W5N5';
const PORTAL_TARGET = 'W0N0';

/** The room the status test closes (`room-close`: out of borders). */
const CLOSED_ROOM = 'W0S4';

/** Calls the server's admin API (`POST /admin`) with `body`; returns whether it succeeded. */
async function admin(server: Server, body: Record<string, unknown>): Promise<Response> {
  return fetch(new URL('admin', server.url), {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${adminOf(server).token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
}

/** Whether `room` has portals leading to `target` (`GET /api/game/room-objects`). */
async function hasPortals(server: Server, room: string, target: string): Promise<boolean> {
  const { body } = await api<{
    objects: { type: string; destination?: { room: string } }[];
  }>(server.url, `api/game/room-objects?room=${room}`);
  return body.objects.some(
    (object) => object.type === 'portal' && object.destination?.room === target,
  );
}

/**
 * Links {@link PORTAL_ROOM} and {@link PORTAL_TARGET} with a portal pair (`portal-create`)
 * unless they already are: the projects share the world.
 */
async function ensurePortals(server: Server): Promise<void> {
  if (await hasPortals(server, PORTAL_ROOM, PORTAL_TARGET)) return;
  const reply = await admin(server, {
    action: 'portal-create',
    from: PORTAL_ROOM,
    to: PORTAL_TARGET,
  });
  // A concurrent project may have linked them meanwhile (then there is no room for more).
  if (!reply.ok && !(await hasPortals(server, PORTAL_ROOM, PORTAL_TARGET))) {
    throw new Error(`portal-create: HTTP ${reply.status} ${await reply.text()}`);
  }
}

/** Closes {@link CLOSED_ROOM} (`room-close`) unless it is closed already. */
async function ensureClosed(server: Server): Promise<void> {
  // Idempotent: the room's status turns from `normal` to `out of borders`, once.
  const reply = await admin(server, { action: 'room-close', room: CLOSED_ROOM });
  if (!reply.ok) throw new Error(`room-close: HTTP ${reply.status} ${await reply.text()}`);
}

/** Moves the mouse to the middle of tile `(x, y)` of `room` (its top centre and the next's). */
async function hoverTileCentre(page: Page, room: string, x: number, y: number) {
  const top = await tilePoint(page, room, x, y);
  const next = await tilePoint(page, room, x, y + 1);
  await page.mouse.move(top.x, (top.y + next.y) / 2, { steps: 3 });
}

/**
 * Collects the users whose badges `page` fetches from now on
 * (`GET /api/user/badge-svg?username=…`).
 */
function badgeRequests(page: Page): string[] {
  const users: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.endsWith('/api/user/badge-svg')) {
      users.push(url.searchParams.get('username') ?? '');
    }
  });
  return users;
}

/** The keys of the badge cache (IndexedDB `overseer-terrain`, store `badges`). */
async function cachedBadges(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const { promise, resolve, reject } = Promise.withResolvers<string[]>();
    const open = indexedDB.open('overseer-terrain');
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const keys = open.result.transaction('badges').objectStore('badges').getAllKeys();
      keys.onsuccess = () => resolve(keys.result.map(String));
      keys.onerror = () => reject(keys.error);
    };
    return promise;
  });
}

/** `page` recording every text of the status bar's loading item from the first load. */
const recorded = test.extend({
  page: async ({ page }, use) => {
    await page.addInitScript(() => {
      const shown: string[] = [];
      window.e2eProgress = shown;
      new MutationObserver(() => {
        const item = document.querySelector('[role="status"][title^="Loading the world"]');
        const text = item?.textContent ?? '';
        if (text && shown.at(-1) !== text) shown.push(text);
      }).observe(document, { subtree: true, childList: true, characterData: true });
    });
    await use(page);
  },
});

/**
 * The signed-in player with a badge (fresh accounts have none, and only badges are drawn), set
 * before the page opens the world and fetches the room's overview with the badge's parameters.
 */
const badged = test.extend({
  presence: async ({ presence, server }, use) => {
    if (!presence.player) throw new Error('no player to give a badge');
    const session = await Session.signIn(
      server.url,
      presence.player.username,
      presence.player.password,
    );
    await session.call('api/user/badge', {
      badge: { type: 7, color1: '#2b6cb0', color2: '#f6ad55', color3: '#f7fafc', param: 0, flip: false },
    });
    await use(presence);
  },
});

test.describe('map', () => {
  test('names the hovered room and tile in a tooltip and the status bar', async ({
    signedInPage: page,
    home,
    user,
  }) => {
    await openRoom(page, home.path);
    await hoverTileCentre(page, home.room, 25, 25);
    const tip = page.getByRole('tooltip');
    await expect(tip).toBeVisible();
    await expect(tip).toContainText(home.room);
    await expect(tip).toContainText('25,25');
    await expect(hovered(page)).toHaveText(`${home.room} 25,25`);
    // The owner and level come with the room's overview.
    await expect(tip).toContainText(`${user.username} · RCL`, { timeout: 30_000 });

    // Dragging the map hides the tooltip; leaving the map too.
    const box = await worldMap(page).boundingBox();
    if (!box) throw new Error('the map has no box');
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 40, box.y + box.height / 2 + 30, { steps: 4 });
    await expect(tip).toBeHidden();
    await page.mouse.up();
    await hoverTile(page, home.room, 25, 25);
    await expect(tip).toBeVisible();
    await status(page).hover();
    await expect(tip).toBeHidden();
    await expect(hovered(page)).toHaveText('');
  });

  test('names only the room while rooms are too small for tiles', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path);
    // Three Escapes zoom out 11×: rooms of about fifty pixels, the sectors level.
    await worldMap(page).focus();
    for (let i = 0; i < 3; i++) await page.keyboard.press('Escape');
    await expect(level(page)).toHaveText(/^(Sectors|World)$/);
    await hoverTile(page, home.room, 25, 25);
    await expect(hovered(page)).toHaveText(home.room);
    await expect(page.getByRole('tooltip')).toContainText(home.room);
    await expect(page.getByRole('tooltip')).not.toContainText(/\d+,\d+/);
  });

  test('says in the tooltip that a room is outside the world @openscreeps', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await ensureClosed(server);
    // Zoomed out over the whole (small) world, where every room's overview loads.
    await openRoom(page, home.path);
    await worldMap(page).focus();
    for (let i = 0; i < 12; i++) await page.keyboard.press('Escape');
    await hoverTile(page, CLOSED_ROOM, 25, 25);
    await expect(page.getByRole('tooltip')).toContainText(CLOSED_ROOM);
    await expect(page.getByRole('tooltip')).toContainText('Outside the world', {
      timeout: 30_000,
    });
  });

  badged(
    "fetches the owners' badges of the rooms in view, then takes them from the cache @openscreeps",
    async ({ signedInPage: page, home, user }) => {
      await openRoom(page, home.path);
      const requests = badgeRequests(page);
      // Badges show from rooms of ten pixels until the live room's scene covers them: one
      // Escape zooms out to the rooms level. The home room's owner is the user.
      await worldMap(page).focus();
      await page.keyboard.press('Escape');
      await expect(level(page)).toHaveText('Rooms');
      await expect.poll(() => requests, { timeout: 30_000 }).toContain(user.username);
      await expect
        .poll(async () =>
          (await cachedBadges(page)).some((key) => key.includes(`|~badge|${user.id}|`)),
        )
        .toBe(true);

      // After a reload the same view takes the badge from the cache: by the time everything in
      // view is known, it was not fetched again.
      const before = requests.length;
      await page.reload();
      await expect(breadcrumb(page)).toContainText(home.room);
      await expect(level(page)).toHaveText('Rooms', { timeout: 30_000 });
      await expect(status(page).getByTitle('Loading the world')).toHaveCount(0, {
        timeout: 60_000,
      });
      expect(requests.slice(before)).not.toContain(user.username);
    },
  );

  recorded('counts what is loading in the status bar until the rooms in view are known', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path);
    const loading = status(page).getByTitle('Loading the world');
    await expect(loading).toHaveCount(0, { timeout: 60_000 });
    const shown = await page.evaluate(() => window.e2eProgress ?? []);
    expect(shown.length).toBeGreaterThan(0);
    // `Terrain 12 / 40 · Owners 3 / 40 · Portals 0 / 40`: never more known than wanted.
    for (const text of shown) {
      const counts = [...text.matchAll(/(Terrain|Owners|Portals) ([\d\u202f]+) \/ ([\d\u202f]+)/g)];
      expect(counts.length, text).toBeGreaterThan(0);
      for (const [, , done, wanted] of counts) {
        const number = (value = '') => Number(value.replaceAll('\u202f', ''));
        expect(number(done), text).toBeLessThanOrEqual(number(wanted));
      }
    }
  });

  test('lists a room’s portals and goes through them @openscreeps', async ({
    signedInPage: page,
    server,
  }) => {
    await ensurePortals(server);
    await openRoom(page, `/#/${server.slug}/-/${PORTAL_ROOM}`);
    await page.getByRole('tab', { name: 'Room' }).click();
    const info = page.getByRole('region', { name: new RegExp(PORTAL_ROOM) });
    const destinations = info.getByRole('list', { name: 'Portal destinations' });
    // The map scans the room for portals, then reads their destinations.
    await expect(destinations).toContainText(`→ ${PORTAL_TARGET}`, { timeout: 60_000 });
    await expect(destinations).toContainText('8 tiles');

    await hoverTile(page, PORTAL_ROOM, 25, 25);
    await expect(page.getByRole('tooltip')).toContainText(`Portal → ${PORTAL_TARGET}`);

    await destinations.getByRole('button', { name: `Go to ${PORTAL_TARGET}` }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${PORTAL_TARGET}$`));
    await expect(breadcrumb(page)).toContainText(PORTAL_TARGET);
    // The other side leads back.
    const back = page
      .getByRole('region', { name: new RegExp(PORTAL_TARGET) })
      .getByRole('list', { name: 'Portal destinations' });
    await expect(back).toContainText(`→ ${PORTAL_ROOM}`, { timeout: 60_000 });
    await hoverTile(page, PORTAL_TARGET, 25, 25);
    await expect(page.getByRole('tooltip')).toContainText(`Portal → ${PORTAL_ROOM}`);

    // And back through one portal picked in the Objects tab: its destination is a link.
    await page.getByRole('tab', { name: /Objects/ }).click();
    const list = page.getByRole('navigation', { name: 'Objects' });
    await list.getByRole('button', { name: /^Portal at \d+,\d+$/ }).first().click();
    const inspector = page.getByRole('region', { name: 'Inspector' });
    await inspector.getByRole('button', { name: `Go to ${PORTAL_ROOM}` }).click();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${PORTAL_ROOM}$`));
    await expect(breadcrumb(page)).toContainText(PORTAL_ROOM);
  });
});
