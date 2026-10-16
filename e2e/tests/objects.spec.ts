// The drawer's Objects tab: the live room's objects grouped by kind with counts, collapsible
// groups, the filter, the inspector (typed fields per kind, the other objects on the tile, the
// raw JSON) and the selection shared with the map and the room preview both ways.

import type { Page } from '@playwright/test';
import { expect, Session, test, type Player, type Server } from './fixtures';
import {
  clickTile,
  inspectedTile,
  inspector,
  mapTile,
  objectList,
  openRoom,
  rowName,
  rowTile,
  rows,
  selectTab,
  tab,
} from './support/room';

/** The number a group heading or tab ends with (`Sources 2`). */
const count = (text: string | null) => Number(text?.match(/(\d+)\s*$/)?.[1] ?? Number.NaN);

/** The group labels of the list, in order, without their counts. */
async function groupLabels(page: Page): Promise<string[]> {
  const heads = await objectList(page).getByRole('heading', { level: 3 }).allTextContents();
  return heads.map((text) => text.replace(/\s*\d+\s*$/, '').trim());
}

/** Opens `room` live on the Objects tab, once its sources are listed. */
async function openObjects(page: Page, path: string, room: string) {
  await openRoom(page, path, room);
  await selectTab(page, 'Objects');
  await expect(objectList(page).getByRole('button', { name: /^Sources?\s*\d+$/ })).toBeVisible({
    timeout: 30_000,
  });
}

/** Replaces the player's code (`POST /api/user/code`); the world runs it from the next tick. */
async function deploy(server: Server, player: Player, main: string) {
  const session = await Session.signIn(server.url, player.username, player.password);
  await session.call('api/user/code', { modules: { main } });
}

