// The world screen: opening a server's world (the account's home room on a first visit, the
// loading and failure states), continuous zoom from the whole world into a live room, the
// whole-world button and breadcrumb, shards, the route (address bar, back button), popping a
// room out into its own window and what survives a reload (route, camera, view mode, terrain
// cache).
//
// Untagged tests only read: they pass on a disposable OpenScreeps world and on screeps.com,
// around the signed-in account's `home` room.

import type { Page, Request } from '@playwright/test';
import {
  expect,
  Session,
  signIn,
  test,
  type Account,
  type Home,
  type Server,
} from './fixtures';
import {
  breadcrumb,
  camera,
  expectDrawn,
  LEVELS,
  level,
  levelOf,
  nextFrames,
  openRoom,
  restingCamera,
  roomCentre,
  roomInfo,
  routeOf,
  savedPlace,
  search,
  shardKey,
  status,
  tilePoint,
  towardsMiddle,
  worldBounds,
  worldMap,
  roomName,
  type Level,
  type SavedPlace,
} from './world-map';

declare global {
  interface Window {
    /** The levels of detail the status bar showed, recorded by `recordLevels`. */
    e2eLevels?: string[];
    /** `overseer.state` as the page found it when it loaded (the restore test). */
    e2eSavedState?: string | null;
  }
}

/** The rooms in the terrain cache (IndexedDB `overseer-terrain`, keys `…|<room>`). */
async function cachedRooms(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const { promise, resolve, reject } = Promise.withResolvers<string[]>();
    const open = indexedDB.open('overseer-terrain');
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const keys = open.result.transaction('terrain').objectStore('terrain').getAllKeys();
      keys.onsuccess = () =>
        resolve(keys.result.map((key) => String(key).split('|').at(-1) ?? ''));
      keys.onerror = () => reject(keys.error);
    };
    return promise;
  });
}

/**
 * Records every zoom level the status bar shows from now on (a DOM observer, so no level is
 * missed however fast it passes); the returned function reads them, in order.
 */
async function recordLevels(page: Page): Promise<() => Promise<string[]>> {
  await level(page).evaluate((element) => {
    const shown = [element.textContent ?? ''];
    window.e2eLevels = shown;
    new MutationObserver(() => {
      const text = element.textContent ?? '';
      if (shown.at(-1) !== text) shown.push(text);
    }).observe(element, { characterData: true, childList: true, subtree: true });
  });
  return () => page.evaluate(() => window.e2eLevels ?? []);
}

/**
 * Opens the world on `home` (live), zooms out with Escape (around the centre, so the room stays
 * there) until the overview level or as far as the map goes, then wheels back in over the
 * centre until the room is live: continuous zoom, no click. Returns the most zoomed-out level
 * and every level the status bar showed on the way in.
 */
async function zoomIntoLiveRoom(page: Page, home: Home): Promise<{ lowest: Level; levels: string[] }> {
  await openRoom(page, home.path);
  const map = worldMap(page);
  await map.focus();
  // Escape zooms out 2.25× at once: twelve presses pass the zoom limit of any world (it is
  // clamped). A screeps.com shard reaches the overview in four, long before the limit: no
  // further, so that the map does not load the whole shard.
  for (let i = 0; i < 12; i++) {
    await page.keyboard.press('Escape');
    await nextFrames(page);
    if ((await levelOf(page)) === 'World') break;
  }
  await nextFrames(page);
  const lowest = await levelOf(page);
  const levels = await recordLevels(page);
  const box = await map.boundingBox();
  if (!box) throw new Error('the map has no box');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  // A wheel step zooms in 1.16× (eased over a few frames), a level spans 3× or more: one step
  // per drawn frame cannot skip a level, however slow the frames.
  for (let i = 0; i < 80 && (await levelOf(page)) !== 'Room'; i++) {
    await page.mouse.wheel(0, -100);
    await nextFrames(page);
  }
  await expect(level(page)).toHaveText('Room');
  return { lowest, levels: await levels() };
}

/** The account's start room, as the server names it (`GET /api/user/world-start-room`). */
async function startRoom(server: Server, account: Account): Promise<string> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const { room } = await session.call<{ room: string[] }>('api/user/world-start-room');
  if (!room[0]) throw new Error('the server names no start room');
  return room[0];
}

/** The room in focus is live in the drawer. */
async function expectLive(page: Page, room: string) {
  await expect(
    roomInfo(page, room).getByRole('status').filter({ hasText: 'Live' }),
  ).toBeVisible({ timeout: 15_000 });
}

