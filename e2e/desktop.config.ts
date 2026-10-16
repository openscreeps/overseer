// End-to-end tests of the desktop app (the Tauri shell on Windows, WebView2), in `desktop/`.
// Run them with `cargo xtask e2e --desktop`, which builds the app for testing, starts the
// server under test and sets the environment read here and by the fixtures
// (`desktop/fixtures.ts`): Playwright attaches to the app's own WebView2, no browser of its own.
//
// `OVERSEER_E2E_TARGET` picks the server as for the web tests (`playwright.config.ts`); the
// `official` run (`--target official`) leaves out the `@openscreeps` tests and records no
// screenshots.
//
// One worker: the test app keeps its profile, window state and keychain entries per identifier,
// so one instance runs at a time.

import { defineConfig } from '@playwright/test';
import { currentTarget } from './tests/fixtures/api';

const official = currentTarget() === 'official';

export default defineConfig({
  testDir: './desktop',
  outputDir: './test-results/desktop',
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  grepInvert: official ? /@openscreeps/ : undefined,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report/desktop' }]],
});
