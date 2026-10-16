// Moving around the world map: zoom buttons and keys, keyboard and pointer panning, the wheel
// zooming around the pointer, clicks selecting a room (and close up a tile), the room in focus
// following the map's centre at the live level, and the 2D/3D view modes.
//
// Read-only: every test runs on any world around the signed-in account's `home` room. The
// camera is read from what the app saves (`overseer.state`), tiles' places from the canvas's
// test hook (see `world-map.ts`).

import type { Page } from '@playwright/test';
import { expect, test, type Home, type Server } from './fixtures';
import {
  breadcrumb,
  level,
  mapCentre,
  openRoom,
  pixelsPerTile,
  restingCamera,
  roomCentre,
  roomInfo,
  routeOf,
  savedPlace,
  search,
  tilePoint,
  tilePointNow,
  towardsMiddle,
  worldBounds,
  worldMap,
  type Camera,
} from './world-map';

/** Whether `a` and `b` are within `tolerance` (relative) of each other. */
const near = (a: number, b: number, tolerance = 0.01) =>
  Math.abs(a - b) <= tolerance * Math.max(Math.abs(a), Math.abs(b), 1e-6);

/** Waits for the camera to rest at `half_height` around the same centre as `from`. */
async function expectZoomed(page: Page, server: Server, home: Home, from: Camera, half: number) {
  await restingCamera(
    page,
    server,
    home,
    (camera) =>
      near(camera.half_height, half) &&
      Math.abs(camera.centre[0] - from.centre[0]) < 0.05 &&
      Math.abs(camera.centre[1] - from.centre[1]) < 0.05,
  );
}

/** Waits for the camera to rest at `centre` (world tiles, within 0.05 tiles). */
async function expectCentre(page: Page, server: Server, home: Home, centre: [number, number]) {
  await restingCamera(
    page,
    server,
    home,
    (camera) =>
      Math.abs(camera.centre[0] - centre[0]) < 0.05 && Math.abs(camera.centre[1] - centre[1]) < 0.05,
  );
}

/** The accessible name of the world map at `room`. */
const mapAt = (home: Home, room: string) =>
  `${home.shard ? `Map of ${home.shard}` : 'World map'}, at ${room}`;

