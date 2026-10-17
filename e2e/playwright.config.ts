// UI end-to-end tests of the Overseer web app (Chromium). Run them with `cargo xtask e2e`, which
// builds the app, starts the server under test and sets the environment read here and by the
// fixtures (`tests/fixtures`).
//
// `OVERSEER_E2E_TARGET` picks the server:
// - `openscreeps` (default): a disposable OpenScreeps world. Two projects run every test:
//   `direct` serves the app from a plain static server, `proxy` from `overseer-proxy --static`
//   in front of the game server.
// - `community`: a disposable world of the stock community server (screeps-launcher with
//   screepsmod-auth and screepsmod-admin-utils). Only the `proxy` project; retries, workers
//   and traces as on `openscreeps`.
// - `official`: screeps.com, read-only. Only the `proxy` project (screeps.com sends no CORS
//   headers), one worker (its rate limits are per account), no `@openscreeps` tests, and no
//   traces, screenshots or videos (they would record the account's API token and data).
//
// Each project first runs its cache setup (`<project>-cache`, `tests/cache.setup.ts`): the
// app's world cache loaded once and copied into every test context (the fixtures' `cache`).
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
import type { Cache, Mode } from './tests/fixtures';

const target = currentTarget();
const official = target === 'official';
// Stock community servers send no CORS headers unless screepsmod-admin-utils' `cors.origins`
// lists the page's origin: browsers reach them through `overseer-proxy`, as the desktop app does.
const proxyOnly = official || target === 'community';
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
const proxyLog = resolve(dirname(proxy), {
  openscreeps: 'proxy.log',
  official: 'proxy-official.log',
  community: 'proxy-community.log',
}[target]);

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
// Each project's world cache, loaded once per run by its setup project (`tests/cache.setup.ts`)
// and copied into every test context (the fixtures' `cache` option). Kept next to the logs: the
// official one seeds the next official run.
const cacheFile = (project: string) => resolve(dirname(proxy), `cache-${target}-${project}.json`);
/** A project serving the app in `mode` at `port`, after its cache setup. */
function served(mode: Mode, port: number) {
  const use = { mode, baseURL: `http://127.0.0.1:${port}/`, cacheFile: cacheFile(mode) };
  return [
    {
      name: `${mode}-cache`,
      testMatch: /cache\.setup\.ts$/,
      use: { ...use, cache: 'previous' as const },
    },
    { name: mode, dependencies: [`${mode}-cache`], use },
  ];
}
const directProjects = served('direct', directPort);
const proxyProjects = served('proxy', proxyPort);
const excluded = [
  ...(official ? ['@openscreeps'] : []),
  ...(process.env.OVERSEER_E2E_SCREENS ? [] : ['@screens']),
];

export default defineConfig<{ mode: Mode; cache: Cache; cacheFile: string | undefined }>({
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
  projects: proxyOnly ? proxyProjects : [...directProjects, ...proxyProjects],
  webServer: proxyOnly ? [proxyServer] : [directServer, proxyServer],
});
