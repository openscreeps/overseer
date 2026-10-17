// Players (route `#/<server>/@user/<name>`): a player's page opened from an owner name in the
// room facts, its rooms as links that open the map, the own rooms on the profile, and the "Find
// player" search (another player, an unknown name).

import { expect, register, test, unique } from './fixtures';
import { accountMenu } from './support/accounts';
import { openRoom, roomInfo, status } from './support/room';

/** `text` as a regular expression matching it literally. */
function literal(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

test.describe('players', { tag: '@openscreeps' }, () => {
  test('opens the owner from the room facts, then the map from the player’s rooms', async ({
    signedInPage: page,
    server,
    home,
    player,
  }) => {
    await openRoom(page, home.path, home.room);
    const info = roomInfo(page, home.room);
    await expect(info).toContainText(new RegExp(`Owner\\s*${player.username} · level 1`));
    // The Owner row comes first; the objects by owner name the player again.
    await info.getByRole('link', { name: player.username, exact: true }).first().click();

    await expect(page).toHaveURL(
      new RegExp(`#/${literal(server.slug)}/@user/${literal(player.username)}$`),
    );
    await expect(page.getByRole('heading', { level: 1, name: player.username })).toBeVisible();
    await expect(page).toHaveTitle(`${player.username} — Overseer`);
    const summary = page.getByRole('region', { name: player.username });
    await expect(summary.getByRole('progressbar', { name: /^GCL progress to level \d+$/ })).toBeVisible();

    const rooms = page.getByRole('region', { name: 'Rooms' });
    await rooms.getByRole('link', { name: home.room, exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`${literal(home.path)}$`));
    await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(home.room);
    await expect(rooms).toHaveCount(0);
  });

  test('finds another player by name and tells an unknown name', async ({
    signedInPage: page,
    server,
    home,
    user,
  }) => {
    const other = await register(server.url);
    await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
    await accountMenu(page, user.username, 'Profile');
    await expect(page.getByRole('heading', { level: 1, name: user.username })).toBeVisible();
    const own = page.getByRole('region', { name: 'Your rooms' });
    await expect(own.getByRole('link', { name: home.room, exact: true })).toBeVisible();

    const search = page.getByRole('search', { name: 'Find player' });
    await search.getByRole('searchbox', { name: 'Find player' }).fill(other.username);
    await search.getByRole('button', { name: 'Find' }).click();
    await expect(page).toHaveURL(new RegExp(`/@user/${literal(other.username)}$`));
    await expect(page.getByRole('heading', { level: 1, name: other.username })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Rooms' })).toContainText(
      'This player owns no room.',
    );

    const nobody = unique('nobody');
    await search.getByRole('searchbox', { name: 'Find player' }).fill(nobody);
    await search.getByRole('button', { name: 'Find' }).click();
    const missing = page.getByRole('alert').filter({ hasText: 'Player not found' });
    await expect(missing).toBeVisible();
    await expect(missing).toContainText(`No player is called “${nobody}”`);

    // Back to the map.
    await page.getByRole('button', { name: 'Back to the map' }).click();
    await expect(page).not.toHaveURL(/@user/);
    await expect(missing).toHaveCount(0);
  });
});
