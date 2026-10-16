// What the world map specs share: the map's landmarks (canvas, status bar, breadcrumb, search),
// its zoom levels, what the app saved of the camera, and room geometry.
//
// The map is drawn by the GPU, so the specs read it through what the UI says (the canvas's
// accessible name, the status bar, the breadcrumb, the route), the camera the app saves
// (`overseer.state`) and the canvas's test hook `overseerTilePoint` (see
// `app/src/ui/surface.rs`), which tells where a tile is drawn. Nothing sleeps for the app: the
// map draws on animation frames, so the helpers wait for frames or poll observable state.

import type { Locator, Page } from '@playwright/test';
import { expect, type Home, type Server } from './fixtures';

/** The status bar. */
export const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The status bar's zoom level. */
export const level = (page: Page) => status(page).getByTitle('Zoom level', { exact: true });

/** The status bar's room under the pointer. */
export const hovered = (page: Page) =>
  status(page).getByTitle('Room under the pointer', { exact: true });

/** The world map canvas. */
export const worldMap = (page: Page) => page.getByTestId('world-map');

/** The breadcrumb. */
export const breadcrumb = (page: Page) => page.getByRole('navigation', { name: 'Breadcrumb' });

/** The room search. */
export const search = (page: Page) => page.getByRole('searchbox', { name: 'Go to room' });

/** The drawer's facts of `room`. */
export const roomInfo = (page: Page, room: string) =>
  page.getByRole('region', { name: new RegExp(room) });

/** The route segment of `home`'s shard (`-` on single-world servers). */
export const shardKey = (home: Home) => home.shard ?? '-';

/** `#/<slug>/<shard>/` followed by `rest`, as a regular expression of the whole hash. */
export function routeOf(server: Server, home: Home, rest = ''): RegExp {
  return new RegExp(`#/${server.slug}/${shardKey(home)}/${rest}$`);
}

/** The zoom levels the status bar names, from the most zoomed out. */
export const LEVELS = ['World', 'Sectors', 'Rooms', 'Room'] as const;
export type Level = (typeof LEVELS)[number];

/** The zoom level the status bar shows. */
export async function levelOf(page: Page): Promise<Level> {
  const text = (await level(page).textContent()) ?? '';
  const found = LEVELS.find((level) => level === text);
  if (!found) throw new Error(`the status bar shows no zoom level: "${text}"`);
  return found;
}

/**
 * Waits for two animation frames of the page: the map draws pending input in the next one and
 * the status bar shows that frame's zoom level right after it.
 */
export async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(() => {
    const { promise, resolve } = Promise.withResolvers<void>();
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    return promise;
  });
}

/** The canvas is drawn (a blank canvas compresses to almost nothing). */
export async function expectDrawn(canvas: Locator) {
  await expect
    .poll(async () => (await canvas.screenshot()).length, { timeout: 15_000 })
    .toBeGreaterThan(4_000);
}

/**
 * Opens the world at `path` (`/#/<slug>/<shard>/<room>`) and waits until the map is connected
 * and shows the room at the live level.
 */
export async function openRoom(page: Page, path: string) {
  const room = path.split('/').at(-1) ?? '';
  await page.goto(path);
  await expect(worldMap(page)).toBeVisible();
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
  await expect(breadcrumb(page)).toContainText(room);
}

/** A camera as the app saves it. */
export interface Camera {
  centre: [number, number];
  half_height: number;
  mode: 'TopDown' | 'Perspective';
  yaw: number;
  pitch: number;
}

/** What the app saved of a server's world (`overseer.state`), by shard route segment. */
export interface SavedPlace {
  shard?: string;
  mode: string;
  rooms: Record<string, string>;
  cameras: Record<string, Camera>;
  bounds: Record<string, { min: string; max: string; at: number }>;
}

/**
 * Saves the camera now, as leaving the page does (`pagehide`; the app otherwise saves it once
 * it rests for a second), and returns the saved place of `server`.
 */
export async function savedPlace(page: Page, server: Server): Promise<SavedPlace> {
  const saved = await page.evaluate(() => {
    window.dispatchEvent(new PageTransitionEvent('pagehide'));
    return localStorage.getItem('overseer.state');
  });
  return JSON.parse(saved ?? '{}').places[server.url];
}

/** The camera of `home`'s shard now (see {@link savedPlace}). */
export async function camera(page: Page, server: Server, home: Home): Promise<Camera> {
  const found = (await savedPlace(page, server)).cameras[shardKey(home)];
  if (!found) throw new Error('the app saved no camera');
  return found;
}

/**
 * Polls the camera until two reads one frame apart agree (eased zooms and pans have come to
 * rest) and `ready` holds, and returns it.
 */
export async function restingCamera(
  page: Page,
  server: Server,
  home: Home,
  ready: (camera: Camera) => boolean = () => true,
): Promise<Camera> {
  const rest: { camera?: Camera } = {};
  await expect
    .poll(async () => {
      const before = await camera(page, server, home);
      await nextFrames(page);
      const after = await camera(page, server, home);
      const still =
        Math.abs(before.half_height - after.half_height) < 1e-3 &&
        Math.abs(before.centre[0] - after.centre[0]) < 1e-3 &&
        Math.abs(before.centre[1] - after.centre[1]) < 1e-3 &&
        Math.abs(before.yaw - after.yaw) < 1e-4;
      rest.camera = after;
      return still && ready(after);
    })
    .toBe(true);
  if (!rest.camera) throw new Error('no camera');
  return rest.camera;
}

