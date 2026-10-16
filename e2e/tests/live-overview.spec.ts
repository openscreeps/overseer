// A viewed room's overview follows the live room at once: the map's tooltip and the drawer
// read the world's one copy of each room's overview, and a room being viewed refreshes it from
// what the live room shows, without waiting for the overview's next scheduled fetch (minutes).
//
// The player's bot owns its room at level 1. While the map looks at another room, the player
// gives the controller up (`unclaim`); flying back to the room, the drawer and the tooltip say
// at once that nobody owns it, although the overview fetched before still named the player.

import type { Page } from '@playwright/test';
import { api, expect, Session, test, type Player, type Server } from './fixtures';
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

/** A room four or more rooms away from `room`, inside the harness's world (11 × 11 rooms). */
function farFrom(room: string): string {
  const [, h, , v] = /^([WE])(\d+)([NS])(\d+)$/.exec(room) ?? [];
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

test.describe('live overview', { tag: '@openscreeps' }, () => {
  test('a viewed room’s tooltip and drawer follow the live room at once', async ({
    signedInPage: page,
    server,
    player,
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
    await view(page, farFrom(player.room));
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
