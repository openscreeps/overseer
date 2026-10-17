// Console autocomplete: suggestions for API names and the player's own creeps as the user types,
// Tab and a tap to accept, the list's keys (↑/↓, Escape), and the keys that keep their work:
// Enter runs the command, ↑ recalls the history while no list shows.
//
// The creep names come from the player's bot and the command runs on the server, so the tests
// are `@openscreeps`.

import type { Page } from '@playwright/test';
import { expect, test } from './fixtures';
import { consoleOutput as output, drawer, selectTab } from './support/room';

/** Opens the world at `path` on the Console tab and returns the console input. */
async function openConsole(page: Page, path: string) {
  await page.goto(path);
  await selectTab(page, 'Console');
  const input = page.getByRole('textbox', { name: 'Console expression' });
  await input.click();
  return input;
}

/** The suggestion list. */
const suggestions = (page: Page) => page.getByRole('listbox', { name: 'Suggestions' });

test.describe('console autocomplete', { tag: '@openscreeps' }, () => {
  test('completes names with Tab, own creeps after a dot, and keeps Enter and ↑', async ({
    signedInPage: page,
    home,
  }) => {
    const input = await openConsole(page, home.path);
    await expect(output(page)).toContainText(/tick \d+/, { timeout: 30_000 });

    // A member of `Game`: the first suggestion is selected, Tab inserts it.
    await input.pressSequentially('Game.cr');
    const list = suggestions(page);
    const creeps = list.getByRole('option', { name: 'creeps, object', exact: true });
    await expect(creeps).toHaveAttribute('aria-selected', 'true');
    await expect(input).toHaveAttribute('aria-activedescendant', (await creeps.getAttribute('id'))!);
    await input.press('Tab');
    await expect(input).toHaveValue('Game.creeps');
    await expect(list).toBeHidden();
    await expect(input).toBeFocused();
    expect(await input.evaluate((element: HTMLTextAreaElement) => element.selectionStart)).toBe(11);

    // The bot's creeps (`c<tick>`) after the dot; they load in the background, and an empty
    // answer (before the first creep) is asked again after 30 seconds.
    const creep = list.getByRole('option', { name: /^c\d+, creep$/ }).first();
    await expect(async () => {
      await input.fill('Game.creeps');
      await input.press('End');
      await input.pressSequentially('.');
      await expect(creep).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 90_000 });
    const name = (await creep.getAttribute('aria-label'))!.replace(/, creep$/, '');
    await creep.click();
    await expect(input).toHaveValue(`Game.creeps.${name}`);
    await expect(list).toBeHidden();

    // A creep's members. Escape closes the list (not the drawer); with the list closed, Tab
    // opens it again (2 matches), and Tab accepts the selected one.
    await input.pressSequentially('.na');
    const member = list.getByRole('option', { name: 'name, string', exact: true });
    await expect(member).toBeVisible();
    await input.press('Escape');
    await expect(list).toBeHidden();
    await expect(drawer(page)).toBeVisible();
    await input.press('Tab');
    await expect(member).toHaveAttribute('aria-selected', 'true');
    await expect(input).toBeFocused();
    await input.press('Tab');
    await expect(input).toHaveValue(`Game.creeps.${name}.name`);

    // Enter runs the command (it does not accept a suggestion) and the result arrives.
    const command = `Game.creeps.${name}.name`;
    await input.pressSequentially(' + "-e2e"');
    await expect(list).toBeHidden();
    await input.press('Enter');
    await expect(input).toHaveValue('');
    await expect(output(page).getByText(`${name}-e2e`, { exact: true })).toBeVisible({
      timeout: 30_000,
    });

    // With no list open, ↑ recalls the command.
    await input.press('ArrowUp');
    await expect(input).toHaveValue(`${command} + "-e2e"`);
    await expect(list).toBeHidden();
  });

  test('moves through suggestions with the arrow keys and accepts a tap', async ({
    signedInPage: page,
    home,
  }) => {
    const input = await openConsole(page, home.path);
    await input.pressSequentially('FIND_SO');
    const list = suggestions(page);
    const sources = list.getByRole('option', { name: 'FIND_SOURCES, 105', exact: true });
    const active = list.getByRole('option', { name: 'FIND_SOURCES_ACTIVE, 104', exact: true });
    await expect(sources).toHaveAttribute('aria-selected', 'true');
    await input.press('ArrowDown');
    await expect(active).toHaveAttribute('aria-selected', 'true');
    await expect(sources).toHaveAttribute('aria-selected', 'false');
    await input.press('ArrowUp');
    await expect(sources).toHaveAttribute('aria-selected', 'true');

    await active.click();
    await expect(input).toHaveValue('FIND_SOURCES_ACTIVE');
    await expect(input).toBeFocused();
    await expect(list).toBeHidden();
  });
});
