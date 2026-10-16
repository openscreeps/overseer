// UI end-to-end tests of the Overseer web app (Chromium). Run them with `cargo xtask e2e`, which
// builds the app, starts the server under test and sets the environment read here and by the
// fixtures (`tests/fixtures`).
//
// `OVERSEER_E2E_TARGET` picks the server:
// - `openscreeps` (default): a disposable OpenScreeps world. Two projects run every test:
//   `direct` serves the app from a plain static server, `proxy` from `overseer-proxy --static`
//   in front of the game server.
// - `official`: screeps.com, read-only. Only the `proxy` project (screeps.com sends no CORS
//   headers), one worker (its rate limits are per account), no `@openscreeps` tests, and no
//   traces, screenshots or videos (they would record the account's API token and data).
//
// `@screens` tests (reference screenshots) run only when `OVERSEER_E2E_SCREENS` names their set
// (`cargo xtask e2e --screens`).
//
// `OVERSEER_E2E_SLOW=<factor>` emulates the CI runner on a fast machine: Chromium draws WebGL
// with SwiftShader (software, as on the runner; headless Chromium already does, headed runs
// too), every page runs `<factor>` times slower with slower responses (the fixtures, through
// CDP) and two workers run, as in CI.

import { dirname, resolve } from 'node:path';
import { defineConfig, devices, type PlaywrightTestConfig } from '@playwright/test';
import { currentTarget } from './tests/fixtures/api';
import type { Mode } from './tests/fixtures';

const target = currentTarget();
const official = target === 'official';
const ci = Boolean(process.env.CI);
const slow = Number(process.env.OVERSEER_E2E_SLOW ?? 0) > 1;
const directPort = Number(process.env.OVERSEER_E2E_DIRECT_PORT ?? 4610);
const proxyPort = Number(process.env.OVERSEER_E2E_PROXY_PORT ?? 4611);
// `cargo xtask e2e` builds the app into its own `target/e2e/dist`: `app/dist` is the development
// build `trunk serve` rewrites on every change.
const dist = process.env.OVERSEER_E2E_DIST ?? resolve(__dirname, '../target/e2e/dist');
const proxy =
  process.env.OVERSEER_E2E_PROXY ??
  resolve(__dirname, '../target/e2e/overseer-proxy' + (process.platform === 'win32' ? '.exe' : ''));
const upstream = process.env.OVERSEER_LIVE_URL ?? 'http://127.0.0.1:21025/';
// The proxy logs every request; keep them next to the server's log (the harness's
// `<target>/e2e`, also when `CARGO_TARGET_DIR` moves it) instead of the test output.
const proxyLog = resolve(dirname(proxy), official ? 'proxy-official.log' : 'proxy.log');

const directServer: NonNullable<PlaywrightTestConfig['webServer']> = {
  command: 'node static-server.mjs',
  url: `http://127.0.0.1:${directPort}/index.html`,
  env: { OVERSEER_E2E_DIST: dist, OVERSEER_E2E_DIRECT_PORT: String(directPort) },
  reuseExistingServer: false,
};
const proxyServer: NonNullable<PlaywrightTestConfig['webServer']> = {
  command: `"${proxy}" --upstream ${upstream} --listen 127.0.0.1:${proxyPort} --static "${dist}" 2> "${proxyLog}"`,
  url: `http://127.0.0.1:${proxyPort}/overseer-proxy.json`,
  reuseExistingServer: false,
};
const directProject = {
  name: 'direct',
  use: { mode: 'direct' as const, baseURL: `http://127.0.0.1:${directPort}/` },
};
const proxyProject = {
  name: 'proxy',
  use: { mode: 'proxy' as const, baseURL: `http://127.0.0.1:${proxyPort}/` },
};
const excluded = [
  ...(official ? ['@openscreeps'] : []),
  ...(process.env.OVERSEER_E2E_SCREENS ? [] : ['@screens']),
];

export default defineConfig<{ mode: Mode }>({
  testDir: './tests',
  outputDir: './test-results',
  fullyParallel: true,
  forbidOnly: ci,
  retries: ci && !official ? 1 : 0,
  workers: official ? 1 : ci || slow ? 2 : undefined,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  grepInvert: excluded.length > 0 ? new RegExp(excluded.join('|')) : undefined,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  use: {
    ...devices['Desktop Chrome'],
    trace: official ? 'off' : 'retain-on-failure',
    screenshot: official ? 'off' : 'only-on-failure',
    video: 'off',
    launchOptions: slow ? { args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] } : {},
  },
  projects: official ? [proxyProject] : [directProject, proxyProject],
  webServer: official ? [proxyServer] : [directServer, proxyServer],
});