/** The world bounds of `home`'s shard as the app saved them: corner rooms as coordinates. */
export async function worldBounds(
  page: Page,
  server: Server,
  home: Home,
): Promise<{ min: [number, number]; max: [number, number] }> {
  const bounds = (await savedPlace(page, server)).bounds[shardKey(home)];
  if (!bounds) throw new Error('the app saved no world bounds');
  return { min: roomXY(bounds.min), max: roomXY(bounds.max) };
}

/**
 * Room coordinates of a room name: `E<x>` is `x`, `W<x>` is `-x - 1`, `S<y>` is `y`, `N<y>` is
 * `-y - 1` (x east, y south).
 */
export function roomXY(name: string): [number, number] {
  const [, we, x, ns, y] = /^([WE])(\d+)([NS])(\d+)$/.exec(name) ?? [];
  if (!we || !ns) throw new Error(`not a room name: ${name}`);
  return [we === 'W' ? -Number(x) - 1 : Number(x), ns === 'N' ? -Number(y) - 1 : Number(y)];
}

/** The name of the room at coordinates `(x, y)` (see {@link roomXY}). */
export function roomName(x: number, y: number): string {
  return `${x < 0 ? `W${-x - 1}` : `E${x}`}${y < 0 ? `N${-y - 1}` : `S${y}`}`;
}

/** The centre of `room` in world tiles (room `(x, y)` spans tiles `[50x, 50x + 50)`). */
export function roomCentre(room: string): [number, number] {
  const [x, y] = roomXY(room);
  return [x * 50 + 25, y * 50 + 25];
}

/**
 * The room `steps` rooms from `room` towards the middle of the world (east or west, whichever
 * leads there; north or south when the room is in the middle column), and the arrow key that
 * pans that way.
 */
export function towardsMiddle(
  room: string,
  bounds: { min: [number, number]; max: [number, number] },
  steps = 1,
): { room: string; key: 'ArrowLeft' | 'ArrowRight' | 'ArrowUp' | 'ArrowDown' } {
  const [x, y] = roomXY(room);
  const middle = [(bounds.min[0] + bounds.max[0]) / 2, (bounds.min[1] + bounds.max[1]) / 2];
  if (Math.abs(middle[0] - x) >= steps) {
    const east = middle[0] > x;
    return { room: roomName(x + (east ? steps : -steps), y), key: east ? 'ArrowRight' : 'ArrowLeft' };
  }
  const south = middle[1] > y;
  return { room: roomName(x, y + (south ? steps : -steps)), key: south ? 'ArrowDown' : 'ArrowUp' };
}

/** A canvas with the app's test hook `overseerTilePoint` (see `app/src/ui/surface.rs`). */
interface TilePointCanvas extends HTMLCanvasElement {
  /** CSS pixel `[x, y]` of the canvas where tile `(x, y)` of `room` is drawn, or `null`. */
  overseerTilePoint(room: string, x: number, y: number): [number, number] | null;
}

/**
 * The page point (CSS pixels) where tile `(x, y)` of `room` is drawn on the world map, once
 * the camera stands still (two reads of the tile's place 150 ms apart agree) with the tile on
 * the canvas: before the camera sets off towards a room, it stands still elsewhere.
 */
export async function tilePoint(
  page: Page,
  room: string,
  x: number,
  y: number,
): Promise<{ x: number; y: number }> {
  const canvas = worldMap(page);
  const settled: { at?: { x: number; y: number } | null } = {};
  await expect
    .poll(async () => {
      settled.at = await canvas.evaluate(
        async (element, [room, x, y]) => {
          const hook = element as TilePointCanvas;
          const before = hook.overseerTilePoint?.(room, x, y);
          const wait = Promise.withResolvers<void>();
          setTimeout(wait.resolve, 150);
          await wait.promise;
          const after = hook.overseerTilePoint?.(room, x, y);
          if (!before || !after) return null;
          if (Math.hypot(before[0] - after[0], before[1] - after[1]) >= 0.5) return null;
          const box = element.getBoundingClientRect();
          if (after[0] < 0 || after[1] < 0 || after[0] > box.width || after[1] > box.height) {
            return null;
          }
          return { x: box.left + after[0], y: box.top + after[1] };
        },
        [room, x, y] as const,
      );
      return settled.at != null;
    })
    .toBe(true);
  if (!settled.at) throw new Error(`the map shows no tile ${x},${y} of ${room}`);
  return settled.at;
}

/** Where tile `(x, y)` of `room` is drawn right now (page CSS pixels), or `null`. */
export async function tilePointNow(
  page: Page,
  room: string,
  x: number,
  y: number,
): Promise<{ x: number; y: number } | null> {
  return worldMap(page).evaluate(
    (element, [room, x, y]) => {
      const point = (element as TilePointCanvas).overseerTilePoint?.(room, x, y);
      if (!point) return null;
      const box = element.getBoundingClientRect();
      return { x: box.left + point[0], y: box.top + point[1] };
    },
    [room, x, y] as const,
  );
}

/** Moves the mouse over tile `(x, y)` of `room` (see {@link tilePoint}). */
export async function hoverTile(page: Page, room: string, x: number, y: number) {
  const at = await tilePoint(page, room, x, y);
  await page.mouse.move(at.x, at.y, { steps: 3 });
}

/** The centre of the world map, in page CSS pixels. */
export async function mapCentre(page: Page): Promise<{ x: number; y: number }> {
  const box = await worldMap(page).boundingBox();
  if (!box) throw new Error('the map has no box');
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

/** CSS pixels per tile of a top-down camera on the world map. */
export async function pixelsPerTile(page: Page, camera: Camera): Promise<number> {
  const box = await worldMap(page).boundingBox();
  if (!box) throw new Error('the map has no box');
  return box.height / (2 * camera.half_height);
}