/** The accessible name of the world map with no room in focus. */
const mapName = (home: Home) => (home.shard ? `Map of ${home.shard}` : 'World map');

/** Lets a cross-origin page (direct mode) read a rewritten response. */
function corsHeaders(request: Request): Record<string, string> {
  const origin = request.headers()['origin'];
  return origin ? { 'access-control-allow-origin': origin } : {};
}

/** A world-size reply of the test (`null`: the server answers). */
type WorldSizeReply = { status: number; body: unknown } | null;

/** How the test answers world-size requests. */
interface WorldSize {
  /** The reply (`null`: the server answers). */
  reply: WorldSizeReply;
  /** Until it settles, requests wait (the world stays loading). */
  hold: Promise<void> | null;
}

/**
 * `page` with `GET /api/game/world-size` answered by the test from the start: the option
 * `worldSizeReply` sets the first reply (the signed-in page opens the world through it on a
 * first visit), `worldSize` the following ones.
 */
const rewritten = test.extend<{
  worldSizeReply: WorldSizeReply;
  worldSize: WorldSize;
}>({
  worldSizeReply: [null, { option: true }],
  worldSize: async ({ worldSizeReply }, use) => use({ reply: worldSizeReply, hold: null }),
  page: async ({ page, worldSize }, use) => {
    await page.route(/\/api\/game\/world-size/, async (route) => {
      await worldSize.hold;
      const reply = worldSize.reply;
      if (!reply) return route.continue();
      return route.fulfill({
        status: reply.status,
        contentType: 'application/json',
        headers: corsHeaders(route.request()),
        body: JSON.stringify(reply.body),
      });
    });
    await use(page);
  },
});

