// A room popped out of the drawer into its own window: its viewer, facts, objects and
// inspector on a session of its own, its own 2D/3D switch, and what it says when the room
// cannot be opened. That the window opens live and drawn is in world.spec.ts.

import { expect, test } from './fixtures';
import {
  clickTile,
  expectDrawn,
  inspector,
  objectList,
  openRoom,
  roomInfoStatus,
  rowName,
  rowTile,
  tab,
} from './support/room';

test.describe('room window', () => {
  test('shows the room’s facts and objects, inspects them and picks on its own viewer', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path, home.room);
    const [popup] = await Promise.all([
      page.waitForEvent('popup'),
      page.getByRole('button', { name: 'Pop out room' }).click(),
    ]);
    // On screeps.com a low map-stats budget prompts to lift it; the window is not about that.
    const prompt = popup.getByRole('dialog', { name: 'Rate limit running low' });
    await popup.addLocatorHandler(prompt, () => prompt.getByRole('button', { name: 'Not now' }).click());
    await expect(popup).toHaveURL(new RegExp(`/${home.room}/window$`));
    await expect(popup.getByRole('heading', { level: 1 })).toContainText(home.room);
    await expect(popup).toHaveTitle(new RegExp(`^${home.room} · .+ — Overseer$`));
    await expect(roomInfoStatus(popup, home.room)).toHaveText('Live', { timeout: 30_000 });

    // The objects and the inspector beside the facts, no tabs.
    await expect(popup.getByRole('tab')).toHaveCount(0);
    const list = objectList(popup);
    const controller = await rowName(list, /^Controller · level \d+ at/);
    await list.getByRole('button', { name: controller, exact: true }).click();
    await expect(inspector(popup).getByRole('heading')).toHaveText(
      controller.replace(/ at \d+,\d+$/, ''),
    );

    // Its own viewer and 2D/3D switch: a pick there selects in its list.
    const viewer = popup.getByTestId('room-control');
    await expectDrawn(viewer);
    const perspective = popup.getByRole('button', { name: 'Perspective view' });
    await perspective.click();
    await expect(perspective).toHaveAttribute('aria-pressed', 'true');
    await expect(popup.getByRole('button', { name: 'Top-down view' })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    const source = await rowName(list, /^Source at/);
    const [x, y] = rowTile(source);
    await clickTile(popup, 'room-control', home.room, x, y);
    await expect(list.getByRole('button', { name: source, exact: true })).toHaveAttribute(
      'aria-current',
      'true',
    );
    // The main window keeps its own selection and tab.
    await expect(tab(page, 'Room')).toHaveAttribute('aria-selected', 'true');
    await popup.close();
  });

  test('says when the room is outside the world and tries again', async ({
    signedInPage: page,
    home,
  }) => {
    const path = home.path.replace(new RegExp(`${home.room}$`), 'W250N250/window');
    await page.goto(path);
    const alert = page.getByRole('alert').filter({ hasText: 'could not be opened' });
    await expect(alert).toHaveText(
      /The room could not be opened: W250N250 is outside this world\.\s*Try again/,
      { timeout: 30_000 },
    );
    await alert.getByRole('button', { name: 'Try again' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Opening the room…' }).or(alert))
      .toBeVisible();
    await expect(alert).toBeVisible({ timeout: 30_000 });
  });
});
