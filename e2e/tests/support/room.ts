// What the room, drawer and console specs share: opening a room live, the drawer's tabs, the
// object list and inspector, and clicks on a tile of a canvas through the app's test hook.
// Everything goes through roles, names and text (no CSS classes or DOM structure).

import { expect, type Locator, type Page } from '@playwright/test';

/** The status bar. */
export const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The drawer (`aside`). */
export const drawer = (page: Page) => page.getByRole('complementary', { name: 'Drawer' });

/** The facts of `room` on the Room tab (or a room window): a region named by its heading (the
 * room's name, then its live state; `W1N1` is not `W1N10`). */
export const roomInfo = (page: Page, room: string) =>
  page.getByRole('region', { name: new RegExp(`^${room}(?!\\d)`) });

/** A drawer tab. */
export type TabName = 'Room' | 'Objects' | 'Console' | 'Memory';

/** The drawer tab `name` (its name ends with a count on Objects and Console). */
export const tab = (page: Page, name: TabName) =>
  page.getByRole('tab', { name: new RegExp(`^${name}(\\s*[\\d+]+)?$`) });

/** Selects the drawer tab `name`. */
export async function selectTab(page: Page, name: TabName) {
  await tab(page, name).click();
  await expect(tab(page, name)).toHaveAttribute('aria-selected', 'true');
}

/** The object list of the Objects tab. */
export const objectList = (page: Page) => page.getByRole('navigation', { name: 'Objects' });

/** The inspector of the Objects tab. */
export const inspector = (page: Page) => page.getByRole('region', { name: 'Inspector' });

/** The object rows of `list` (`<name> at X,Y`). */
export const rows = (list: Locator) => list.getByRole('button', { name: / at \d+,\d+$/ });

/** The tile the inspector shows the selected object on (`X,Y` in its header). */
export const inspectedTile = (panel: Locator) => panel.getByText(/^\d{1,2},\d{1,2}$/);

/** The console output. */
export const consoleOutput = (page: Page) => page.getByRole('log', { name: 'Console output' });

/**
 * Opens the world at `path` (`/#/<slug>/<shard or ->/<room>`) on the drawer's Room tab and
 * waits until `room` is live there.
 */
export async function openRoom(page: Page, path: string, room: string) {
  await page.goto(path);
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await selectTab(page, 'Room');
  await expect(roomInfoStatus(page, room)).toHaveText('Live', { timeout: 30_000 });
}

/** The live state of `room` its facts show (`Live`, `Connecting…`, …), on the Room tab. */
export function roomInfoStatus(page: Page, room: string): Locator {
  return roomInfo(page, room).getByRole('status').first();
}

/** The selected tile the world map names in its label (`tile X,Y selected`). */
export async function mapTile(page: Page): Promise<string | undefined> {
  const label = (await page.getByTestId('world-map').getAttribute('aria-label')) ?? '';
  return label.match(/tile (\d+,\d+) selected/)?.[1];
}

/** The tile in an object row's name (`… at X,Y`). */
export function rowTile(name: string): [number, number] {
  const match = name.match(/ at (\d+),(\d+)$/);
  if (!match) throw new Error(`no position in ${name}`);
  return [Number(match[1]), Number(match[2])];
}

/** The name of the first row of `list` named `name`, once it shows. */
export async function rowName(list: Locator, name: RegExp): Promise<string> {
  const row = list.getByRole('button', { name }).first();
  await expect(row).toBeVisible({ timeout: 30_000 });
  const label = await row.getAttribute('aria-label');
  if (!label) throw new Error(`the ${name} row has no name`);
  return label;
}

/** The canvas is drawn (a blank canvas compresses to almost nothing). */
export async function expectDrawn(canvas: Locator) {
  await expect
    .poll(async () => (await canvas.screenshot()).length, { timeout: 15_000 })
    .toBeGreaterThan(4_000);
}

/** A canvas with the app's test hook `overseerTilePoint` (see `app/src/ui/surface.rs`). */
interface TilePointCanvas extends HTMLCanvasElement {
  /** CSS pixel `[x, y]` of the canvas where tile `(x, y)` of `room` is drawn, or `null`. */
  overseerTilePoint(room: string, x: number, y: number): [number, number] | null;
}

/**
 * Clicks tile `(x, y)` of `room` on the canvas `testid`, once the camera stands still (two
 * reads of the tile's place 150 ms apart agree) with the tile on the canvas (before the camera
 * sets off towards a room, it stands still elsewhere), checking that the canvas takes the
 * click. One round trip per try: on a slow page each one waits for its busy main thread.
 */
export async function clickTile(page: Page, testid: string, room: string, x: number, y: number) {
  const canvas = page.getByTestId(testid);
  const settled: { at?: { left: number; top: number; hit?: string | null } | null } = {};
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
          const [left, top] = [box.left + after[0], box.top + after[1]];
          const hit = document.elementFromPoint(left, top)?.getAttribute('data-testid');
          return { left, top, hit };
        },
        [room, x, y] as const,
      );
      return settled.at != null;
    })
    .toBe(true);
  if (!settled.at) throw new Error(`${testid} shows no tile ${x},${y}`);
  expect(settled.at.hit, `tile ${x},${y} is on the canvas, uncovered`).toBe(testid);
  await page.mouse.click(settled.at.left, settled.at.top);
}
