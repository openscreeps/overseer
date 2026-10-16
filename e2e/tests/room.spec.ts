// The drawer's Room tab: the room in focus as its own viewer (the room preview: drawn, picks a
// tile in 2D and 3D) and its facts (status, owner and level, energy, sources, mineral, objects
// by owner, the tick of the last update). Portal destinations are in map.spec.ts.

import { expect, test } from './fixtures';
import {
  clickTile,
  expectDrawn,
  objectList,
  openRoom,
  roomInfo,
  rowName,
  rowTile,
  selectTab,
  tab,
} from './support/room';

/** The tick the facts were last updated at (`Updated tick N`). */
const updated = (text: string | null) => Number(text?.match(/Updated\s*tick (\d+)/)?.[1] ?? 0);

test.describe('room', () => {
  test('shows the room’s facts and keeps them live', async ({ signedInPage: page, home }) => {
    await openRoom(page, home.path, home.room);
    const info = roomInfo(page, home.room);
    await expect(info.getByRole('heading', { level: 2 })).toHaveText(
      new RegExp(`^${home.room}\\s*Live$`),
    );
    for (const term of [/^Status$/, /^Owner$/, /^Sources?$/, /^Mineral$/, /^Updated$/]) {
      await expect(info.getByRole('term').filter({ hasText: term })).toHaveCount(1);
    }
    await expect(info).toContainText(/Status\s*(Open|Novice area|Respawn area|Closed)/);
    await expect(info.getByRole('progressbar', { name: 'Source energy' }).first()).toBeVisible();
    await expect(info).toContainText(/Mineral\s*[A-Z]+ · [\d,.\s]+ · density \d/);

    const first = updated(await info.textContent());
    expect(first).toBeGreaterThan(0);
    await expect.poll(async () => updated(await info.textContent()), { timeout: 30_000 })
      .toBeGreaterThan(first);
  });

  test(
    'shows the player’s room: owner, level, energy and objects by owner',
    { tag: '@openscreeps' },
    async ({ signedInPage: page, home, user }) => {
      await openRoom(page, home.path, home.room);
      const info = roomInfo(page, home.room);
      await expect(info).toContainText(/Status\s*Open/);
      await expect(info).toContainText(new RegExp(`Owner\\s*${user.username} · level 1`));
      await expect(info.getByRole('progressbar', { name: 'Progress to level 2' })).toBeVisible();
      await expect(info).toContainText(/Upgrade\s*[\d\s]+ \/ 200/);
      await expect(
        info.getByRole('progressbar', { name: 'Energy available for spawning' }),
      ).toBeVisible();
      await expect(info).toContainText(/Energy\s*\d+ \/ 300/);
      await expect(info.getByRole('term').filter({ hasText: /^Objects$/ })).toBeVisible();
      await expect(info).toContainText(
        new RegExp(`${user.username}\\s*\\d+ creeps? · \\d+ structures?`),
        { timeout: 30_000 },
      );
    },
  );

  test('previews the room and picks a tile in 2D and 3D', async ({ signedInPage: page, home }) => {
    await openRoom(page, home.path, home.room);
    const preview = page.getByTestId('room-control');
    await expect(preview).toHaveAccessibleName(`Room ${home.room}`);
    await expect(preview).toHaveAccessibleDescription(/click a tile to select its object/);
    await expectDrawn(preview);

    await selectTab(page, 'Objects');
    const list = objectList(page);
    const source = await rowName(list, /^Source at/);
    const mineral = await rowName(list, /^Mineral [A-Z]+ at/);

    for (const [mode, row] of [
      ['Top-down view', source],
      ['Perspective view', mineral],
    ] as const) {
      const button = page.getByRole('button', { name: mode });
      await button.click();
      await expect(button).toHaveAttribute('aria-pressed', 'true');
      await selectTab(page, 'Room');
      await expectDrawn(preview);
      const [x, y] = rowTile(row);
      await clickTile(page, 'room-control', home.room, x, y);
      await expect(tab(page, 'Objects')).toHaveAttribute('aria-selected', 'true');
      await expect(list.getByRole('button', { name: row, exact: true })).toHaveAttribute(
        'aria-current',
        'true',
      );
    }
  });
});
