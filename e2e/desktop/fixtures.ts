// Playwright fixtures of the desktop app's tests: `import { test, expect } from './fixtures';`.
//
// `cargo xtask e2e --desktop` (Windows) builds the Tauri shell for testing and names the
// executable in `OVERSEER_E2E_DESKTOP_APP` (see `xtask/src/e2e/desktop.rs`). The test build has
// its own identifier (the overlay `app/src-tauri/tauri.e2e.conf.json`), so its WebView2 profile
// (`%LOCALAPPDATA%\<identifier>`), saved window state (`%APPDATA%\<identifier>`) and keychain
// entries (service `overseer:<identifier>`) are its own: the installed app's are never read or
// written. Its main window's WebView2 listens for the DevTools protocol on a free port, which
// the profile's `DevToolsActivePort` names; Playwright attaches there (`connectOverCDP`).
//
// - `app`: the test app ({@link DesktopApp}), not started: `launch()` starts it and returns the
//   main window's page, `restart()` closes every window the way the user does (the shell saves
//   the window state as it exits) and starts it again, `popOut()` waits for the window a click
//   opens. Every test starts from nothing (no profile, no window state, no keychain entry) and
//   leaves nothing: the fixture stops the app and removes all three afterwards.
// - `target`, `server`: as in the web tests (`../tests/fixtures`). On `official` (screeps.com,
//   read-only) every launch blocks and fails the test on any request to a server that may change
//   something, and answers "Not now" when the "Rate limit running low" prompt opens. The guard
//   sees what the app sends once Playwright attached, a moment after the launch: the app sends
//   nothing before a test drives it, except resuming a session after a restart (reads only).
// - `credentials`: an account to sign in with through the UI ({@link signIn}): a fresh account
//   on `openscreeps`, the API token's account on `official`.
// - `presence`: `credentials` with a room that shows the account (`home`): a fresh player with a
//   spawn and a bot on `openscreeps` (removed after the test), the start room on `official`.
//
// Windows are moved, resized, maximized, minimized and closed through the shell's window API
// (`geometry`, `place`, `maximize`, `close`): the test build grants it, standing in for the user
// dragging or clicking a window's frame. External links: `interceptOpener`.

import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { closeSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { promisify } from 'node:util';
import {
  chromium,
  expect,
  test as base,
  type Browser,
  type BrowserContext,
  type Page,
} from '@playwright/test';
import {
  api,
  createPlayer,
  currentTarget,
  isMutating,
  liveServer,
  liveToken,
  register,
  removeUser,
  type Server,
  type Target,
} from '../tests/fixtures/api';

export { expect };
export { Session, type Server } from '../tests/fixtures/api';

const run = promisify(execFile);

/** The test build's identifier (the overlay's): never the installed app's. */
export const IDENTIFIER: string = JSON.parse(
  readFileSync(resolve(__dirname, '../../app/src-tauri/tauri.e2e.conf.json'), 'utf8'),
).identifier;
if (!IDENTIFIER || IDENTIFIER === 'com.openscreeps.overseer') {
  throw new Error('the test build needs an identifier of its own');
}

/** The keychain service of the test build (`app/src-tauri/src/secrets.rs`). */
export const SERVICE = `overseer:${IDENTIFIER}`;

/** The origin of the app's pages in the shell on Windows. */
export const ORIGIN = 'http://tauri.localhost';

/** The test build's data directories: its window state, its WebView2 profile. */
const DATA_DIRS = [process.env.APPDATA, process.env.LOCALAPPDATA].map((dir) => {
  if (!dir) throw new Error('APPDATA and LOCALAPPDATA must be set (Windows)');
  return join(dir, IDENTIFIER);
});

/** Where the main window's WebView2 names its DevTools port (first line). */
const PORT_FILE = join(DATA_DIRS[1] ?? '', 'EBWebView', 'DevToolsActivePort');

/** The test app's executable, from the harness. */
function executable(): string {
  const exe = process.env.OVERSEER_E2E_DESKTOP_APP;
  if (!exe) throw new Error('OVERSEER_E2E_DESKTOP_APP is not set: run `cargo xtask e2e --desktop`');
  return exe;
}

/** Polls `probe` every 100 ms until it returns something other than `undefined`. */
async function waitFor<T>(
  what: string,
  probe: () => T | undefined | Promise<T | undefined>,
  timeout = 60_000,
): Promise<T> {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await probe();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`timed out after ${timeout} ms waiting for ${what}`);
    const { promise, resolve } = Promise.withResolvers<void>();
    setTimeout(resolve, 100);
    await promise;
  }
}

