// The room search ("Go to room"): it frames the room whole and hands the keyboard back to the
// map, `/` focuses it and Escape leaves it, and what is not a room of the world is refused.
//
// Read-only: every test runs on any world around the signed-in account's `home` room.

import { expect, test } from './fixtures';
import {
  breadcrumb,
  level,
  openRoom,
  restingCamera,
  roomCentre,
  roomInfo,
  roomName,
  routeOf,
  search,
  towardsMiddle,
  worldBounds,
  worldMap,
} from './world-map';

test.describe('room search', () => {
  test('goes to a room in any case, framed whole, and hands the keyboard back to the map', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const other = towardsMiddle(home.room, await worldBounds(page, server, home)).room;
    // Zoomed in as far as the map goes (it is clamped) and off the room's centre.
    await worldMap(page).focus();
    for (let i = 0; i < 12; i++) await page.keyboard.press('+');
    for (let i = 0; i < 4; i++) await page.keyboard.press('ArrowLeft');
    await restingCamera(page, server, home, (camera) => camera.half_height < 25);

    await search(page).fill(other.toLowerCase());
    await search(page).press('Enter');
    await expect(page).toHaveURL(routeOf(server, home, other));
    await expect(breadcrumb(page)).toContainText(other);
    await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
    // The camera looks at the room's centre with its 50 tiles (and a margin) in view.
    const [x, y] = roomCentre(other);
    await restingCamera(
      page,
      server,
      home,
      (camera) =>
        camera.half_height >= 25 &&
        Math.abs(camera.centre[0] - x) < 0.5 &&
        Math.abs(camera.centre[1] - y) < 0.5,
    );
    await expect(
      roomInfo(page, other).getByRole('status').filter({ hasText: 'Live' }),
    ).toBeVisible({ timeout: 30_000 });
    // The search is cleared and the keys go to the map, without a focus ring around it.
    await expect(search(page)).toHaveValue('');
    await expect(worldMap(page)).toBeFocused();
    expect(await worldMap(page).evaluate((canvas) => getComputedStyle(canvas).outlineStyle)).toBe(
      'none',
    );
    // Two zoom-out steps (1.5× each) leave the live level.
    await page.keyboard.press('-');
    await page.keyboard.press('-');
    await expect(level(page)).not.toHaveText('Room');
  });

  test('slash focuses the search, Escape clears it and returns to the map', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path);
    await expect(search(page)).toHaveAttribute('aria-keyshortcuts', '/');
    await worldMap(page).focus();
    await page.keyboard.press('/');
    await expect(search(page)).toBeFocused();
    await expect(search(page)).toHaveValue('');
    await page.keyboard.type('W1');
    await page.keyboard.press('Escape');
    await expect(search(page)).toHaveValue('');
    await expect(worldMap(page)).toBeFocused();
    // Escape in the search is not the map's zoom out.
    await expect(level(page)).toHaveText('Room');
    await expect(breadcrumb(page)).toContainText(home.room);
  });

  test('refuses what is not a room name, and rooms outside the world', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    await openRoom(page, home.path);
    const bounds = await worldBounds(page, server, home);
    const outside = roomName(bounds.max[0] + 1, bounds.max[1]);
    const error = page.getByRole('alert').filter({ hasText: /\S/ });

    await search(page).fill('nowhere');
    await search(page).press('Enter');
    await expect(error).toHaveText('“nowhere” is not a room name such as W8N3');
    await expect(search(page)).toHaveAttribute('aria-invalid', 'true');
    await expect(search(page)).toBeFocused();
    // Typing again clears the error.
    await search(page).press('End');
    await page.keyboard.type('x');
    await expect(error).toHaveCount(0);
    await expect(search(page)).toHaveAttribute('aria-invalid', 'false');

    await search(page).fill(outside.toLowerCase());
    await search(page).press('Enter');
    await expect(error).toHaveText(`${outside} is outside this world`);
    await expect(search(page)).toHaveValue(outside.toLowerCase());
    // The map stays where it was.
    await expect(page).toHaveURL(routeOf(server, home, home.room));
    await expect(breadcrumb(page)).toContainText(home.room);
  });
});
