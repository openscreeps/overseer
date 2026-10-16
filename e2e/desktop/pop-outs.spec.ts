// Popped-out windows (the shell's `open_window`): the room and the console each in a native
// window of their own, titled after what they show, live like the main window and signed in
// with the same keychain session; one window per route (popping out again brings the open one
// back); their size and position kept across restarts like the main window's.

import type { Page } from '@playwright/test';
import {
  expect,
  geometry,
  nativeTitle,
  open,
  ORIGIN,
  place,
  signIn,
  statusBar,
  test,
  type Home,
  type Server,
} from './fixtures';

/** The app's route of the room `home` on `server`. */
const roomRoute = (server: Server, home: Home) =>
  `#/${server.slug}/${home.shard ?? '-'}/${home.room}`;

/** ` · <shard>` in window titles on a sharded server. */
const shardPart = (home: Home) => (home.shard ? ` · ${home.shard}` : '');

/** Opens the world on `home`, live. */
async function openRoom(page: Page, server: Server, home: Home): Promise<void> {
  await open(page, roomRoute(server, home));
  await expect(statusBar(page)).toContainText('Connected', { timeout: 30_000 });
}

/** The window's inner size in logical pixels. */
async function logicalSize(page: Page): Promise<[number, number]> {
  const { width, height, scale } = await geometry(page);
  return [Math.round(width / scale), Math.round(height / scale)];
}

test.describe('pop-outs', () => {
  test('pops a room out into a window of its own, once', async ({ app, server, presence }) => {
    const { home } = presence;
    const page = await app.launch();
    await signIn(page, server, presence.credentials);
    await openRoom(page, server, home);
    const popOut = page.getByRole('button', { name: 'Pop out room' });
    const room = await app.popOut(() => popOut.click());

    await expect(room).toHaveURL(`${ORIGIN}/index.html${roomRoute(server, home)}/window`);
    expect(await nativeTitle(room)).toBe(
      `${home.room}${shardPart(home)} · ${server.name} — Overseer`,
    );
    expect(await logicalSize(room)).toEqual([640, 640]);
    await expect(room.getByRole('heading', { level: 1 })).toContainText(home.room);
    await expect(room.getByRole('status').filter({ hasText: 'Live' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(statusBar(room)).toContainText(/tick \d+/);

    // Minimized, popping the room out again brings the window back: no second window.
    await room.evaluate(() => window.__TAURI__!.window.getCurrentWindow().minimize());
    const minimized = () =>
      room.evaluate(() => window.__TAURI__!.window.getCurrentWindow().isMinimized());
    await expect.poll(minimized).toBe(true);
    await popOut.click();
    await expect.poll(minimized).toBe(false);
    expect(app.context.pages()).toHaveLength(2);
  });

  test('pops the console out into a window of its own', async ({
    app,
    server,
    presence,
    target,
  }) => {
    const { home } = presence;
    const page = await app.launch();
    await signIn(page, server, presence.credentials);
    await openRoom(page, server, home);
    await page.getByRole('tab', { name: /Console/ }).click();
    const console_ = await app.popOut(() =>
      page.getByRole('button', { name: 'Pop out console' }).click(),
    );

    await expect(console_).toHaveURL(
      `${ORIGIN}/index.html#/${server.slug}/${home.shard ?? '-'}/console/window`,
    );
    expect(await nativeTitle(console_)).toBe(
      `Console${shardPart(home)} · ${server.name} — Overseer`,
    );
    expect(await logicalSize(console_)).toEqual([640, 640]);
    await expect(console_.getByRole('heading', { level: 1 })).toContainText('Console');
    const output = console_.getByRole('log', { name: 'Console output' });
    await expect(output).toBeVisible();
    if (target === 'openscreeps') {
      // The player's bot logs every tick.
      await expect(output).toContainText(/tick \d+/, { timeout: 30_000 });
    }
  });

  test('a popped-out window comes back with its size and position after a restart', async ({
    app,
    server,
    presence,
  }) => {
    const { home } = presence;
    let page = await app.launch();
    await signIn(page, server, presence.credentials);
    await openRoom(page, server, home);
    let room = await app.popOut(() => page.getByRole('button', { name: 'Pop out room' }).click());
    const placed = await place(room, { x: 160, y: 120, width: 520, height: 480 });
    expect(await logicalSize(room)).toEqual([520, 480]);

    page = await app.restart();
    // Pop-outs do not reopen by themselves.
    expect(app.context.pages()).toHaveLength(1);
    await openRoom(page, server, home);
    room = await app.popOut(() => page.getByRole('button', { name: 'Pop out room' }).click());
    await expect.poll(() => geometry(room)).toEqual(placed);
  });
});
