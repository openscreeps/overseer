// Reference screenshots of the key screens and states, at a desktop (1440×900) and a small
// (390×844) viewport: what the UI looks like, for people comparing a styling change, never an
// assertion (canvas pixels depend on the GPU). Excluded from normal runs; `cargo xtask e2e
// --screens[=<set>]` runs it with `OVERSEER_E2E_SCREENS=<set>` and the images land in
// `e2e/references/<set>/<viewport>/` (`e2e/references/index.md` says what each one shows).

import { resolve } from 'node:path';
import type { Locator, Page } from '@playwright/test';
import { addServer, expect, SERVER_NAME, Session, test } from './fixtures';

/** The set the images belong to (`OVERSEER_E2E_SCREENS`); unset skips the spec. */
const SET = process.env.OVERSEER_E2E_SCREENS;

/** The viewports every screen is taken at. */
const VIEWPORTS = {
  desktop: { width: 1440, height: 900 },
  small: { width: 390, height: 844 },
} as const;

/** A canvas with the app's test hook `overseerTilePoint` (see `app/src/ui/surface.rs`). */
interface TilePointCanvas extends HTMLCanvasElement {
  /** CSS pixel `[x, y]` of the canvas where tile `(x, y)` of `room` is drawn, or `null`. */
  overseerTilePoint(room: string, x: number, y: number): [number, number] | null;
}

/** The status bar. */
const status = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The zoom level item of the status bar. */
const level = (page: Page) => status(page).getByTitle('Zoom level', { exact: true });

/** Saves `page` as `<name>.png` of the set and viewport. */
async function shoot(page: Page, viewport: string, name: string): Promise<void> {
  await page.screenshot({
    path: resolve(__dirname, '../references', SET ?? 'unset', viewport, `${name}.png`),
    animations: 'disabled',
    caret: 'hide',
  });
}

/** Waits until the canvas is drawn (a blank canvas compresses to almost nothing). */
async function drawn(canvas: Locator): Promise<void> {
  await expect
    .poll(async () => (await canvas.screenshot()).length, { timeout: 30_000 })
    .toBeGreaterThan(4_000);
}

/** Waits for two animation frames: the map draws pending input in the next one. */
async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(() => {
    const { promise, resolve } = Promise.withResolvers<void>();
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    return promise;
  });
}

/** Opens the world on `room`, live and framed on it. */
async function openRoom(page: Page, slug: string, room: string): Promise<void> {
  await page.goto(`/#/${slug}/-/${room}`);
  await expect(page.getByTestId('world-map')).toBeVisible();
  await expect(status(page)).toContainText('Connected', { timeout: 30_000 });
  await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
  await drawn(page.getByTestId('world-map'));
}

/**
 * Moves the mouse over tile `(x, y)` of `room` on the world map once the camera stands still
 * with the tile on the canvas; returns whether it could.
 */
async function hoverTile(page: Page, room: string, x: number, y: number): Promise<boolean> {
  const canvas = page.getByTestId('world-map');
  let at: { left: number; top: number } | null = null;
  await expect
    .poll(
      async () => {
        at = await canvas.evaluate(
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
            const [left, top] = [box.left + after[0], box.top + after[1]];
            const hit = document.elementFromPoint(left, top)?.getAttribute('data-testid');
            return hit === 'world-map' ? { left, top } : null;
          },
          [room, x, y] as const,
        );
        return at != null;
      },
      { timeout: 15_000 },
    )
    .toBe(true)
    .catch(() => {});
  const point = at as { left: number; top: number } | null;
  if (!point) return false;
  await page.mouse.move(point.left, point.top, { steps: 3 });
  return true;
}

