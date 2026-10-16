// The main window: its size the first time, and its size, position and maximized state across
// restarts (the shell's `tauri-plugin-window-state`, which saves them as the app exits).
// Popped-out windows: `pop-outs.spec.ts`.

import { expect, geometry, maximize, nativeTitle, place, test } from './fixtures';

test.describe('main window', () => {
  test('opens at its configured size the first time', async ({ app }) => {
    const page = await app.launch();
    expect(await nativeTitle(page)).toBe('Overseer');
    const { width, height, maximized, scale } = await geometry(page);
    expect({
      width: Math.round(width / scale),
      height: Math.round(height / scale),
      maximized,
    }).toEqual({ width: 1280, height: 820, maximized: false });
  });

  test('comes back with its size and position after a restart', async ({ app }) => {
    let page = await app.launch();
    const placed = await place(page, { x: 96, y: 72, width: 960, height: 640 });
    expect(Math.round(placed.width / placed.scale)).toBe(960);
    expect(Math.round(placed.height / placed.scale)).toBe(640);
    page = await app.restart();
    await expect.poll(() => geometry(page)).toEqual(placed);
  });

  test('comes back maximized, and restores to its size and position from before', async ({
    app,
  }) => {
    let page = await app.launch();
    const placed = await place(page, { x: 120, y: 90, width: 900, height: 600 });
    await maximize(page);
    page = await app.restart();
    await expect.poll(async () => (await geometry(page)).maximized).toBe(true);
    await maximize(page, false);
    await expect.poll(() => geometry(page)).toEqual(placed);
  });
});