test.describe('objects', () => {
  test('groups the objects by kind and counts them alike everywhere', async ({
    signedInPage: page,
    home,
  }) => {
    await openObjects(page, home.path, home.room);
    const labels = await groupLabels(page);
    // Kinds in the game's order: the controller before the sources before the mineral.
    for (const [before, after] of [
      ['Controllers', 'Sources'],
      ['Sources', 'Minerals'],
    ]) {
      expect(labels).toContain(before);
      expect(labels.indexOf(before)).toBeLessThan(labels.indexOf(after));
    }
    // The groups' counts, the rows, the count beside the filter and the tab's agree (read
    // until a tick does not fall between the reads).
    const list = objectList(page);
    await expect
      .poll(async () => {
        const heads = await list.getByRole('heading', { level: 3 }).allTextContents();
        const sum = heads.map(count).reduce((total, n) => total + n, 0);
        const shown = await rows(list).count();
        const total = count(await page.getByTitle('Objects in the room').textContent());
        const badge = count(await tab(page, 'Objects').textContent());
        return sum > 0 && [shown, total, badge].every((n) => n === sum);
      })
      .toBe(true);
  });

  test('collapses and expands a group', async ({ signedInPage: page, home }) => {
    await openObjects(page, home.path, home.room);
    const list = objectList(page);
    const sources = list.getByRole('button', { name: /^Sources?\s*\d+$/ });
    const sourceRows = list.getByRole('button', { name: /^Source at \d+,\d+$/ });
    const n = count(await sources.textContent());
    await expect(sources).toHaveAttribute('aria-expanded', 'true');
    await expect(sourceRows).toHaveCount(n);

    await sources.click();
    await expect(sources).toHaveAttribute('aria-expanded', 'false');
    await expect(sourceRows).toHaveCount(0);
    // The others stay open, and the group keeps its count.
    await expect(list.getByRole('button', { name: /^Controller · level \d+ at/ })).toBeVisible();
    expect(count(await sources.textContent())).toBe(n);

    await sources.click();
    await expect(sources).toHaveAttribute('aria-expanded', 'true');
    await expect(sourceRows).toHaveCount(n);
  });

  test('filters by kind and position, and says when nothing matches', async ({
    signedInPage: page,
    home,
  }) => {
    await openObjects(page, home.path, home.room);
    const list = objectList(page);
    const filter = page.getByRole('searchbox', { name: 'Filter objects' });
    const sources = count(
      await list.getByRole('button', { name: /^Sources?\s*\d+$/ }).textContent(),
    );

    await filter.fill('SOURCE');
    await expect(rows(list)).toHaveCount(sources);
    for (const name of await rows(list).evaluateAll((all) => all.map((row) => row.ariaLabel))) {
      expect(name).toMatch(/^Source at \d+,\d+$/);
    }
    expect(await groupLabels(page)).toEqual(['Sources']);

    await filter.fill('');
    const controller = await rowName(list, /^Controller · level \d+ at/);
    const [x, y] = rowTile(controller);
    await filter.fill(`${x},${y}`);
    await expect(list.getByRole('button', { name: controller, exact: true })).toBeVisible();
    for (const name of await rows(list).evaluateAll((all) => all.map((row) => row.ariaLabel))) {
      expect(name).toMatch(new RegExp(` at ${x},${y}$`));
    }

    await filter.fill('no object is called this');
    await expect(list.getByRole('status')).toHaveText('No objects match the filter.');
    await expect(rows(list)).toHaveCount(0);
    // The filter hides rows, not objects: the counts of the room stay.
    expect(count(await tab(page, 'Objects').textContent())).toBeGreaterThan(0);

    await filter.fill('');
    await expect(list.getByRole('status')).toHaveCount(0);
    await expect(list.getByRole('button', { name: controller, exact: true })).toBeVisible();
  });

  test('inspects a controller, a source and a mineral, with their raw JSON', async ({
    signedInPage: page,
    home,
  }) => {
    await openObjects(page, home.path, home.room);
    const list = objectList(page);
    const panel = inspector(page);
    await expect(panel).toContainText('Select an object in the list or on the map.');

    const kinds = [
      { row: /^Controller · level \d+ at/, kind: 'Controller', terms: ['Level'], meters: [] },
      { row: /^Source at/, kind: 'Source', terms: ['Energy'], meters: ['Energy'] },
      { row: /^Mineral [A-Z]+ at/, kind: 'Mineral', terms: ['Type', 'Amount', 'Density'], meters: [] },
    ];
    for (const { row, kind, terms, meters } of kinds) {
      const name = await rowName(list, row);
      const [x, y] = rowTile(name);
      const button = list.getByRole('button', { name, exact: true });
      await button.click();
      await expect(button).toHaveAttribute('aria-current', 'true');
      await expect(list.locator('[aria-current="true"]')).toHaveCount(1);
      await expect(panel.getByRole('heading')).toHaveText(name.replace(/ at \d+,\d+$/, ''));
      await expect(panel.getByText(kind, { exact: true })).toBeVisible();
      await expect(inspectedTile(panel)).toHaveText(`${x},${y}`);
      for (const term of terms) {
        await expect(panel.getByRole('term').filter({ hasText: new RegExp(`^${term}$`) })).toBeVisible();
      }
      for (const meter of meters) {
        await expect(panel.getByRole('progressbar', { name: meter })).toBeVisible();
      }
      // The map selects the object's tile.
      await expect.poll(() => mapTile(page)).toBe(`${x},${y}`);

      await panel.getByText('Raw JSON', { exact: true }).click();
      const type = kind.toLowerCase();
      await expect(panel.getByText(new RegExp(`"type": "${type}"`))).toBeVisible();
      await expect(panel.getByText(new RegExp(`"x": ${x},`))).toBeVisible();
    }
  });

  test('selects one object after another by clicks on the map and the room preview', async ({
    signedInPage: page,
    home,
  }) => {
    await openObjects(page, home.path, home.room);
    // As a user would: frame the room with the search, then pick on the map.
    const search = page.getByRole('searchbox', { name: 'Go to room' });
    await search.fill(home.room);
    await search.press('Enter');
    const list = objectList(page);
    const names = await Promise.all(
      [/^Controller · level \d+ at/, /^Source at/, /^Mineral [A-Z]+ at/].map((name) =>
        rowName(list, name),
      ),
    );
    const panel = inspector(page);
    /** Clicks the object of `row` on canvas `testid`: it becomes the selection. */
    const pick = async (testid: string, row: string) => {
      const [x, y] = rowTile(row);
      await clickTile(page, testid, home.room, x, y);
      await expect(list.getByRole('button', { name: row, exact: true })).toHaveAttribute(
        'aria-current',
        'true',
      );
      await expect(inspectedTile(panel)).toHaveText(`${x},${y}`);
    };

    // The main map: each click selects its object (A, B, C, then A again).
    for (const row of [...names, names[0]]) {
      await pick('world-map', row);
      await expect.poll(() => mapTile(page)).toBe(rowTile(row).join(','));
    }

    // The drawer's room preview: a pick there opens the Objects tab, so come back each time.
    for (const row of [...names.slice(1), names[0]]) {
      await selectTab(page, 'Room');
      await pick('room-control', row);
      await expect(tab(page, 'Objects')).toHaveAttribute('aria-selected', 'true');
    }
  });

  test.describe('in the player’s room', { tag: '@openscreeps' }, () => {
    test('lists the player’s objects first and follows a walking creep', async ({
      signedInPage: page,
      home,
      player,
    }) => {
      await openRoom(page, home.path, home.room);
      // A click on the spawn in the room preview selects it on the Objects tab and the map.
      await clickTile(page, 'room-control', home.room, player.spawn.x, player.spawn.y);
      await expect(tab(page, 'Objects')).toHaveAttribute('aria-selected', 'true');
      const list = objectList(page);
      const spawn = list.getByRole('button', {
        name: `Spawn1 at ${player.spawn.x},${player.spawn.y}`,
      });
      await expect(spawn).toHaveAttribute('aria-current', 'true');
      const panel = inspector(page);
      await expect(panel.getByRole('heading', { name: 'Spawn1' })).toBeVisible();
      await expect(panel.getByText('Spawn', { exact: true })).toBeVisible();
      await expect(panel.getByText(player.username, { exact: true })).toBeVisible();
      await expect(panel.getByRole('progressbar', { name: 'Hits' })).toBeVisible();
      await expect(panel.getByRole('term').filter({ hasText: /^Spawning$/ })).toBeVisible();
      await expect.poll(() => mapTile(page)).toBe(`${player.spawn.x},${player.spawn.y}`);

      // The player's kinds first (creeps, spawns, the controller), then the room's.
      await expect(list.getByRole('button', { name: /^Creeps\s*\d+$/ })).toBeVisible({
        timeout: 30_000,
      });
      expect((await groupLabels(page)).slice(0, 4)).toEqual([
        'Creeps',
        'Spawns',
        'Controllers',
        'Sources',
      ]);

      // A creep picked in the list: its fields, and the map follows it as it walks.
      const creep = list.getByRole('button', { name: /^c\d+ at \d+,\d+$/ }).first();
      await creep.click();
      await expect(creep).toHaveAttribute('aria-current', 'true');
      await expect(panel.getByText('Creep', { exact: true })).toBeVisible();
      for (const field of ['Hits', 'Ticks to live', 'Fatigue', 'Body']) {
        await expect(panel.getByRole('term').filter({ hasText: new RegExp(`^${field}$`) })).toBeVisible();
      }
      await expect(panel.getByRole('list', { name: '1 body parts' })).toBeVisible();
      // Both read at once: the creep walks every tick, and two separate reads of a slow page
      // straddle a step more often than not.
      await expect
        .poll(() =>
          page.evaluate(() => {
            const panel = document.querySelector('[aria-label="Inspector"]');
            if (!panel) return false;
            const texts = document.createTreeWalker(panel, NodeFilter.SHOW_TEXT);
            let position: string | undefined;
            for (let node = texts.nextNode(); node && !position; node = texts.nextNode()) {
              const text = node.textContent?.trim() ?? '';
              if (/^\d{1,2},\d{1,2}$/.test(text)) position = text;
            }
            const map = document
              .querySelector('[data-testid="world-map"]')
              ?.getAttribute('aria-label');
            return position != null && position === map?.match(/tile (\d+,\d+) selected/)?.[1];
          }),
        )
        .toBe(true);

      // The owner's name finds everything they own, nothing else.
      await page.getByRole('searchbox', { name: 'Filter objects' }).fill(player.username);
      await expect(spawn).toBeVisible();
      await expect(list.getByRole('button', { name: /^Controller · level 1 at/ })).toBeVisible();
      await expect(list.getByRole('button', { name: /^Source at/ })).toHaveCount(0);
    });

    test('lists the other objects on the tile and switches to them', async ({
      signedInPage: page,
      server,
      home,
      player,
    }) => {
      // A creep stands on a road's construction site two tiles right of the spawn.
      const [x, y] = [player.spawn.x + 2, player.spawn.y];
      await deploy(
        server,
        player,
        `module.exports.loop = function () {
          const spawn = Game.spawns.Spawn1;
          const pos = new RoomPosition(${x}, ${y}, spawn.room.name);
          if (!pos.lookFor(LOOK_CONSTRUCTION_SITES).length) pos.createConstructionSite(STRUCTURE_ROAD);
          if (!Game.creeps.idle && !spawn.spawning) spawn.spawnCreep([MOVE], 'idle');
          if (Game.creeps.idle) Game.creeps.idle.moveTo(pos);
        };`,
      );
      await openObjects(page, home.path, home.room);
      const list = objectList(page);
      const creep = list.getByRole('button', { name: `idle at ${x},${y}`, exact: true });
      const site = list.getByRole('button', { name: `Road (site) at ${x},${y}`, exact: true });
      await expect(site).toBeVisible({ timeout: 30_000 });
      await expect(creep).toBeVisible({ timeout: 60_000 });

      const panel = inspector(page);
      await site.click();
      await expect(panel.getByRole('heading')).toHaveText('Road (site)');
      await expect(panel.getByRole('progressbar', { name: 'Progress' })).toBeVisible();
      await expect(panel).toContainText('Also here');
      await panel.getByRole('button', { name: 'idle', exact: true }).click();
      await expect(creep).toHaveAttribute('aria-current', 'true');
      await expect(panel.getByRole('heading')).toHaveText('idle');
      await panel.getByRole('button', { name: 'Road (site)', exact: true }).click();
      await expect(site).toHaveAttribute('aria-current', 'true');
    });

    test('says when the selected object is gone', async ({
      signedInPage: page,
      server,
      home,
      player,
    }) => {
      await openObjects(page, home.path, home.room);
      const list = objectList(page);
      const name = await rowName(list, /^c\d+ at \d+,\d+$/);
      const creep = name.replace(/ at \d+,\d+$/, '');
      await list.getByRole('button', { name, exact: true }).click();
      await expect(inspector(page).getByRole('heading')).toHaveText(creep);

      const session = await Session.signIn(server.url, player.username, player.password);
      await session.call('api/user/console', { expression: `Game.creeps.${creep}.suicide()` });
      await expect(inspector(page).getByRole('status')).toHaveText('The object is gone.', {
        timeout: 30_000,
      });
      await expect(list.getByRole('button', { name: new RegExp(`^${creep} at`) })).toHaveCount(0);
      await expect(
        list.getByRole('button', { name: new RegExp(`^Tombstone of ${creep} at`) }),
      ).toBeVisible();
    });
  });
});
