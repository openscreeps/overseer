// The world slice: server → shard → continuous world map → live room, the drawer's room
// control and inspector, popping a room out into its own window, and what survives a reload
// (route, camera, view mode, terrain cache).
//
// The canvas is drawn by the GPU, so assertions go through what the UI says about it: the
// status bar (level of detail, tick), the breadcrumb, the drawer and the route; plus one check
// that the canvas is not blank.

import type { Locator, Page } from '@playwright/test';
import { expect, Session, signIn, test, type Account, type Server } from './fixtures';

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The world map canvas. */
const worldMap = (page: Page) => page.getByTestId('world-map');

/** Rooms in the terrain cache (IndexedDB `overseer-terrain`). */
async function cachedRooms(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      new Promise<number>((resolve, reject) => {
        const open = indexedDB.open('overseer-terrain');
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const count = open.result.transaction('terrain').objectStore('terrain').count();
          count.onsuccess = () => resolve(count.result);
          count.onerror = () => reject(count.error);
        };
      }),
  );
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

/**
 * The level of detail the status bar shows once it stopped changing: the same for a second
 * (the camera eases zoom and glides after pans for a few hundred milliseconds).
 */
async function settledLevel(page: Page): Promise<Level> {
  const read = async () => {
    const text = (await status(page).textContent()) ?? '';
    return LEVELS.find((level) => text.includes(level));
  };
  let previous = await read();
  let stable = 0;
  for (let i = 0; i < 60; i++) {
    await page.waitForTimeout(150);
    const current = await read();
    stable = current !== undefined && current === previous ? stable + 1 : 0;
    if (stable >= 7) return current as Level;
    previous = current;
  }
  throw new Error(`the level of detail does not settle (last: ${previous})`);
}

/**
 * Opens the world on `room` (live), zooms out as far as the map goes with Escape (around the
 * centre, so the room stays there), then wheels back in over the centre until the room is
 * live: continuous zoom, no click. Returns the most zoomed-out level and the levels the status
 * bar went through on the way in.
 */
async function zoomIntoLiveRoom(
  page: Page,
  slug: string,
  room: string,
): Promise<{ lowest: Level; levels: Level[] }> {
  await page.goto(`/#/${slug}/-/${room}`);
  const map = worldMap(page);
  await expect(map).toBeVisible();
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await expect(status(page)).toContainText('Live rooms', { timeout: 30_000 });
  await map.focus();
  // Escape zooms out 2.25×: twelve presses pass the zoom limit of any world (it is clamped).
  for (let i = 0; i < 12; i++) {
    await page.keyboard.press('Escape');
  }
  const lowest = await settledLevel(page);
  const box = await map.boundingBox();
  if (!box) throw new Error('the map has no box');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  const levels: Level[] = [lowest];
  for (let i = 0; i < 80; i++) {
    const text = (await status(page).textContent()) ?? '';
    for (const level of LEVELS) {
      if (text.includes(level) && levels.at(-1) !== level) levels.push(level);
    }
    if (levels.at(-1) === 'Live rooms') break;
    await page.mouse.wheel(0, -100);
    await page.waitForTimeout(80);
  }
  return { lowest, levels };
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
  await expect(status(page)).toContainText('Live rooms', { timeout: 30_000 });
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
    await page.getByRole('button', { name: 'Perspective view' }).click();
    // The camera is saved once it rests for a second.
    await page.waitForTimeout(1_500);
    const before = await page.evaluate(() => JSON.parse(localStorage.getItem('overseer.state') ?? '{}'));
    const place = before.places[server.url];
    expect(place.mode).toBe('Perspective');
    expect(place.rooms['-']).toBe(player.room);
    const camera = place.cameras['-'];
    expect(camera).toBeTruthy();
    const cached = await cachedRooms(page);
    expect(cached).toBeGreaterThan(0);

    const terrainBodies: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && request.url().endsWith('/api/game/rooms')) {
        terrainBodies.push(request.postData() ?? '');
      }
    });
    await page.reload();

    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    await expect(status(page)).toContainText('Live rooms', { timeout: 30_000 });
    await expect(page.getByRole('button', { name: 'Perspective view' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(
      player.room,
    );
    await page.waitForTimeout(1_500);
    const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('overseer.state') ?? '{}'));
    const after = saved.places[server.url].cameras['-'];
    expect(after.centre[0]).toBeCloseTo(camera.centre[0], 1);
    expect(after.centre[1]).toBeCloseTo(camera.centre[1], 1);
    expect(after.half_height).toBeCloseTo(camera.half_height, 1);
    expect(await cachedRooms(page)).toBe(cached);
    // Only the library's world-bounds probe (the four rooms around the centre) may ask for
    // terrain: every cached room comes from IndexedDB.
    const probe: Record<string, true> = { W0N0: true, E0N0: true, W0S0: true, E0S0: true };
    for (const body of terrainBodies) {
      const rooms: string[] = JSON.parse(body).rooms ?? [];
      expect(rooms.filter((room) => !probe[room])).toEqual([]);
    }
  });

  test('keyboard pans and zooms, slash searches a room', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await page.goto(`/#/${server.slug}/-/${player.room}`);
    await expect(status(page)).toContainText('Live rooms', { timeout: 30_000 });
    await worldMap(page).focus();
    await page.keyboard.press('Escape');
    await expect(status(page)).not.toContainText('Live rooms');
    await page.keyboard.press('/');
    const search = page.getByRole('searchbox', { name: 'Go to room' });
    await expect(search).toBeFocused();
    await search.fill('nowhere');
    await search.press('Enter');
    await expect(page.getByRole('alert').filter({ hasText: 'not a room name' })).toBeVisible();
    await search.fill(player.room.toLowerCase());
    await search.press('Enter');
    await expect(page).toHaveURL(new RegExp(`#/${server.slug}/-/${player.room}$`));
    await expect(status(page)).toContainText('Live rooms', { timeout: 30_000 });
    await worldMap(page).focus();
    await page.keyboard.press('-');
    await page.keyboard.press('-');
    await expect(status(page)).not.toContainText('Live rooms');
    await page.keyboard.press('+');
    await page.keyboard.press('+');
    await expect(status(page)).toContainText('Live rooms');
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
