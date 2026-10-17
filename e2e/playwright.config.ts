// UI end-to-end tests of the Overseer web app (Chromium). Run them with `cargo xtask e2e`, which
// builds the app, starts the server under test and sets the environment read here and by the
// fixtures (`tests/fixtures`).
//
// `OVERSEER_E2E_TARGET` picks the server:
// - `openscreeps` (default): a disposable OpenScreeps world. The `direct` project serves the app
//   from a plain static server and runs every test. The `proxy` project serves it from
//   `overseer-proxy --static` in front of the game server and runs only the `@transport` tests
//   (how the app reaches the server: sign-in, session, socket). The other tests do not depend on
//   it, and the `community` and `official` runs put every test through the proxy.
// - `community`: a disposable world of the stock community server (screeps-launcher with
//   screepsmod-auth and screepsmod-admin-utils). Only the `proxy` project, every test; workers
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

import { resolve } from 'node:path';
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
// The run directory of `cargo xtask e2e`: the app's own bundle (`dist/`; `app/dist` is the
// development build `trunk serve` rewrites on every change), the proxy binary, the world caches,
// the logs and Playwright's output. Runs at the same time each have their own.
const output = process.env.OVERSEER_E2E_OUTPUT ?? resolve(__dirname, '../target/e2e', target);
const dist = resolve(output, 'dist');
const proxy = resolve(output, 'overseer-proxy' + (process.platform === 'win32' ? '.exe' : ''));
const upstream = process.env.OVERSEER_LIVE_URL ?? 'http://127.0.0.1:21025/';
// The proxy logs every request; keep them next to the server's log, not in the test output.
const proxyLog = resolve(output, 'proxy.log');

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
// and copied into every test context (the fixtures' `cache` option). The official run's
// directory stays, so its cache seeds the next official run.
const cacheFile = (project: string) => resolve(output, `cache-${project}.json`);
/**
 * A project serving the app in `mode` at `port`, after its cache setup; only the tests that
 * `grep` matches, when given.
 */
function served(mode: Mode, port: number, grep?: RegExp) {
  const use = { mode, baseURL: `http://127.0.0.1:${port}/`, cacheFile: cacheFile(mode) };
  return [
    {
      name: `${mode}-cache`,
      testMatch: /cache\.setup\.ts$/,
      use: { ...use, cache: 'previous' as const },
    },
    { name: mode, dependencies: [`${mode}-cache`], use, grep },
  ];
}
const directProjects = served('direct', directPort);
// The reference screenshots (`--screens`) come from the `proxy` project: all of them.
const proxyProjects = served(
  'proxy',
  proxyPort,
  proxyOnly || process.env.OVERSEER_E2E_SCREENS ? undefined : /@transport/,
);
const excluded = [
  ...(official ? ['@openscreeps'] : []),
  ...(process.env.OVERSEER_E2E_SCREENS ? [] : ['@screens']),
];

export default defineConfig<{ mode: Mode; cache: Cache; cacheFile: string | undefined }>({
  testDir: './tests',
  outputDir: resolve(output, 'test-results'),
  fullyParallel: true,
  forbidOnly: ci,
  // No retries: a test that fails only sometimes fails the run, so that its cause is found.
  retries: 0,
  workers: official ? 1 : ci || slow ? 2 : undefined,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  grepInvert: excluded.length > 0 ? new RegExp(excluded.join('|')) : undefined,
  reporter: [['list'], ['html', { open: 'never', outputFolder: resolve(output, 'playwright-report') }]],
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
