// The world slice: server → shard → continuous world map → live room, the drawer's room
// control and inspector, popping a room out into its own window, and what survives a reload
// (route, camera, view mode, terrain cache).
//
// The canvas is drawn by the GPU, so assertions go through what the UI says about it: the
// status bar (level of detail, tick), the breadcrumb, the drawer and the route; plus one check
// that the canvas is not blank.
//
// Nothing here sleeps for the app: the map draws on animation frames, so a test waits for
// frames, for the status bar or for what the app stores.

import type { Locator, Page } from '@playwright/test';
import { expect, Session, signIn, test, type Account, type Server } from './fixtures';

declare global {
  interface Window {
    /** The levels of detail the status bar showed, recorded by `recordLevels`. */
    e2eLevels?: string[];
    /** `overseer.state` as the page found it when it loaded (the restore test). */
    e2eSavedState?: string | null;
  }
}

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The status bar's level of detail (its loading progress also names "Terrain"). */
const level = (page: Page) => status(page).getByTitle('Level of detail', { exact: true });

/** The world map canvas. */
const worldMap = (page: Page) => page.getByTestId('world-map');

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

/** The app's saved state (`overseer.state`): per server, the route's rooms, cameras and mode. */
type SavedPlace = {
  mode: string;
  rooms: Record<string, string>;
  cameras: Record<string, { centre: [number, number]; half_height: number }>;
};

/**
 * Saves the camera now, as leaving the page does (`pagehide`; the app otherwise saves it once
 * it rests for a second), and returns the saved place of `server`.
 */
async function savePlace(page: Page, server: Server): Promise<SavedPlace> {
  const saved = await page.evaluate(() => {
    window.dispatchEvent(new PageTransitionEvent('pagehide'));
    return localStorage.getItem('overseer.state');
  });
  return JSON.parse(saved ?? '{}').places[server.url];
}

/** The canvas is drawn (a blank canvas compresses to almost nothing). */
async function expectDrawn(canvas: Locator) {
  await expect
    .poll(async () => (await canvas.screenshot()).length, { timeout: 15_000 })
    .toBeGreaterThan(4_000);
}

/** The levels of detail the status bar names, from the most zoomed out. */
const LEVELS = ['Overview', 'Terrain', 'Minimap', 'Live rooms'] as const;
type Level = (typeof LEVELS)[number];

/** The level of detail the status bar shows. */
async function levelOf(page: Page): Promise<Level> {
  const text = (await level(page).textContent()) ?? '';
  const found = LEVELS.find((level) => level === text);
  if (!found) throw new Error(`the status bar shows no level of detail: "${text}"`);
  return found;
}

/**
 * Waits for two animation frames of the page: the map draws pending input in the next one
 * and the status bar shows that frame's level of detail right after it.
 */
async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(() => {
    const { promise, resolve } = Promise.withResolvers<void>();
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    return promise;
  });
}

/**
 * Records every level of detail the status bar shows from now on (a DOM observer, so no
 * level is missed however fast it passes); the returned function reads them, in order.
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
 * Opens the world on `room` (live), zooms out as far as the map goes with Escape (around the
 * centre, so the room stays there), then wheels back in over the centre until the room is
 * live: continuous zoom, no click. Returns the most zoomed-out level and every level the status
 * bar showed on the way in.
 */
async function zoomIntoLiveRoom(
  page: Page,
  slug: string,
  room: string,
): Promise<{ lowest: Level; levels: string[] }> {
  await page.goto(`/#/${slug}/-/${room}`);
  const map = worldMap(page);
  await expect(map).toBeVisible();
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await expect(level(page)).toHaveText('Live rooms', { timeout: 30_000 });
  await map.focus();
  // Escape zooms out 2.25× at once: twelve presses pass the zoom limit of any world (it is
  // clamped).
  for (let i = 0; i < 12; i++) {
    await page.keyboard.press('Escape');
  }
  await nextFrames(page);
  const lowest = await levelOf(page);
  const levels = await recordLevels(page);
  const box = await map.boundingBox();
  if (!box) throw new Error('the map has no box');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  // A wheel step zooms in 1.16× (eased over a few frames), a level spans 3× or more: one step
  // per drawn frame cannot skip a level, however slow the frames.
  for (let i = 0; i < 80 && (await levelOf(page)) !== 'Live rooms'; i++) {
    await page.mouse.wheel(0, -100);
    await nextFrames(page);
  }
  await expect(level(page)).toHaveText('Live rooms');
  return { lowest, levels: await levels() };
}

