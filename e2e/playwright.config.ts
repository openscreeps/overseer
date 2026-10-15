// UI end-to-end tests of the Overseer web app (Chromium). Run them with `cargo xtask e2e`, which
// builds the app, starts a disposable game server and sets the environment read here and by the
// fixtures (`tests/fixtures`). Two projects run every test: `direct` serves the app from a plain
// static server, `proxy` from `overseer-proxy --static` in front of the game server.

import { resolve } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
import type { Mode } from './tests/fixtures';

const ci = Boolean(process.env.CI);
const directPort = Number(process.env.OVERSEER_E2E_DIRECT_PORT ?? 4610);
const proxyPort = Number(process.env.OVERSEER_E2E_PROXY_PORT ?? 4611);
const dist = process.env.OVERSEER_E2E_DIST ?? resolve(__dirname, '../app/dist');
const proxy =
  process.env.OVERSEER_E2E_PROXY ??
  resolve(__dirname, '../target/e2e/overseer-proxy' + (process.platform === 'win32' ? '.exe' : ''));
const upstream = process.env.OVERSEER_LIVE_URL ?? 'http://127.0.0.1:21025/';
// The proxy logs every request; keep them next to the server's log instead of the test output.
const proxyLog = resolve(__dirname, '../target/e2e/proxy.log');

export default defineConfig<{ mode: Mode }>({
  testDir: './tests',
  outputDir: './test-results',
  fullyParallel: true,
  forbidOnly: ci,
  retries: ci ? 1 : 0,
  workers: ci ? 2 : undefined,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  use: {
    ...devices['Desktop Chrome'],
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'direct', use: { mode: 'direct', baseURL: `http://127.0.0.1:${directPort}/` } },
    { name: 'proxy', use: { mode: 'proxy', baseURL: `http://127.0.0.1:${proxyPort}/` } },
  ],
  webServer: [
    {
      command: 'node static-server.mjs',
      url: `http://127.0.0.1:${directPort}/index.html`,
      env: { OVERSEER_E2E_DIST: dist, OVERSEER_E2E_DIRECT_PORT: String(directPort) },
      reuseExistingServer: false,
    },
    {
      command: `"${proxy}" --upstream ${upstream} --listen 127.0.0.1:${proxyPort} --static "${dist}" 2> "${proxyLog}"`,
      url: `http://127.0.0.1:${proxyPort}/overseer-proxy.json`,
      reuseExistingServer: false,
    },
  ],
});