/** Whether a DevTools endpoint answers on `port`. */
async function devtoolsAnswer(port: number): Promise<boolean> {
  try {
    return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
  } catch {
    return false;
  }
}

/** The port in `DevToolsActivePort`, once the WebView2 wrote it. */
function devtoolsPort(): number | undefined {
  try {
    const port = Number(readFileSync(PORT_FILE, 'utf8').split('\n')[0]);
    return port > 0 ? port : undefined;
  } catch {
    return undefined;
  }
}

/** The keys (`token:<url>`, `server-password:<url>`) the test build keeps in the keychain. */
export async function keychain(): Promise<string[]> {
  const suffix = `.${SERVICE}`;
  const { stdout } = await run('cmdkey', [`/list:*${suffix}`]);
  // `Target: <key>.<service>` lines (the label may be translated); `User:` names the key alone.
  return stdout
    .split(/\r?\n/)
    .map((line) => line.trim().split(': ')[1]?.trim() ?? '')
    .filter((target) => target.endsWith(suffix))
    .map((target) => target.slice(0, -suffix.length));
}

/** Removes everything the test build keeps: keychain entries and data directories. */
async function wipe(): Promise<void> {
  for (const key of await keychain()) await run('cmdkey', [`/delete:${key}.${SERVICE}`]);
  for (const dir of DATA_DIRS) {
    // WebView2 processes that are shutting down may still hold files.
    rmSync(dir, { recursive: true, force: true, maxRetries: 40, retryDelay: 250 });
  }
}

/** The shell's window API, as `withGlobalTauri` exposes it (the part the tests use). */
interface TauriWindow {
  label: string;
  title(): Promise<string>;
  innerSize(): Promise<{ width: number; height: number }>;
  outerPosition(): Promise<{ x: number; y: number }>;
  minimize(): Promise<void>;
  isMinimized(): Promise<boolean>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  scaleFactor(): Promise<number>;
  setSize(size: unknown): Promise<void>;
  setPosition(position: unknown): Promise<void>;
  maximize(): Promise<void>;
  unmaximize(): Promise<void>;
}

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: Invoke };
      window: { getCurrentWindow(): TauriWindow };
      dpi: {
        LogicalSize: new (width: number, height: number) => unknown;
        LogicalPosition: new (x: number, y: number) => unknown;
      };
    };
    /** The URLs the app asked the shell to open in the system browser (`interceptOpener`). */
    e2eOpened?: string[];
    /** The shell's own `invoke`, while `interceptOpener` stands in for it. */
    e2eInvoke?: Invoke;
  }
}

/** The label of the window showing `page` (`main`, `route-…`); `undefined` until it loaded. */
async function labelOf(page: Page): Promise<string | undefined> {
  if (!page.url().startsWith(ORIGIN)) return undefined;
  return page
    .evaluate(() => window.__TAURI__?.window.getCurrentWindow().label)
    .catch(() => undefined);
}

/** A window's place on the screen, in physical pixels: outer position, inner size. */
export interface Geometry {
  x: number;
  y: number;
  width: number;
  height: number;
  maximized: boolean;
  /** Physical pixels per logical pixel. */
  scale: number;
}

/** Where the window showing `page` is. */
export function geometry(page: Page): Promise<Geometry> {
  return page.evaluate(async () => {
    const window_ = window.__TAURI__!.window.getCurrentWindow();
    const [position, size, maximized, scale] = await Promise.all([
      window_.outerPosition(),
      window_.innerSize(),
      window_.isMaximized(),
      window_.scaleFactor(),
    ]);
    return { x: position.x, y: position.y, width: size.width, height: size.height, maximized, scale };
  });
}

/** Moves and resizes the window showing `page` (logical pixels), as dragging it would. */
export async function place(
  page: Page,
  to: { x: number; y: number; width: number; height: number },
): Promise<Geometry> {
  await page.evaluate(async ({ x, y, width, height }) => {
    const { window: windows, dpi } = window.__TAURI__!;
    const window_ = windows.getCurrentWindow();
    await window_.setSize(new dpi.LogicalSize(width, height));
    await window_.setPosition(new dpi.LogicalPosition(x, y));
  }, to);
  return geometry(page);
}