test.describe('map controls', () => {
  test('zoom buttons and keys zoom around the centre', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const start = await restingCamera(page, server, home);
    const zoom = page.getByRole('group', { name: 'Zoom' });
    await expect(zoom.getByRole('button', { name: 'Zoom in' })).toHaveAttribute(
      'aria-keyshortcuts',
      '+',
    );

    // The buttons zoom 1.5× at a time.
    await zoom.getByRole('button', { name: 'Zoom out' }).click();
    await expectZoomed(page, server, home, start, start.half_height * 1.5);
    await zoom.getByRole('button', { name: 'Zoom in' }).click();
    await expectZoomed(page, server, home, start, start.half_height);

    // So do + (or =) and - (or _); Escape zooms out 2.25×.
    await worldMap(page).focus();
    await page.keyboard.press('-');
    await expectZoomed(page, server, home, start, start.half_height * 1.5);
    await page.keyboard.press('=');
    await expectZoomed(page, server, home, start, start.half_height);
    await page.keyboard.press('+');
    await expectZoomed(page, server, home, start, start.half_height / 1.5);
    await page.keyboard.press('_');
    await expectZoomed(page, server, home, start, start.half_height);
    await page.keyboard.press('Escape');
    await expectZoomed(page, server, home, start, start.half_height * 2.25);
  });

  test('arrow keys and WASD pan the map, not while typing or with modifiers', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const start = await restingCamera(page, server, home);
    // A key pans 80 CSS pixels.
    const step = 80 / (await pixelsPerTile(page, start));
    const [x, y] = start.centre;
    await worldMap(page).focus();

    const pans: [string, number, number][] = [
      ['ArrowRight', x + step, y],
      ['a', x, y],
      ['ArrowDown', x, y + step],
      ['w', x, y],
      ['D', x + step, y],
      ['ArrowLeft', x, y],
      ['s', x, y + step],
      ['ArrowUp', x, y],
    ];
    for (const [key, ex, ey] of pans) {
      await page.keyboard.press(key);
      await expectCentre(page, server, home, [ex, ey]);
    }

    // With a modifier the keys are the browser's: the map stays (the next plain key pans from
    // where it was).
    await page.keyboard.press('Control+ArrowRight');
    await page.keyboard.press('Alt+d');
    await page.keyboard.press('ArrowDown');
    await expectCentre(page, server, home, [x, y + step]);

    // Typed into the search, they are text.
    await search(page).focus();
    await page.keyboard.type('dddd');
    await expect(search(page)).toHaveValue('dddd');
    await page.keyboard.press('Escape');
    await expect(worldMap(page)).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await expectCentre(page, server, home, [x, y]);
  });

  test('dragging pans the map', async ({ signedInPage: page, server, home }) => {
    await openRoom(page, home.path);
    const start = await restingCamera(page, server, home);
    const perTile = await pixelsPerTile(page, start);
    const centre = await mapCentre(page);
    await page.mouse.move(centre.x, centre.y);
    await page.mouse.down();
    await page.mouse.move(centre.x - 200, centre.y - 120, { steps: 10 });
    await page.mouse.up();
    // The ground follows the pointer: the camera moves the other way, a released drag
    // gliding on a little.
    const end = await restingCamera(page, server, home);
    const moved = [(end.centre[0] - start.centre[0]) * perTile, (end.centre[1] - start.centre[1]) * perTile];
    expect(moved[0]).toBeGreaterThan(200 * 0.9);
    expect(moved[0]).toBeLessThan(200 * 1.6);
    expect(moved[1]).toBeGreaterThan(120 * 0.9);
    expect(moved[1]).toBeLessThan(120 * 1.6);
    expect(end.half_height).toBeCloseTo(start.half_height, 3);
  });

  test('the wheel zooms around the pointer', async ({ signedInPage: page, server, home }) => {
    await openRoom(page, home.path);
    const start = await restingCamera(page, server, home);
    // Off the centre, so that zooming around the centre would move the tile.
    const point = await tilePoint(page, home.room, 12, 34);
    await page.mouse.move(point.x, point.y);

    for (let i = 0; i < 3; i++) await page.mouse.wheel(0, -100);
    const nearer = await restingCamera(page, server, home);
    expect(nearer.half_height).toBeLessThan(start.half_height / 1.3);
    const after = await tilePointNow(page, home.room, 12, 34);
    expect(after).not.toBeNull();
    expect(Math.hypot((after?.x ?? 0) - point.x, (after?.y ?? 0) - point.y)).toBeLessThan(2);

    for (let i = 0; i < 3; i++) await page.mouse.wheel(0, 100);
    const back = await restingCamera(page, server, home);
    expect(back.half_height).toBeGreaterThan(nearer.half_height * 1.3);
    const again = await tilePointNow(page, home.room, 12, 34);
    expect(Math.hypot((again?.x ?? 0) - point.x, (again?.y ?? 0) - point.y)).toBeLessThan(2);
  });

  test('a click selects a room, and close up a tile', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const other = towardsMiddle(home.room, await worldBounds(page, server, home)).room;
    // Zoomed out to rooms of about a hundred pixels, where a click picks a room.
    await worldMap(page).focus();
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await expect(level(page)).not.toHaveText('Room');
    const before = await restingCamera(page, server, home);

    const room = await tilePoint(page, other, 25, 25);
    await page.mouse.click(room.x, room.y);
    await expect(page).toHaveURL(routeOf(server, home, other));
    await expect(breadcrumb(page)).toContainText(other);
    await expect(worldMap(page)).toHaveAccessibleName(mapAt(home, other));
    // Selecting a room does not move the camera.
    const after = await restingCamera(page, server, home);
    expect(after.centre).toEqual(before.centre);
    expect(after.half_height).toBe(before.half_height);

    // At the live level a click selects the tile under it.
    await search(page).fill(other);
    await search(page).press('Enter');
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
    const tile = await tilePoint(page, other, 20, 30);
    await page.mouse.click(tile.x, tile.y);
    await expect(worldMap(page)).toHaveAccessibleName(`${mapAt(home, other)}, tile 20,30 selected`);
  });

  test('at the live level the room under the centre comes into focus', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const { room: other, key } = towardsMiddle(home.room, await worldBounds(page, server, home));
    const start = await restingCamera(page, server, home);
    // From the room's centre, three quarters of a room's width on: into the next room.
    const presses = Math.ceil((0.75 * 50 * (await pixelsPerTile(page, start))) / 80);
    await worldMap(page).focus();
    for (let i = 0; i < presses; i++) await page.keyboard.press(key);
    await expect(page).toHaveURL(routeOf(server, home, other), { timeout: 30_000 });
    await expect(breadcrumb(page)).toContainText(other);
    await expect(level(page)).toHaveText('Room');
    await expect(
      roomInfo(page, other).getByRole('status').filter({ hasText: 'Live' }),
    ).toBeVisible({ timeout: 30_000 });
    // Not framed: the camera stays where the keys took it, short of the room's centre.
    const [cx, cy] = roomCentre(other);
    const camera = await restingCamera(page, server, home);
    expect(Math.hypot(camera.centre[0] - cx, camera.centre[1] - cy)).toBeGreaterThan(5);
  });

  test('switches between top-down and perspective; dragging in 3D orbits', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const modes = page.getByRole('group', { name: 'View mode' });
    const topDown = modes.getByRole('button', { name: 'Top-down view' });
    const perspective = modes.getByRole('button', { name: 'Perspective view' });
    await expect(topDown).toHaveAttribute('aria-pressed', 'true');
    await expect(perspective).toHaveAttribute('aria-pressed', 'false');

    await perspective.click();
    await expect(perspective).toHaveAttribute('aria-pressed', 'true');
    await expect(topDown).toHaveAttribute('aria-pressed', 'false');
    const orbiting = await restingCamera(page, server, home, (camera) => camera.mode === 'Perspective');
    expect((await savedPlace(page, server)).mode).toBe('Perspective');

    // A primary drag orbits around the point looked at.
    const centre = await mapCentre(page);
    await page.mouse.move(centre.x, centre.y);
    await page.mouse.down();
    await page.mouse.move(centre.x + 150, centre.y, { steps: 10 });
    await page.mouse.up();
    const orbited = await restingCamera(page, server, home);
    expect(Math.abs(orbited.yaw - orbiting.yaw)).toBeGreaterThan(0.1);
    expect(orbited.centre[0]).toBeCloseTo(orbiting.centre[0], 1);
    expect(orbited.centre[1]).toBeCloseTo(orbiting.centre[1], 1);

    await topDown.click();
    await expect(topDown).toHaveAttribute('aria-pressed', 'true');
    await expect(perspective).toHaveAttribute('aria-pressed', 'false');
    await restingCamera(page, server, home, (camera) => camera.mode === 'TopDown');
    expect((await savedPlace(page, server)).mode).toBe('TopDown');
  });
});
