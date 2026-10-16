// The right drawer itself: its tabs (keyboard, counts), its width (drag, keys, double-click,
// limits), collapsing it (its button, the top bar's), what survives a reload, and what it says
// while no room is in focus. The tabs' content has its own specs (room, objects, console).

import { expect, test } from './fixtures';
import { drawer, openRoom, selectTab, tab } from './support/room';

/** The drawer's default, narrowest and widest widths (`app/src/ui/drawer.rs`, `saved.rs`). */
const DEFAULT_WIDTH = 384;
const MIN_WIDTH = 280;
const MAX_WIDTH = 960;

test.describe('drawer', () => {
  test('moves between its tabs with the arrow keys, Home and End', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path, home.room);
    const room = tab(page, 'Room');
    await room.focus();
    for (const [key, name] of [
      ['ArrowRight', 'Objects'],
      ['ArrowRight', 'Console'],
      ['ArrowRight', 'Room'],
      ['ArrowLeft', 'Console'],
      ['Home', 'Room'],
      ['End', 'Console'],
    ] as const) {
      await page.keyboard.press(key);
      const selected = tab(page, name);
      await expect(selected).toHaveAttribute('aria-selected', 'true');
      await expect(selected).toBeFocused();
      await expect(page.getByRole('tabpanel')).toHaveAccessibleName(new RegExp(`^${name}`));
    }
    // Only the selected tab is in the tab order.
    await expect(page.getByRole('tab', { selected: false })).toHaveCount(2);
    for (const other of await page.getByRole('tab', { selected: false }).all()) {
      await expect(other).toHaveAttribute('tabindex', '-1');
    }
  });

  test('resizes by dragging its edge, with the keys, within limits, and resets', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path, home.room);
    const edge = page.getByRole('separator', { name: 'Resize drawer' });
    const width = async () => (await drawer(page).boundingBox())?.width ?? 0;
    await expect(edge).toHaveAttribute('aria-valuenow', String(DEFAULT_WIDTH));
    await expect(edge).toHaveAttribute('aria-valuemin', String(MIN_WIDTH));
    await expect(edge).toHaveAttribute('aria-valuemax', String(MAX_WIDTH));

    const box = await edge.boundingBox();
    if (!box) throw new Error('the edge has no box');
    const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x - 120, y, { steps: 6 });
    await page.mouse.up();
    await expect(edge).toHaveAttribute('aria-valuenow', String(DEFAULT_WIDTH + 120));
    expect(await width()).toBeCloseTo(DEFAULT_WIDTH + 120, 0);

    // Beyond what the window allows, the drawer stops at its limits.
    const wide = await edge.boundingBox();
    if (!wide) throw new Error('the edge has no box');
    await page.mouse.move(wide.x + wide.width / 2, y);
    await page.mouse.down();
    await page.mouse.move(1, y, { steps: 6 });
    await page.mouse.up();
    await expect(edge).toHaveAttribute('aria-valuenow', String(MAX_WIDTH));
    expect(await width()).toBeCloseTo(MAX_WIDTH, 0);

    // The keys: ← widens, → narrows by 16 px, Home and End go to the limits.
    await edge.focus();
    await page.keyboard.press('Home');
    await expect(edge).toHaveAttribute('aria-valuenow', String(MIN_WIDTH));
    await page.keyboard.press('ArrowRight');
    await expect(edge).toHaveAttribute('aria-valuenow', String(MIN_WIDTH));
    await page.keyboard.press('ArrowLeft');
    await expect(edge).toHaveAttribute('aria-valuenow', String(MIN_WIDTH + 16));
    expect(await width()).toBeCloseTo(MIN_WIDTH + 16, 0);
    await page.keyboard.press('End');
    await expect(edge).toHaveAttribute('aria-valuenow', String(MAX_WIDTH));
    await page.keyboard.press('ArrowRight');
    await expect(edge).toHaveAttribute('aria-valuenow', String(MAX_WIDTH - 16));

    await edge.dblclick();
    await expect(edge).toHaveAttribute('aria-valuenow', String(DEFAULT_WIDTH));
    expect(await width()).toBeCloseTo(DEFAULT_WIDTH, 0);
  });

  test('keeps its width and tab across a reload', async ({ signedInPage: page, home }) => {
    await openRoom(page, home.path, home.room);
    const edge = page.getByRole('separator', { name: 'Resize drawer' });
    await edge.focus();
    for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowLeft');
    await expect(edge).toHaveAttribute('aria-valuenow', String(DEFAULT_WIDTH + 80));
    await selectTab(page, 'Objects');

    await page.reload();
    await expect(tab(page, 'Objects')).toHaveAttribute('aria-selected', 'true');
    await expect(edge).toHaveAttribute('aria-valuenow', String(DEFAULT_WIDTH + 80));
    expect((await drawer(page).boundingBox())?.width ?? 0).toBeCloseTo(DEFAULT_WIDTH + 80, 0);
  });

  test('collapses, also across a reload, and reopens from the top bar', async ({
    signedInPage: page,
    home,
  }) => {
    await openRoom(page, home.path, home.room);
    await selectTab(page, 'Console');
    const toggle = page.getByRole('banner').getByRole('button', { name: 'Drawer', exact: true });
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');

    await page.getByRole('button', { name: 'Collapse drawer' }).click();
    await expect(drawer(page)).toBeHidden();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // The map takes the room the drawer left.
    const map = await page.getByTestId('world-map').boundingBox();
    expect(map?.width ?? 0).toBeGreaterThan(1200);

    await page.reload();
    await expect(page.getByTestId('world-map')).toBeVisible();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(drawer(page)).toBeHidden();

    await toggle.click();
    await expect(drawer(page)).toBeVisible();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(tab(page, 'Console')).toHaveAttribute('aria-selected', 'true');
    await toggle.click();
    await expect(drawer(page)).toBeHidden();
  });

  test('says what to do while no room is in focus', async ({ signedInPage: page, home }) => {
    test.fixme(
      true,
      'App bug: "Show the whole world" sets no room, then the next frame (still at the Room ' +
        'level while the camera zooms out) focuses the centre room again (app/src/ui/world.rs).',
    );
    await openRoom(page, home.path, home.room);
    const world = page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('button');
    await world.click();
    await expect(world).toHaveAttribute('aria-current', 'page');
    await expect(drawer(page)).toContainText(
      'Zoom into a room or click one on the map to see it here.',
    );
    await selectTab(page, 'Objects');
    await expect(drawer(page)).toContainText('Zoom into a room to list its objects.');
    // Without a live room there is nothing to count.
    await expect(tab(page, 'Objects')).toHaveAccessibleName(/^Objects$/);
  });

  test(
    'counts the console lines that arrive while its tab is hidden',
    { tag: '@openscreeps' },
    async ({ signedInPage: page, home }) => {
      await openRoom(page, home.path, home.room);
      // The player's bot logs every tick.
      const console = tab(page, 'Console');
      await expect(console).toHaveAccessibleName(/^Console\s*\d+$/, { timeout: 30_000 });
      const unread = async () =>
        Number((await console.textContent())?.match(/(\d+)\s*$/)?.[1] ?? 0);
      const first = await unread();
      await expect.poll(unread, { timeout: 30_000 }).toBeGreaterThan(first);

      await console.click();
      await expect(console).toHaveAccessibleName(/^Console$/);
      await selectTab(page, 'Room');
      await expect(console).toHaveAccessibleName(/^Console\s*\d+$/, { timeout: 30_000 });
    },
  );
});