/** Maximizes (`true`) or restores (`false`) the window showing `page`. */
export async function maximize(page: Page, maximized = true): Promise<void> {
  await page.evaluate(async (maximized) => {
    const window_ = window.__TAURI__!.window.getCurrentWindow();
    await (maximized ? window_.maximize() : window_.unmaximize());
  }, maximized);
  await expect.poll(async () => (await geometry(page)).maximized).toBe(maximized);
}

/** The native title of the window showing `page`. */
export function nativeTitle(page: Page): Promise<string> {
  return page.evaluate(() => window.__TAURI__!.window.getCurrentWindow().title());
}

/**
 * Makes the shell's opener (`plugin:opener|open_url`, the system browser) record the URLs the
 * page asks it to open instead of opening them: a test must not open the user's browser. The
 * app calls the shell through `window.__TAURI__.core.invoke`, looked up at every call; its
 * module objects are frozen, so `window.__TAURI__` is replaced by a copy with a stand-in
 * `invoke`. Fails when the stand-in is not in place (then a click would open the browser for
 * real). Returns a reader of the recorded URLs.
 */
export async function interceptOpener(page: Page): Promise<() => Promise<string[]>> {
  const intercepted = await page.evaluate(() => {
    const tauri = window.__TAURI__!;
    const invoke = tauri.core.invoke;
    const opened: string[] = [];
    const standIn: Invoke = (command, args) => {
      if (command !== 'plugin:opener|open_url') return invoke(command, args);
      opened.push(String(args?.url));
      return Promise.resolve(null);
    };
    window.__TAURI__ = { ...tauri, core: { ...tauri.core, invoke: standIn } };
    window.e2eOpened = opened;
    window.e2eInvoke = invoke;
    return window.__TAURI__.core.invoke === standIn;
  });
  if (!intercepted) throw new Error("cannot stand in for the shell's opener");
  return () => page.evaluate(() => window.e2eOpened ?? []);
}

/** Sets up every page of a launch: its browser context, then each window's page once. */
interface Hooks {
  context?: (context: BrowserContext) => Promise<void>;
  page?: (page: Page) => Promise<void>;
}

/** The test app: one process at a time, started and stopped by the test. */
export class DesktopApp {
  #child: ChildProcess | undefined;
  #exited: Promise<void> = Promise.resolve();
  #browser: Browser | undefined;
  #port = 0;
  readonly #hooks: Hooks[] = [];
  readonly #prepared = new WeakSet<Page>();
  readonly #pidFile = executable().replace(/\.exe$/i, '.pid');
  readonly #log = join(dirname(executable()), 'app.log');

  /** Runs `hooks` on every launch from the next one on. */
  hook(hooks: Hooks): void {
    this.#hooks.push(hooks);
  }

  /** Whether the app runs. */
  get running(): boolean {
    return this.#child !== undefined;
  }

  /** The browser context of the app's webviews (one per launch). */
  get context(): BrowserContext {
    const context = this.#browser?.contexts()[0];
    if (!context) throw new Error('the app is not running');
    return context;
  }