test.describe('world', () => {
  test('zooms from the whole world into a live room without clicks', async ({
    signedInPage: page,
    server,
    home,
    user,
  }) => {
    await expectDrawn(worldMap(page));
    const { lowest, levels } = await zoomIntoLiveRoom(page, home);
    // How far out the map goes depends on the world and the window: an 11 × 11 world fully
    // zoomed out is more than 24 px per room in a 1280 × 720 window (Sectors), a screeps.com
    // shard far less (World); the overview has its own test below. Every level from there in
    // is passed through.
    expect(['World', 'Sectors']).toContain(lowest);
    expect(levels).toEqual(LEVELS.slice(LEVELS.indexOf(lowest)));
    await expect(page).toHaveURL(routeOf(server, home, home.room));
    await expect(breadcrumb(page)).toContainText(home.room);
    await expect(status(page)).toContainText(/tick \d+/, { timeout: 15_000 });
    await expectLive(page, home.room);
    await expect(roomInfo(page, home.room)).toContainText(user.username);
  });

  test.describe('in a small window', () => {
    // A whole world fully zoomed out in a map about 350 px high: below 24 px per room (the
    // harness's 11 × 11 world too), the overview level.
    test.use({ viewport: { width: 1280, height: 420 } });

    test('zooms from the overview into a live room without clicks', async ({
      signedInPage: page,
      server,
      home,
    }) => {
      const { lowest, levels } = await zoomIntoLiveRoom(page, home);
      expect(lowest).toBe('World');
      expect(levels).toEqual([...LEVELS]);
      await expect(page).toHaveURL(routeOf(server, home, home.room));
    });
  });

  test('opens on the home room on a first visit', async ({ signedInPage: page, server, home }) => {
    // The signed-in page opens the server's world with no room in the route and nothing saved.
    await expect(page).toHaveURL(routeOf(server, home, home.room), { timeout: 30_000 });
    await expect(breadcrumb(page)).toContainText(home.room);
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
    await expectLive(page, home.room);
    await expect(worldMap(page)).toHaveAccessibleName(`${mapName(home)}, at ${home.room}`);
    await expect(page.getByRole('alert').filter({ hasText: /\S/ })).toHaveCount(0);
  });

  test('opens on the start room of a fresh account @openscreeps', async ({
    page,
    server,
    account,
  }) => {
    const room = await startRoom(server, account);
    await signIn(page, server, account);
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${room}$`), { timeout: 30_000 });
    await expect(breadcrumb(page)).toContainText(room);
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
    await expectLive(page, room);
    await expect(page.getByRole('alert').filter({ hasText: /\S/ })).toHaveCount(0);
  });

  test('shows the whole world from the button and the breadcrumb', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const bounds = await worldBounds(page, server, home);
    const corners = [
      [roomName(...bounds.min), 5, 5],
      [roomName(...bounds.max), 45, 45],
    ] as const;
    const world = breadcrumb(page).getByRole('button', { name: home.shard ?? 'World' });
    await expect(world).not.toHaveAttribute('aria-current', 'page');

    const expectWholeWorld = async () => {
      // The room leaves the route, the breadcrumb and the map's name; the world's corner rooms
      // are on the canvas once the camera has flown out.
      await expect(page).toHaveURL(routeOf(server, home));
      await expect(breadcrumb(page)).not.toContainText(home.room);
      await expect(world).toHaveAttribute('aria-current', 'page');
      await expect(worldMap(page)).toHaveAccessibleName(mapName(home));
      for (const [room, x, y] of corners) await tilePoint(page, room, x, y);
      await expect(level(page)).not.toHaveText(/^Rooms?$/);
    };

    await page.getByRole('group', { name: 'Zoom' }).getByRole('button', {
      name: 'Show the whole world',
    }).click();
    await expectWholeWorld();

    await search(page).fill(home.room);
    await search(page).press('Enter');
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
    await expect(world).not.toHaveAttribute('aria-current', 'page');
    await world.click();
    await expectWholeWorld();
  });

  test('flies to a room entered in the address bar, and back', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const other = towardsMiddle(home.room, await worldBounds(page, server, home)).room;
    const at = (room: string) => (camera: { centre: [number, number] }) => {
      const [x, y] = roomCentre(room);
      return Math.abs(camera.centre[0] - x) < 0.5 && Math.abs(camera.centre[1] - y) < 0.5;
    };

    await page.goto(`/#/${server.slug}/${shardKey(home)}/${other.toLowerCase()}`);
    await expect(page).toHaveURL(routeOf(server, home, other));
    await expect(breadcrumb(page)).toContainText(other);
    await restingCamera(page, server, home, at(other));
    await expect(level(page)).toHaveText('Room');
    await expectLive(page, other);

    await page.goBack();
    await expect(page).toHaveURL(routeOf(server, home, home.room));
    await expect(breadcrumb(page)).toContainText(home.room);
    await restingCamera(page, server, home, at(home.room));
    await expectLive(page, home.room);
  });

  test('hides the shard switcher on a single-world server', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    test.skip(home.shard !== null, 'the server has shards');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/`));
    await expect(worldMap(page)).toBeVisible();
    await expect(page.getByRole('combobox', { name: 'Shard' })).toHaveCount(0);
  });

  test('switches shards, each with its own room', async ({ signedInPage: page, server, home }) => {
    test.skip(home.shard === null, 'a single-world server has no shards to switch');
    await openRoom(page, home.path);
    const shard = page.getByRole('combobox', { name: 'Shard' });
    await expect(shard).toHaveValue(home.shard ?? '');
    const options = await shard.getByRole('option').allTextContents();
    const other = options.find((name) => name !== home.shard);
    if (!other) throw new Error('the shard switcher offers one shard');

    await shard.selectOption(other);
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/${other}/`));
    await expect(shard).toHaveValue(other);
    await expect(breadcrumb(page).getByRole('button', { name: other })).toBeVisible();
    await expect(worldMap(page)).toHaveAccessibleName(new RegExp(`^Map of ${other}`));
    await expect(level(page)).toHaveText(/\S/, { timeout: 30_000 });

    // Back on the home shard, its room comes back.
    await shard.selectOption(home.shard ?? '');
    await expect(page).toHaveURL(routeOf(server, home, home.room));
    await expect(breadcrumb(page)).toContainText(home.room);
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
  });

  test('pops a room out into a live window', async ({ signedInPage: page, server, home }) => {
    await openRoom(page, home.path);
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.getByRole('button', { name: 'Pop out room' }).click(),
    ]);
    await expect(popup).toHaveURL(routeOf(server, home, `${home.room}/window`));
    await expect(popup.getByRole('heading', { level: 1 })).toContainText(home.room);
    await expect(popup).toHaveTitle(new RegExp(home.room));
    await expect(popup.getByTestId('room-control')).toBeVisible();
    await expect(popup.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(status(popup)).toContainText(/tick \d+/);
    await expectDrawn(popup.getByTestId('room-control'));
    await popup.close();
  });

  test('inspects an object from the drawer @openscreeps', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openRoom(page, `/#/${server.slug}/-/${player.room}`);
    const control = page.getByTestId('room-control');
    await expectDrawn(control);
    // The room scope frames the 50 tiles with 1.5 tiles of margin on each side (top-down).
    const box = await control.boundingBox();
    if (!box) throw new Error('the room control has no box');
    const tile = box.height / 53;
    await page.mouse.click(
      box.x + box.width / 2 + (player.spawn.x + 0.5 - 25) * tile,
      box.y + box.height / 2 + (player.spawn.y + 0.5 - 25) * tile,
    );
    // The click selects the tile's topmost object in the drawer's Objects tab.
    const inspector = page.getByRole('region', { name: 'Inspector' });
    await expect(inspector.getByRole('heading', { name: 'Spawn1' })).toBeVisible();
    await expect(inspector).toContainText(`${player.spawn.x},${player.spawn.y}`);
    await expect(inspector).toContainText(player.username);
  });

  test.describe(() => {
    // The page caches the rooms in view itself first: the run's warm cache holds them already.
    test.use({ cache: 'cold' });

    test('restores route, camera and mode after a reload, terrain from the cache', async ({
      signedInPage: page,
      server,
      home,
    }) => {
      await openRoom(page, home.path);
      const perspective = page.getByRole('button', { name: 'Perspective view' });
      await perspective.click();
      await expect(perspective).toHaveAttribute('aria-pressed', 'true');
      // The terrain of the rooms in view goes to the cache as it arrives.
      await expect.poll(async () => (await cachedRooms(page)).length).toBeGreaterThan(0);
      const cached = await cachedRooms(page);
      // What the reloaded page starts from: the state the app saved when the page went away.
      await page.addInitScript(() => {
        window.e2eSavedState = localStorage.getItem('overseer.state');
      });
      const terrainBodies: string[] = [];
      page.on('request', (request) => {
        if (request.method() === 'POST' && request.url().endsWith('/api/game/rooms')) {
          terrainBodies.push(request.postData() ?? '');
        }
      });
      await page.reload();

      const saved = await page.evaluate(() => window.e2eSavedState ?? '{}');
      const place: SavedPlace = JSON.parse(saved).places[server.url];
      expect(place.mode).toBe('Perspective');
      expect(place.rooms[shardKey(home)]).toBe(home.room);
      const before = place.cameras[shardKey(home)];
      expect(before).toBeTruthy();
      await expect(page).toHaveURL(routeOf(server, home, home.room));
      await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
      await expect(perspective).toHaveAttribute('aria-pressed', 'true');
      await expect(breadcrumb(page)).toContainText(home.room);
      const after = await camera(page, server, home);
      expect(after.centre[0]).toBeCloseTo(before.centre[0], 1);
      expect(after.centre[1]).toBeCloseTo(before.centre[1], 1);
      expect(after.half_height).toBeCloseTo(before.half_height, 1);
      expect(await cachedRooms(page)).toEqual(expect.arrayContaining(cached));
      // Every room cached before the reload comes from IndexedDB: only the library's
      // world-bounds probe (the four rooms around the centre) may ask the server for those.
      const probe: Record<string, true> = { W0N0: true, E0N0: true, W0S0: true, E0S0: true };
      const refetched = terrainBodies
        .flatMap((body): string[] => JSON.parse(body).rooms ?? [])
        .filter((room) => cached.includes(room) && !probe[room]);
      expect(refetched).toEqual([]);
    });
  });

  rewritten.describe('when the server cannot measure the world', () => {
    rewritten.use({ worldSizeReply: { status: 500, body: { error: 'the world is resting' } } });

    rewritten('says so, and opens the world on Try again', async ({
      signedInPage: page,
      home,
      worldSize,
    }) => {
      const failure = page.getByRole('alert').filter({ hasText: 'The world could not be opened' });
      await expect(failure).toBeVisible({ timeout: 30_000 });
      await expect(level(page)).toHaveText('');

      // Trying again shows the world loading until the server answers.
      const answer = Promise.withResolvers<void>();
      worldSize.reply = null;
      worldSize.hold = answer.promise;
      await failure.getByRole('button', { name: 'Try again' }).click();
      await expect(failure).toHaveCount(0);
      await expect(
        page.getByRole('status').filter({ hasText: 'Loading the world…' }),
      ).toBeVisible();
      answer.resolve();
      await expect(page.getByRole('status').filter({ hasText: 'Loading the world…' })).toHaveCount(
        0,
        { timeout: 30_000 },
      );
      await expect(level(page)).toHaveText(/\S/, { timeout: 30_000 });
      await expect(breadcrumb(page)).toContainText(home.room);
    });
  });

  rewritten.describe('when the world has no rooms', () => {
    rewritten.use({ worldSizeReply: { status: 200, body: { ok: 1, width: 0, height: 0 } } });

    rewritten('says so', async ({ signedInPage: page }) => {
      await expect(
        page.getByRole('status').filter({ hasText: 'This world has no rooms yet.' }),
      ).toBeVisible({ timeout: 30_000 });
    });
  });
});