/** The account's start room, as the server names it (`GET /api/user/world-start-room`). */
async function startRoom(server: Server, account: Account): Promise<string> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const { room } = await session.call<{ room: string[] }>('api/user/world-start-room');
  if (!room[0]) throw new Error('the server names no start room');
  return room[0];
}

/** The world shows `room` live: route, breadcrumb, status bar and drawer. */
async function expectLiveRoom(page: Page, slug: string, room: string) {
  await expect(page).toHaveURL(new RegExp(`#/${slug}/-/${room}$`), { timeout: 30_000 });
  await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(room);
  await expect(level(page)).toHaveText('Live rooms', { timeout: 30_000 });
  const info = page.getByRole('region', { name: new RegExp(room) });
  await expect(info.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
    timeout: 15_000,
  });
}

test.describe('world', () => {
  test('zooms from the whole world into a live room without clicks', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await expectDrawn(worldMap(page));
    const { lowest, levels } = await zoomIntoLiveRoom(page, server.slug, player.room);
    // How far out the map goes depends on the world and the window: the harness's 11 × 11
    // world fully zoomed out is more than 24 px per room in a 1280 × 720 window (Terrain);
    // the overview has its own test below. Every level from there in is passed through.
    expect(['Overview', 'Terrain']).toContain(lowest);
    expect(levels).toEqual(LEVELS.slice(LEVELS.indexOf(lowest)));
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    const breadcrumb = page.getByRole('navigation', { name: 'Breadcrumb' });
    await expect(breadcrumb).toContainText(player.room);
    await expect(status(page)).toContainText(/tick \d+/, { timeout: 15_000 });
    const info = page.getByRole('region', { name: new RegExp(player.room) });
    await expect(info.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
      timeout: 15_000,
    });
    await expect(info).toContainText(player.username);
  });

  test.describe('in a small window', () => {
    // The whole 11 × 11 world fully zoomed out in a map about 350 px high: below 24 px per
    // room, the overview level.
    test.use({ viewport: { width: 1280, height: 420 } });

    test('zooms from the overview into a live room without clicks', async ({
      signedInPage: page,
      server,
      player,
    }) => {
      const { lowest, levels } = await zoomIntoLiveRoom(page, server.slug, player.room);
      expect(lowest).toBe('Overview');
      expect(levels).toEqual([...LEVELS]);
      await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    });
  });

  test('inspects an object from the drawer', async ({ signedInPage: page, server, player }) => {
    await zoomIntoLiveRoom(page, server.slug, player.room);
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
    const inspector = page.getByRole('region', { name: 'Selected tile' });
    await expect(inspector).toContainText(`Tile ${player.spawn.x},${player.spawn.y}`);
    await expect(inspector).toContainText('spawn');
    await expect(inspector).toContainText(`owner ${player.username}`);
  });

  test('pops a room out into a live window', async ({ signedInPage: page, server, player }) => {
    await zoomIntoLiveRoom(page, server.slug, player.room);
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.getByRole('button', { name: 'Pop out room' }).click(),
    ]);
    await expect(popup).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}/window$`));
    await expect(popup.getByRole('heading', { level: 1 })).toContainText(player.room);
    await expect(popup).toHaveTitle(new RegExp(player.room));
    await expect(popup.getByTestId('room-control')).toBeVisible();
    await expect(popup.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(status(popup)).toContainText(/tick \d+/);
    await expectDrawn(popup.getByTestId('room-control'));
    await popup.close();
  });

  test('restores route, camera and mode after a reload, terrain from the cache', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await zoomIntoLiveRoom(page, server.slug, player.room);
    const perspective = page.getByRole('button', { name: 'Perspective view' });
    await perspective.click();
    await expect(perspective).toHaveAttribute('aria-pressed', 'true');
    const cached = await cachedRooms(page);
    expect(cached.length).toBeGreaterThan(0);
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
    expect(place.rooms['-']).toBe(player.room);
    const camera = place.cameras['-'];
    expect(camera).toBeTruthy();
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    await expect(level(page)).toHaveText('Live rooms', { timeout: 30_000 });
    await expect(perspective).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(
      player.room,
    );
    const after = (await savePlace(page, server)).cameras['-'];
    expect(after.centre[0]).toBeCloseTo(camera.centre[0], 1);
    expect(after.centre[1]).toBeCloseTo(camera.centre[1], 1);
    expect(after.half_height).toBeCloseTo(camera.half_height, 1);
    expect(await cachedRooms(page)).toEqual(expect.arrayContaining(cached));
    // Every room cached before the reload comes from IndexedDB: only the library's
    // world-bounds probe (the four rooms around the centre) may ask the server for those.
    const probe: Record<string, true> = { W0N0: true, E0N0: true, W0S0: true, E0S0: true };
    const refetched = terrainBodies
      .flatMap((body): string[] => JSON.parse(body).rooms ?? [])
      .filter((room) => cached.includes(room) && !probe[room]);
    expect(refetched).toEqual([]);
  });

  test('keyboard pans and zooms, slash searches a room', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await page.goto(`/#/${server.slug}/-/${player.room}`);
    await expect(level(page)).toHaveText('Live rooms', { timeout: 30_000 });
    await worldMap(page).focus();
    await page.keyboard.press('Escape');
    await expect(level(page)).not.toHaveText('Live rooms');
    await page.keyboard.press('/');
    const search = page.getByRole('searchbox', { name: 'Go to room' });
    await expect(search).toBeFocused();
    await search.fill('nowhere');
    await search.press('Enter');
    await expect(page.getByRole('alert').filter({ hasText: 'not a room name' })).toBeVisible();
    await search.fill(player.room.toLowerCase());
    await search.press('Enter');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    await expect(level(page)).toHaveText('Live rooms', { timeout: 30_000 });
    await worldMap(page).focus();
    await page.keyboard.press('-');
    await page.keyboard.press('-');
    await expect(level(page)).not.toHaveText('Live rooms');
    await page.keyboard.press('+');
    await page.keyboard.press('+');
    await expect(level(page)).toHaveText('Live rooms');
    // Arrow keys pan towards the middle of the world (a room there always exists): the live
    // room under the centre becomes the room in focus.
    const towardsMiddle = player.room.startsWith('W') ? 'ArrowRight' : 'ArrowLeft';
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press(towardsMiddle);
    }
    await expect(page).not.toHaveURL(new RegExp(`/${player.room}$`));
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/[WE]\\d+[NS]\\d+$`));
  });

  test('opens on the start room of a fresh account', async ({ page, server, account }) => {
    const room = await startRoom(server, account);
    await signIn(page, server, account);
    await expectLiveRoom(page, server.slug, room);
    await expect(page.getByRole('alert')).toHaveCount(0);
  });

  test("opens on the room of a player's spawn", async ({ page, server, player }) => {
    expect(await startRoom(server, player)).toBe(player.room);
    await signIn(page, server, player);
    await expectLiveRoom(page, server.slug, player.room);
  });

  test('hides the shard switcher on a single-world server', async ({
    signedInPage: page,
    server,
  }) => {
    await page.goto(`/#/${server.slug}/`);
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/`));
    await expect(worldMap(page)).toBeVisible();
    await expect(page.getByRole('combobox', { name: 'Shard' })).toHaveCount(0);
  });
});