  /** Starts the app; returns the main window's page once the app's page loaded in it. */
  async launch(): Promise<Page> {
    if (this.#child) throw new Error('the app already runs');
    // A port file of the last launch would name a closed port.
    rmSync(PORT_FILE, { force: true });
    const log = openSync(this.#log, 'a');
    const child = spawn(executable(), [], { stdio: ['ignore', log, log] });
    closeSync(log);
    this.#child = child;
    const exited = Promise.withResolvers<void>();
    child.once('exit', () => exited.resolve());
    this.#exited = exited.promise;
    // For the harness: it stops an app a crashed test run left behind.
    writeFileSync(this.#pidFile, String(child.pid));
    this.#port = await waitFor('the app to open its DevTools port', () => {
      if (child.exitCode !== null) throw new Error(`the app exited (${child.exitCode})`);
      return devtoolsPort();
    });
    this.#browser = await chromium.connectOverCDP(`http://127.0.0.1:${this.#port}`);
    for (const { context } of this.#hooks) await context?.(this.context);
    return this.window('main');
  }

  /** The page of the window labelled `label`, once the app loaded in it. */
  async window(label: string): Promise<Page> {
    const page = await waitFor(`the ${label} window`, async () => {
      for (const page of this.context.pages()) {
        if ((await labelOf(page)) === label) return page;
      }
      return undefined;
    });
    return this.#prepare(page);
  }

  /** Runs `open` (a click) and returns the page of the window it opens. */
  async popOut(open: () => Promise<void>): Promise<Page> {
    const opened = this.context.waitForEvent('page');
    await open();
    const page = await opened;
    await page.waitForURL(`${ORIGIN}/**`);
    await page.waitForFunction(() => window.__TAURI__ !== undefined);
    return this.#prepare(page);
  }

  async #prepare(page: Page): Promise<Page> {
    if (!this.#prepared.has(page)) {
      this.#prepared.add(page);
      for (const hooks of this.#hooks) await hooks.page?.(page);
    }
    return page;
  }

  /**
   * Closes every window as the user does with its close button (the shell's `close`, granted
   * to the test build): the app exits once the last one closed and the shell saves the window
   * state on its way out. Kills the app when it does not exit.
   */
  async close(): Promise<void> {
    const child = this.#child;
    if (!child) return;
    for (const page of this.context.pages()) {
      if (!page.url().startsWith(ORIGIN)) continue;
      // Not awaited in the page: the window goes away with it.
      await page
        .evaluate(() => void window.__TAURI__?.window.getCurrentWindow().close())
        .catch(() => {});
    }
    try {
      await waitFor(
        'the app to exit once its windows closed',
        () => (child.exitCode === null && child.signalCode === null ? undefined : true),
        30_000,
      );
    } catch (error) {
      await this.kill();
      throw error;
    }
    await this.#gone();
  }

  /** Stops the app and its WebView2 processes at once. */
  async kill(): Promise<void> {
    const child = this.#child;
    if (!child) return;
    if (child.exitCode === null) {
      await run('taskkill', ['/F', '/T', '/PID', String(child.pid)]).catch(() => {});
    }
    await this.#exited;
    await this.#gone();
  }

  /** Closes the app and starts it again; returns the new main window's page. */
  async restart(): Promise<Page> {
    await this.close();
    return this.launch();
  }

  /** After the app exited: waits until its WebView2 is gone too (a new launch would join it). */
  async #gone(): Promise<void> {
    // The browser is gone or going: this only drops the connection.
    await this.#browser?.close().catch(() => {});
    await waitFor(
      "the app's WebView2 to exit",
      async () => ((await devtoolsAnswer(this.#port)) ? undefined : true),
      30_000,
    );
    rmSync(this.#pidFile, { force: true });
    this.#child = undefined;
    this.#browser = undefined;
  }
}

/** How a test signs in through the UI. */
export type Credentials =
  | { kind: 'password'; username: string; login: string; password: string }
  | { kind: 'token'; username: string; token: string };

/** A room that shows the account. */
export interface Home {
  room: string;
  /** Its shard on a sharded server (screeps.com); `null` on a single-shard server. */
  shard: string | null;
}

/** `credentials` and their `home`. */
export interface Presence {
  credentials: Credentials;
  home: Home;
}

/** The API token's account and start room on screeps.com, asked once per worker. */
let official: Promise<Presence> | undefined;

function officialPresence(server: Server, token: string): Promise<Presence> {
  official ??= (async () => {
    const { body: me } = await api<{ username: string }>(server.url, 'api/auth/me', { token });
    const { body: start } = await api<{ room: string[] }>(server.url, 'api/user/world-start-room', {
      token,
    });
    // `shard3/W6S2` on a sharded server.
    const [shard, room] = start.room[0]?.includes('/')
      ? (start.room[0].split('/') as [string, string])
      : [null, start.room[0]];
    if (!room) throw new Error('screeps.com names no start room for the account');
    return { credentials: { kind: 'token', username: me.username, token }, home: { room, shard } };
  })();
  return official;
}

/** The status bar of `page`. */
export const statusBar = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });

/** The app's route `hash` (`#/…`) in `page`, loaded afresh. */
export async function open(page: Page, hash: string): Promise<void> {
  await page.goto(`${ORIGIN}/index.html${hash}`);
}

/**
 * Signs in to `server` through the UI: adds it first when it is a private server, then fills
 * its sign-in form with `credentials`; done when the account menu names the user.
 */
export async function signIn(page: Page, server: Server, credentials: Credentials): Promise<void> {
  await open(page, '#/');
  const servers = page.getByRole('region', { name: 'Servers' });
  if (server.kind === 'private') {
    await servers.getByRole('button', { name: 'Add server' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add server' });
    await dialog.getByLabel('Name', { exact: true }).fill(server.name);
    await dialog.getByLabel('Address', { exact: true }).fill(server.url);
    await dialog.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(dialog).toBeHidden();
  }
  await page.getByRole('button', { name: `Sign in ${server.name}` }).click();
  const form = page.getByRole('region', { name: `Sign in to ${server.name}` });
  if (credentials.kind === 'token') {
    await form.getByRole('button', { name: 'API token', exact: true }).click();
    await expect(form.getByText('It is kept in the system keychain.')).toBeVisible();
    await form.getByLabel('API token', { exact: true }).fill(credentials.token);
  } else {
    await form.getByLabel('Username or e-mail').fill(credentials.login);
    await form.getByLabel('Password', { exact: true }).fill(credentials.password);
  }
  await form.getByRole('button', { name: 'Sign in', exact: true }).last().click();
  await expect(page.getByRole('button', { name: `Account: ${credentials.username}` })).toBeVisible({
    timeout: 30_000,
  });
}

/** Whether `url` is a request to a game server (through the shell's proxy or not). */
function toServer(url: string): boolean {
  const { protocol, hostname } = new URL(url);
  return (
    (protocol === 'http:' || protocol === 'https:') &&
    hostname !== 'tauri.localhost' &&
    hostname !== 'ipc.localhost'
  );
}

export const test = base.extend<
  {
    app: DesktopApp;
    cleanup: (() => Promise<void>)[];
    credentials: Credentials;
    presence: Presence;
  },
  { target: Target; server: Server; apiToken: string | undefined }
>({
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  target: [async ({}, use) => use(currentTarget()), { scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  server: [async ({}, use) => use(liveServer()), { scope: 'worker' }],
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  apiToken: [async ({}, use) => use(liveToken()), { scope: 'worker' }],
  // Set up before the app (which depends on it), so it runs after the app stopped.
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures destructure their dependencies.
  cleanup: async ({}, use) => {
    const tasks: (() => Promise<void>)[] = [];
    await use(tasks);
    for (const task of tasks.reverse()) await task();
  },
  app: async ({ cleanup, target }, use, testInfo) => {
    void cleanup;
    await wipe();
    const app = new DesktopApp();
    const refused = new Set<string>();
    if (target === 'official') {
      app.hook({
        context: async (context) => {
          context.on('request', (request) => {
            if (toServer(request.url()) && isMutating(request.method(), request.url())) {
              refused.add(`${request.method()} ${new URL(request.url()).pathname}`);
            }
          });
          await context.route(toServer, (route) => {
            const request = route.request();
            return isMutating(request.method(), request.url())
              ? route.abort('blockedbyclient')
              : route.fallback();
          });
        },
        // The account's real budget runs low across runs: "Not now" dismisses the prompt to
        // lift the limits on the website (another may follow at once).
        page: (page) =>
          page.addLocatorHandler(
            page.getByRole('dialog', { name: 'Rate limit running low' }),
            (dialog) => dialog.getByRole('button', { name: 'Not now' }).click(),
            { noWaitAfter: true },
          ),
      });
    }
    try {
      await use(app);
      // No screenshots of screeps.com: they would show the account.
      if (app.running && target !== 'official' && testInfo.status !== testInfo.expectedStatus) {
        const page = await app.window('main').catch(() => undefined);
        const shot = await page?.screenshot().catch(() => undefined);
        if (shot) await testInfo.attach('main window', { body: shot, contentType: 'image/png' });
      }
    } finally {
      await app.kill();
      await wipe();
    }
    expect([...refused], 'requests that may change the screeps.com account').toEqual([]);
  },
  credentials: async ({ target, server, apiToken }, use) => {
    if (target === 'official') {
      if (!apiToken) throw new Error('OVERSEER_LIVE_TOKEN is not set');
      await use((await officialPresence(server, apiToken)).credentials);
      return;
    }
    const account = await register(server.url);
    const { username, password } = account;
    await use({ kind: 'password', username, login: username, password });
  },
  presence: async ({ target, server, apiToken, cleanup }, use) => {
    if (target === 'official') {
      if (!apiToken) throw new Error('OVERSEER_LIVE_TOKEN is not set');
      await use(await officialPresence(server, apiToken));
      return;
    }
    const player = await createPlayer(server.url);
    cleanup.push(() => removeUser(server, player.id));
    await use({
      credentials: {
        kind: 'password',
        username: player.username,
        login: player.username,
        password: player.password,
      },
      home: { room: player.room, shard: null },
    });
  },
});
