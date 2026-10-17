// End-to-end tests of the desktop app (the Tauri shell on Windows, WebView2), in `desktop/`.
// Run them with `cargo xtask e2e --desktop`, which builds the app for testing, starts the
// server under test and sets the environment read here and by the fixtures
// (`desktop/fixtures.ts`): Playwright attaches to the app's own WebView2, no browser of its own.
//
// `OVERSEER_E2E_TARGET` picks the server as for the web tests (`playwright.config.ts`); the
// `official` run (`--target official`) leaves out the `@openscreeps` tests (see
// `tests/fixtures`) and records no screenshots.
//
// One worker: the test app keeps its profile, window state and keychain entries per identifier,
// so one instance runs at a time.

import { resolve } from 'node:path';
import { defineConfig } from '@playwright/test';
import { currentTarget, type Target } from './tests/fixtures/api';

/** The tests each target leaves out. */
const excluded: Record<Target, RegExp | undefined> = {
  openscreeps: undefined,
  official: /@openscreeps/,
  community: undefined,
};

// The run directory of `cargo xtask e2e --desktop`: the results and report go there.
const output = process.env.OVERSEER_E2E_OUTPUT ?? resolve(__dirname, '../target/e2e/desktop');

export default defineConfig({
  testDir: './desktop',
  outputDir: resolve(output, 'test-results'),
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  grepInvert: excluded[currentTarget()],
  reporter: [['list'], ['html', { open: 'never', outputFolder: resolve(output, 'playwright-report') }]],
});