for (const [viewport, size] of Object.entries(VIEWPORTS)) {
  test.describe(`screens @screens ${viewport}`, () => {
    test.skip(!SET, 'reference screenshots only: cargo xtask e2e --screens');
    test.use({ viewport: size });

    test('signed out: servers, add server, toast, sign-in, register', async ({ page, server }) => {
      await page.goto('/#/');
      await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
      await shoot(page, viewport, '01-servers');

      await page
        .getByRole('region', { name: 'Servers' })
        .getByRole('button', { name: 'Add server' })
        .click();
      const dialog = page.getByRole('dialog', { name: 'Add server' });
      await expect(dialog).toBeVisible();
      await dialog.getByLabel('Name', { exact: true }).fill(SERVER_NAME);
      await dialog.getByLabel('Address', { exact: true }).fill(server.url);
      await shoot(page, viewport, '02-add-server');
      await dialog.getByRole('button', { name: 'Add', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: `Added ${SERVER_NAME}` })).toBeVisible();
      await shoot(page, viewport, '03-toast');

      await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
      const signIn = page.getByRole('region', { name: `Sign in to ${SERVER_NAME}` });
      await expect(signIn.getByLabel('Username or e-mail')).toBeVisible();
      await shoot(page, viewport, '04-sign-in');

      await page
        .getByRole('group', { name: 'Account' })
        .getByRole('button', { name: 'Register', exact: true })
        .click();
      await expect(page.getByRole('region', { name: `Register on ${SERVER_NAME}` })).toBeVisible();
      await shoot(page, viewport, '05-register');
    });

    test('world map: whole world, sectors, room with tooltip', async ({
      signedInPage: page,
      server,
      player,
    }) => {
      await openRoom(page, server.slug, player.room);
      const map = page.getByTestId('world-map');
      await map.focus();
      // Escape zooms out 2.25× at once; twelve presses pass the zoom limit of any world.
      for (let i = 0; i < 12; i++) await page.keyboard.press('Escape');
      await nextFrames(page);
      await drawn(map);
      await shoot(page, viewport, '06-map-world');

      const box = await map.boundingBox();
      if (!box) throw new Error('the map has no box');
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      for (let i = 0; i < 80 && (await level(page).textContent()) !== 'Rooms'; i++) {
        await page.mouse.wheel(0, -100);
        await nextFrames(page);
      }
      await drawn(map);
      await shoot(page, viewport, '07-map-sectors');

      // The room again, framed whole as a user would: the room search.
      const search = page.getByRole('searchbox', { name: 'Go to room' });
      await search.fill(player.room);
      await search.press('Enter');
      await expect(level(page)).toHaveText('Room', { timeout: 30_000 });
      await drawn(map);
      if (await hoverTile(page, player.room, 25, 25)) {
        await expect(page.getByRole('tooltip'))
          .toBeVisible({ timeout: 10_000 })
          .catch(() => {});
      }
      await shoot(page, viewport, '08-map-room-tooltip');
    });

    test('drawer: room, objects with inspector, console', async ({
      signedInPage: page,
      server,
      player,
    }) => {
      await openRoom(page, server.slug, player.room);
      const drawerButton = page.getByRole('button', { name: 'Drawer', exact: true });
      const roomTab = page.getByRole('tab', { name: 'Room', exact: true });
      if (!(await roomTab.isVisible())) await drawerButton.click();
      await roomTab.click();
      await expect(
        page
          .getByRole('region', { name: new RegExp(player.room) })
          .getByRole('status')
          .filter({ hasText: 'Live' }),
      ).toBeVisible({ timeout: 30_000 });
      // The bottom sheet (narrower than 641 px) has no room viewer: the map shows the room.
      if (size.width >= 641) await drawn(page.getByTestId('room-control'));
      await shoot(page, viewport, '09-drawer-room');

      await page.getByRole('tab', { name: /Objects/ }).click();
      const row = page.getByRole('button', {
        name: `Spawn1 at ${player.spawn.x},${player.spawn.y}`,
      });
      await row.click();
      await expect(
        page.getByRole('region', { name: 'Inspector' }).getByRole('heading', { name: 'Spawn1' }),
      ).toBeVisible();
      await shoot(page, viewport, '10-drawer-objects-inspector');

      await page.getByRole('tab', { name: /Console/ }).click();
      await expect(
        page.getByRole('log', { name: 'Console output' }).getByText(/tick \d+/).first(),
      ).toBeVisible({ timeout: 30_000 });
      await shoot(page, viewport, '11-drawer-console');
    });

    test('account menu, profile, rate limits', async ({ signedInPage: page, server, player }) => {
      await openRoom(page, server.slug, player.room);
      const account = page.getByRole('button', { name: `Account: ${player.username}` });
      await account.click();
      await expect(page.getByRole('menu')).toBeVisible();
      await shoot(page, viewport, '12-account-menu');

      await page.getByRole('menuitem', { name: 'Rate limits' }).click();
      await expect(page.getByRole('dialog', { name: 'Rate limits' })).toBeVisible();
      await shoot(page, viewport, '13-rate-limits');
      await page.keyboard.press('Escape');

      await account.click();
      await page.getByRole('menuitem', { name: 'Profile' }).click();
      const profile = page.getByRole('region', { name: player.username });
      await expect(profile.getByRole('img', { name: `Badge of ${player.username}` })).toBeVisible();
      await expect(profile).toContainText(player.room);
      await shoot(page, viewport, '14-profile');
    });

    test('pop-out room window', async ({ signedInPage: page, server, player }) => {
      await openRoom(page, server.slug, player.room);
      const drawerButton = page.getByRole('button', { name: 'Drawer', exact: true });
      const popOut = page.getByRole('button', { name: 'Pop out room' });
      if (!(await popOut.isVisible())) await drawerButton.click();
      const [popup] = await Promise.all([page.waitForEvent('popup'), popOut.click()]);
      await popup.setViewportSize(size);
      await expect(popup.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
        timeout: 30_000,
      });
      await drawn(popup.getByTestId('room-control'));
      await shoot(popup, viewport, '15-room-window');
      await popup.close();
    });

    test('room close up: creeps with their badges, structures', async ({
      signedInPage: page,
      server,
      player,
    }) => {
      // Fresh accounts have no badge: give the player one, so their creeps carry it.
      const session = await Session.signIn(server.url, player.username, player.password);
      await session.call('api/user/badge', {
        badge: { type: 7, color1: '#2b6cb0', color2: '#f6ad55', color3: '#f7fafc', param: 0, flip: false },
      });
      await openRoom(page, server.slug, player.room);
      const map = page.getByTestId('world-map');
      const box = await map.boundingBox();
      if (!box) throw new Error('the map has no box');
      // The closest zoom over the spawn, where the creeps are born.
      for (let i = 0; i < 30; i++) {
        const at = await map.evaluate(
          (element, [room, x, y]) => (element as TilePointCanvas).overseerTilePoint(room, x, y),
          [player.room, player.spawn.x, player.spawn.y] as const,
        );
        if (!at) break;
        await page.mouse.move(box.x + at[0], box.y + at[1]);
        await page.mouse.wheel(0, -200);
        await nextFrames(page);
      }
      // Let the badges load and the zoom settle.
      await page.waitForTimeout(2_000);
      await shoot(page, viewport, '16-room-close');
      await page.getByRole('button', { name: 'Perspective view' }).click();
      await page.waitForTimeout(1_500);
      await shoot(page, viewport, '17-room-close-3d');
    });
  });
}
