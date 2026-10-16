// The right drawer: the Objects tab (grouped list, typed inspector, selection shared with the
// maps both ways) and what survives a reload (width and tab).

import type { Page } from '@playwright/test';
import { expect, test } from './fixtures';

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** Opens the world on `room` and waits until it is live in the drawer. */
async function openRoom(page: Page, slug: string, room: string) {
  await page.goto(`/#/${slug}/-/${room}`);
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  const info = page.getByRole('region', { name: new RegExp(room) });
  await expect(info.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
    timeout: 30_000,
  });
}

/** The selected tile the world map names in its label (`tile X,Y selected`). */
async function mapTile(page: Page): Promise<string | undefined> {
  const label = (await page.getByTestId('world-map').getAttribute('aria-label')) ?? '';
  return label.match(/tile (\d+,\d+) selected/)?.[1];
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
async function clickTile(page: Page, testid: string, room: string, x: number, y: number) {
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

/** The tile in an object row's name (`… at X,Y`). */
function rowTile(name: string): [number, number] {
  const match = name.match(/ at (\d+),(\d+)$/);
  if (!match) throw new Error(`no position in ${name}`);
  return [Number(match[1]), Number(match[2])];
}

test.describe('drawer', { tag: '@openscreeps' }, () => {
  test('lists the room objects and selects them both ways with the map', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openRoom(page, server.slug, player.room);
    const control = page.getByTestId('room-control');
    // Drawn (a blank canvas compresses to almost nothing): the viewer takes clicks.
    await expect
      .poll(async () => (await control.screenshot()).length, { timeout: 15_000 })
      .toBeGreaterThan(4_000);
    const box = await control.boundingBox();
    if (!box) throw new Error('the room control has no box');
    const tile = box.height / 53;
    await page.mouse.click(
      box.x + box.width / 2 + (player.spawn.x + 0.5 - 25) * tile,
      box.y + box.height / 2 + (player.spawn.y + 0.5 - 25) * tile,
    );
    await expect(page.getByRole('tab', { name: /Objects/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    const spawnRow = page.getByRole('button', {
      name: `Spawn1 at ${player.spawn.x},${player.spawn.y}`,
    });
    await expect(spawnRow).toHaveAttribute('aria-current', 'true');
    const inspector = page.getByRole('region', { name: 'Inspector' });
    await expect(inspector.getByRole('heading', { name: 'Spawn1' })).toBeVisible();
    await expect(inspector).toContainText('Spawn');
    await expect(inspector).toContainText(player.username);
    await expect(inspector).toContainText('Energy');
    await expect.poll(() => mapTile(page)).toBe(`${player.spawn.x},${player.spawn.y}`);

    // The list groups the objects by kind, the user's first.
    const list = page.getByRole('navigation', { name: 'Objects' });
    await expect(list.getByRole('button', { name: /^Creeps/ })).toBeVisible({ timeout: 30_000 });
    await expect(list.getByRole('button', { name: /^Spawns/ })).toBeVisible();
    await expect(list.getByRole('button', { name: /^Sources/ })).toBeVisible();

    // A creep picked in the list: typed fields, and the map selects its tile (it walks).
    const creep = list.getByRole('button', { name: /^c\d+ at \d+,\d+$/ }).first();
    await creep.click();
    await expect(creep).toHaveAttribute('aria-current', 'true');
    await expect(inspector).toContainText('Creep');
    for (const field of ['Hits', 'Ticks to live', 'Body']) {
      await expect(inspector.getByRole('term').filter({ hasText: field })).toBeVisible();
    }
    // Both read at once: the creep walks every tick, and two separate reads of a slow page
    // straddle a step more often than not.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const position = document
            .querySelector('[aria-label="Inspector"] [title="Position"]')
            ?.textContent;
          const map = document.querySelector('[data-testid="world-map"]')?.getAttribute('aria-label');
          return position != null && position === map?.match(/tile (\d+,\d+) selected/)?.[1];
        }),
      )
      .toBe(true);

    // The filter narrows the list.
    await list.page().getByRole('searchbox', { name: 'Filter objects' }).fill('spawn1');
    await expect(spawnRow).toBeVisible();
    await expect(list.getByRole('button', { name: /^c\d+ at/ })).toHaveCount(0);
  });

  test('selects one object after another by clicks on the map and the room preview', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openRoom(page, server.slug, player.room);
    // As a user would: frame the room with the search, then pick from the Objects tab.
    const search = page.getByRole('searchbox', { name: 'Go to room' });
    await search.fill(player.room);
    await search.press('Enter');
    await page.getByRole('tab', { name: /Objects/ }).click();
    const list = page.getByRole('navigation', { name: 'Objects' });
    const rows = [
      `Spawn1 at ${player.spawn.x},${player.spawn.y}`,
      ...(await Promise.all(
        [/^Controller.* at \d+,\d+$/, /^Source at \d+,\d+$/, /^Mineral .* at \d+,\d+$/].map(
          async (name) => {
            const row = list.getByRole('button', { name }).first();
            await expect(row).toBeVisible({ timeout: 30_000 });
            const label = await row.getAttribute('aria-label');
            if (!label) throw new Error(`the ${name} row has no name`);
            return label;
          },
        ),
      )),
    ];
    const inspector = page.getByRole('region', { name: 'Inspector' });
    /** Clicks the object of `row` on canvas `testid`: it becomes the selection. */
    const pick = async (testid: string, row: string) => {
      const [x, y] = rowTile(row);
      await clickTile(page, testid, player.room, x, y);
      await expect(list.getByRole('button', { name: row, exact: true })).toHaveAttribute(
        'aria-current',
        'true',
      );
      await expect(inspector.getByTitle('Position')).toHaveText(`${x},${y}`);
    };

    // The main map: each click selects its object (A, B, C, D, then A again).
    for (const row of [...rows, rows[0]]) {
      await pick('world-map', row);
      await expect.poll(() => mapTile(page)).toBe(rowTile(row).join(','));
    }

    // The drawer's room preview: a pick there opens the Objects tab, so come back each time.
    for (const row of [...rows.slice(1), rows[0]]) {
      await page.getByRole('tab', { name: 'Room', exact: true }).click();
      await pick('room-control', row);
      await expect(page.getByRole('tab', { name: /Objects/ })).toHaveAttribute(
        'aria-selected',
        'true',
      );
    }
  });

  test('keeps its width and tab across a reload, and collapses', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openRoom(page, server.slug, player.room);
    const separator = page.getByRole('separator', { name: 'Resize drawer' });
    const before = Number(await separator.getAttribute('aria-valuenow'));
    const box = await separator.boundingBox();
    if (!box) throw new Error('the separator has no box');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 - 120, box.y + box.height / 2, { steps: 6 });
    await page.mouse.up();
    await expect(separator).toHaveAttribute('aria-valuenow', String(before + 120));
    const drawer = page.locator('#drawer');
    const width = (await drawer.boundingBox())?.width ?? 0;
    expect(width).toBeCloseTo(before + 120, 0);
    await page.getByRole('tab', { name: /Console/ }).click();

    await page.reload();
    await expect(page.getByRole('tab', { name: /Console/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(separator).toHaveAttribute('aria-valuenow', String(before + 120));
    expect((await drawer.boundingBox())?.width ?? 0).toBeCloseTo(before + 120, 0);

    await page.getByRole('button', { name: 'Collapse drawer' }).click();
    await expect(drawer).toBeHidden();
    await page.reload();
    await expect(page.getByTestId('world-map')).toBeVisible();
    await expect(drawer).toBeHidden();
    await page.getByRole('button', { name: 'Drawer', exact: true }).click();
    await expect(page.getByRole('tab', { name: /Console/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });
});
