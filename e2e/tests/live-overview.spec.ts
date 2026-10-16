// A viewed room's overview follows the live room at once: the map's tooltip and the drawer
// read the world's one copy of each room's overview, and a room being viewed refreshes it from
// what the live room shows, without waiting for the overview's next scheduled fetch (minutes).
//
// The player's bot owns its room at level 1. While the map looks at another room, the player
// gives the controller up (`unclaim`); flying back to the room, the drawer and the tooltip say
// at once that nobody owns it, although the overview fetched before still named the player.

import type { Page } from '@playwright/test';
import { api, expect, Session, test, type Player, type Server } from './fixtures';

/** How long a viewed room may take to show its live overview (the live room's first update). */
const AT_ONCE = 20_000;

/** A canvas with the app's test hook `overseerTilePoint` (see `app/src/ui/surface.rs`). */
interface TilePointCanvas extends HTMLCanvasElement {
  /** CSS pixel `[x, y]` of the canvas where tile `(x, y)` of `room` is drawn, or `null`. */
  overseerTilePoint(room: string, x: number, y: number): [number, number] | null;
}

/** A room object as `GET /api/game/room-objects` lists it. */
interface RoomObject {
  _id: string;
  type: string;
  user?: string | null;
}

/** The controller of `room`. */
async function controller(server: Server, room: string): Promise<RoomObject> {
  const { body } = await api<{ objects: RoomObject[] }>(
    server.url,
    `api/game/room-objects?room=${room}`,
  );
  const found = body.objects.find((object) => object.type === 'controller');
  if (!found) throw new Error(`${room} has no controller`);
  return found;
}

/** A room four or more rooms away from `room`, inside the harness's world (11 × 11 rooms). */
function farFrom(room: string): string {
  const [, h, , v] = /^([WE])(\d+)([NS])(\d+)$/.exec(room) ?? [];
  return `${h === 'W' ? 'E' : 'W'}3${v === 'N' ? 'S' : 'N'}3`;
}

/** The drawer's facts of `room`. */
const info = (page: Page, room: string) => page.getByRole('region', { name: new RegExp(room) });

/** Frames `room` with the room search, as a user would, and waits until it is live. */
async function view(page: Page, room: string) {
  const search = page.getByRole('searchbox', { name: 'Go to room' });
  await search.fill(room);
  await search.press('Enter');
  await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(room);
  await expect(
    info(page, room).getByRole('status').filter({ hasText: 'Live' }),
  ).toBeVisible({ timeout: 30_000 });
}

/**
 * Moves the mouse over tile `(x, y)` of `room` on the world map once the camera stands still
 * (two reads of the tile's place 150 ms apart agree) with the tile on the canvas.
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

/** Gives up `player`'s controller (`unclaim`) and waits until the server did. */
async function unclaim(server: Server, player: Player) {
  const session = await Session.signIn(server.url, player.username, player.password);
  const { _id } = await controller(server, player.room);
  await session.call('api/game/add-object-intent', {
    _id,
    room: player.room,
    name: 'unclaim',
    intent: {},
  });
  await expect
    .poll(async () => (await controller(server, player.room)).user ?? null, { timeout: 30_000 })
    .toBeNull();
}

test.describe('live overview', () => {
  test('a viewed room’s tooltip and drawer follow the live room at once', async ({
    signedInPage: page,
    server,
    player,
  }) => {
    await page.goto(`/#/${server.slug}/-/${player.room}`);
    await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected', {
      timeout: 30_000,
    });
    await expect(
      info(page, player.room).getByRole('status').filter({ hasText: 'Live' }),
    ).toBeVisible({ timeout: 30_000 });
    // The overview names the player as the owner, in the drawer and the tooltip.
    const owner = info(page, player.room).locator('dt:text-is("Owner") + dd');
    await expect(owner).toHaveText(`${player.username} · level 1`, { timeout: 30_000 });
    await hoverTile(page, player.room, 25, 25);
    const tip = page.getByRole('tooltip');
    await expect(tip).toContainText(`${player.username} · RCL 1`);

    // Elsewhere on the map, the player gives the room up: the overview still names them.
    await view(page, farFrom(player.room));
    await unclaim(server, player);

    // Viewing the room again: the drawer and the tooltip agree with the live room at once.
    await view(page, player.room);
    await expect(owner).toHaveText('Nobody', { timeout: AT_ONCE });
    await hoverTile(page, player.room, 25, 25);
    await expect(tip).toBeVisible();
    await expect(tip).toContainText(player.room);
    await expect(tip).not.toContainText(player.username, { timeout: AT_ONCE });
  });
});
