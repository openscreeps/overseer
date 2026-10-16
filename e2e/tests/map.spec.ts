// The world map's room names before a click and its portals: hovering a room shows its tooltip
// (name, owner and level, portal destinations) and names it in the status bar; a portal room
// lists where its portals lead in the drawer, and "Go to" takes the map there.
//
// The harness's 11 × 11 world has no portals of its own (one sector centre, W5N5): the
// portal test links W5N5 with the crossroads W0N0 through the server's admin API, once per
// world (both are rooms the map scans for portals).

import type { Page } from '@playwright/test';
import { api, expect, test, type Server } from './fixtures';

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The portal pair of this spec. */
const PORTAL_ROOM = 'W5N5';
const PORTAL_TARGET = 'W0N0';

/** A canvas with the app's test hook `overseerTilePoint` (see `app/src/ui/surface.rs`). */
interface TilePointCanvas extends HTMLCanvasElement {
  /** CSS pixel `[x, y]` of the canvas where tile `(x, y)` of `room` is drawn, or `null`. */
  overseerTilePoint(room: string, x: number, y: number): [number, number] | null;
}

/** Whether `room` has portals leading to `target` (`GET /api/game/room-objects`). */
async function hasPortals(server: Server, room: string, target: string): Promise<boolean> {
  const { body } = await api<{ objects: { type: string; destination?: { room: string } }[] }>(
    server.url,
    `api/game/room-objects?room=${room}`,
  );
  return body.objects.some(
    (object) => object.type === 'portal' && object.destination?.room === target,
  );
}

/**
 * Links {@link PORTAL_ROOM} and {@link PORTAL_TARGET} with a portal pair (`portal-create` of the
 * admin API) unless they already are: the projects share the world.
 */
async function ensurePortals(server: Server): Promise<void> {
  if (await hasPortals(server, PORTAL_ROOM, PORTAL_TARGET)) return;
  const reply = await fetch(new URL('admin', server.url), {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${server.adminToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ action: 'portal-create', from: PORTAL_ROOM, to: PORTAL_TARGET }),
  });
  // A concurrent project may have linked them meanwhile (then there is no room for more).
  if (!reply.ok && !(await hasPortals(server, PORTAL_ROOM, PORTAL_TARGET))) {
    throw new Error(`portal-create: HTTP ${reply.status} ${await reply.text()}`);
  }
}

/** Opens the world on `room` and waits until the map is connected and framed on the room. */
async function openRoom(page: Page, slug: string, room: string) {
  await page.goto(`/#/${slug}/-/${room}`);
  await expect(page.getByTestId('world-map')).toBeVisible();
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await expect(status(page).getByTitle('Zoom level', { exact: true })).toHaveText('Room', {
    timeout: 30_000,
  });
}

/**
 * Moves the mouse over tile `(x, y)` of `room` on the world map once the camera stands still
 * (two reads of the tile's place 150 ms apart agree) with the tile on the canvas: before the
 * camera sets off towards a room, it stands still elsewhere.
 */
async function hoverTile(page: Page, room: string, x: number, y: number) {
  const canvas = page.getByTestId('world-map');
  const settled: { at?: { left: number; top: number } | null } = {};
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
          return { left: box.left + after[0], top: box.top + after[1] };
        },
        [room, x, y] as const,
      );
      return settled.at != null;
    })
    .toBe(true);
  if (!settled.at) throw new Error(`the map shows no tile ${x},${y} of ${room}`);
  await page.mouse.move(settled.at.left, settled.at.top, { steps: 3 });
}

test.describe('map', () => {
  test('names the hovered room in a tooltip and the status bar', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await openRoom(page, server.slug, player.room);
    await hoverTile(page, player.room, 25, 25);
    const tip = page.getByRole('tooltip');
    await expect(tip).toBeVisible();
    await expect(tip).toContainText(player.room);
    // The owner and level come with the room's overview.
    await expect(tip).toContainText(`${player.username} · RCL`, { timeout: 30_000 });
    const hovered = status(page).getByTitle('Room under the pointer', { exact: true });
    await expect(hovered).toContainText(player.room);

    // Dragging the map hides the tooltip; leaving the map too.
    const box = await page.getByTestId('world-map').boundingBox();
    if (!box) throw new Error('the map has no box');
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 40, box.y + box.height / 2 + 30, { steps: 4 });
    await expect(tip).toBeHidden();
    await page.mouse.up();
    await hoverTile(page, player.room, 25, 25);
    await expect(tip).toBeVisible();
    await status(page).hover();
    await expect(tip).toBeHidden();
    await expect(hovered).toHaveText('');
  });

  test('lists a room’s portals and goes through them', async ({ signedInPage: page, server }) => {
    await ensurePortals(server);
    await openRoom(page, server.slug, PORTAL_ROOM);
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
    await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(
      PORTAL_TARGET,
    );
    // The other side leads back.
    const back = page
      .getByRole('region', { name: new RegExp(PORTAL_TARGET) })
      .getByRole('list', { name: 'Portal destinations' });
    await expect(back).toContainText(`→ ${PORTAL_ROOM}`, { timeout: 60_000 });
  });
});
