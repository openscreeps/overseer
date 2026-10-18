// A viewed room's overview follows the live room at once: the map's tooltip and the drawer
// read the world's one copy of each room's overview, and a room being viewed refreshes it from
// what the live room shows, without waiting for the overview's next scheduled fetch (minutes).
//
// The player's bot owns its room at level 1. While the map looks at another room, the player
// gives the controller up (`unclaim`); flying back to the room, the drawer and the tooltip say
// at once that nobody owns it, although the overview fetched before still named the player.

import type { Page } from '@playwright/test';
import { api, expect, Session, test, type Player, type Server, type Target } from './fixtures';
import { breadcrumb, hoverTile, openRoom, roomInfo, search } from './world-map';

/** How long a viewed room may take to show its live overview (the live room's first update). */
const AT_ONCE = 20_000;

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

/**
 * A room four or more rooms away from `room` (a player's room), inside the harness's world:
 * OpenScreeps' 11 × 11 rooms centred on the zero lines (W5N5 to E5S5), or the community world
 * W10N10 to W0N0 (players start within W9N9 to W1N1).
 */
function farFrom(room: string, target: Target): string {
  const [, h, x, v, y] = /^([WE])(\d+)([NS])(\d+)$/.exec(room) ?? [];
  if (target === 'community') {
    const away = (n: number) => (n <= 5 ? n + 4 : n - 4);
    return `W${away(Number(x))}N${away(Number(y))}`;
  }
  return `${h === 'W' ? 'E' : 'W'}3${v === 'N' ? 'S' : 'N'}3`;
}

/** Frames `room` with the room search, as a user would, and waits until it is live. */
async function view(page: Page, room: string) {
  await search(page).fill(room);
  await search(page).press('Enter');
  await expect(breadcrumb(page)).toContainText(room);
  await expect(roomInfo(page, room).getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
    timeout: 30_000,
  });
}

/**
 * Gives up `player`'s controller (`unclaim`) and waits until the server did.
 *
 * The stock community engine can drop a manual intent: its processor reads a room's intents,
 * processes the room, then deletes every intent of the room. An intent that arrives between
 * the read and the delete is lost (`@screeps/engine` `processor.js`, `getRoomIntents` …
 * `clearRoomIntents`). The player's room is processed every tick, so the intent is sent again
 * while the controller stays claimed. A second `unclaim` of a free controller does nothing.
 */
async function unclaim(server: Server, player: Player) {
  const session = await Session.signIn(server.url, player.username, player.password);
  const { _id } = await controller(server, player.room);
  await expect(async () => {
    await session.call('api/game/add-object-intent', {
      _id,
      room: player.room,
      name: 'unclaim',
      intent: {},
    });
    // 15 ticks of the harness's worlds (200 ms each).
    await expect
      .poll(async () => (await controller(server, player.room)).user ?? null, { timeout: 3_000 })
      .toBeNull();
  }).toPass({ timeout: 30_000 });
}

test.describe('live overview', { tag: ['@openscreeps', '@transport'] }, () => {
  test('a viewed room’s tooltip and drawer follow the live room at once', async ({
    signedInPage: page,
    server,
    player,
    target,
  }) => {
    await openRoom(page, `/#/${server.slug}/-/${player.room}`);
    const info = roomInfo(page, player.room);
    await expect(info.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
      timeout: 30_000,
    });
    // The overview names the player as the owner, in the drawer and the tooltip.
    await expect(
      info.getByRole('definition').filter({ hasText: `${player.username} · level 1` }),
    ).toBeVisible({ timeout: 30_000 });
    await hoverTile(page, player.room, 25, 25);
    const tip = page.getByRole('tooltip');
    await expect(tip).toContainText(`${player.username} · RCL 1`);

    // Elsewhere on the map, the player gives the room up: the overview still names them.
    await view(page, farFrom(player.room, target));
    await unclaim(server, player);

    // Viewing the room again: the drawer and the tooltip agree with the live room at once.
    await view(page, player.room);
    await expect(info.getByRole('definition').filter({ hasText: /^Nobody$/ })).toBeVisible({
      timeout: AT_ONCE,
    });
    await hoverTile(page, player.room, 25, 25);
    await expect(tip).toBeVisible();
    await expect(tip).toContainText(player.room);
    await expect(tip).not.toContainText(player.username, { timeout: AT_ONCE });
  });
});
