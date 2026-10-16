// External links: the desktop app opens screeps.com's pages (register, recover a password, API
// tokens) in the system browser through the shell's opener, and the shell opens nothing else.
//
// The test stands in for the opener (`interceptOpener`) and records what the app asks it to
// open: a test must not open the user's browser. The shell's refusal of any other address is
// checked against the real opener.

import { expect, interceptOpener, open, test } from './fixtures';

test('opens screeps.com pages in the system browser, and nothing else', async ({ app }) => {
  const page = await app.launch();
  await open(page, '#/');
  await page.getByRole('button', { name: 'Sign in screeps.com' }).click();
  const form = page.getByRole('region', { name: 'Sign in to screeps.com' });
  await expect(form).toBeVisible();
  const opened = await interceptOpener(page);

  await form.getByRole('button', { name: 'Register on screeps.com' }).click();
  await form.getByRole('button', { name: 'Forgot your password?' }).click();
  await form.getByRole('button', { name: 'API token', exact: true }).click();
  await form.getByRole('button', { name: 'Open the API token page' }).click();
  await expect
    .poll(opened)
    .toEqual([
      'https://screeps.com/a/#!/register',
      'https://screeps.com/a/#!/register/ask-recover',
      'https://screeps.com/a/#!/account/auth-tokens',
    ]);
  // The app stays where it was: no navigation, no window of its own.
  await expect(form).toBeVisible();
  expect(app.context.pages()).toHaveLength(1);

  const refused = await page.evaluate(() =>
    window.e2eInvoke!('plugin:opener|open_url', { url: 'https://example.com/' }).then(
      () => 'opened',
      (error: unknown) => String(error),
    ),
  );
  expect(refused).toContain('Not allowed to open url https://example.com/');
});
