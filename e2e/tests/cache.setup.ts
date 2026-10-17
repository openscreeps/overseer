// The `cache` setup of each project (`<project>-cache` in `playwright.config.ts`): opens the
// server's world once per run the way the tests do, lets the app load it and saves the app's
// world cache (`IndexedDB`: terrain, overviews, portals, badges) to the project's `cacheFile`.
// Every test context of the project then starts from a copy of it (the `cache` fixture option)
// instead of loading the same rooms again. localStorage is not saved: no profile, token or
// camera leaks into the tests.
//
// - `openscreeps`, `community`: the whole disposable world (11 × 11 rooms on `openscreeps`), for
//   the tests that zoom out, until its loading progress is gone and its requests stopped.
// - `official`: the account's home room on screeps.com (thousands of rooms per shard), until
//   the terrain and owners around it are known. The portal scan and the portal destinations,
//   paced over many minutes by the rate limits, are not waited for: they go on in the tests as
//   before. The setup starts from the previous official run's cache (`cache: 'previous'`), so
//   what the cache holds is not fetched again.
//
// Either way the wait is bounded (`LIMIT_MS`): past it, what loaded so far is saved.

import { mkdirSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import type { Page } from '@playwright/test';
import { expect, signInReader, test } from './fixtures';

/** The longest the setup waits for the world to load. */
const LIMIT_MS = 2 * 60_000;
/**
 * How long the loaded state must hold: the app writes what it fetched a second later
 * (`FLUSH_DELAY` in `app/src/terrain_cache.rs`).
 */
const QUIET_MS = 3_000;

/** Records per store of the app's world cache; fails naming the step that took over 20 s. */
function cachedRecords(page: Page): Promise<Record<string, number>> {
  return page.evaluate(async () => {
    const within = <T>(
      step: string,
      start: (done: (value: T) => void, fail: (error: unknown) => void) => void,
    ) => {
      const { promise, resolve, reject } = Promise.withResolvers<T>();
      const timer = setTimeout(() => reject(new Error(`IndexedDB: ${step} took over 20 s`)), 20_000);
      start(
        (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        (error) => {
          clearTimeout(timer);
          reject(error instanceof Error ? error : new Error(`IndexedDB: ${step} failed`));
        },
      );
      return promise;
    };
    const db = await within<IDBDatabase>('open', (done, fail) => {
      const open = indexedDB.open('overseer-terrain');
      open.onsuccess = () => done(open.result);
      open.onerror = () => fail(open.error);
      open.onblocked = () => fail(new Error('IndexedDB: open is blocked'));
    });
    const counts: Record<string, number> = {};
    try {
      for (const name of Array.from(db.objectStoreNames)) {
        counts[name] = await within<number>(`count of ${name}`, (done, fail) => {
          const request = db.transaction(name).objectStore(name).count();
          request.onsuccess = () => done(request.result);
          request.onerror = () => fail(request.error);
        });
      }
    } finally {
      db.close();
    }
    return counts;
  });
}

test('loads the world into the cache', async ({ page, server, apiToken, target, cacheFile }) => {
  test.setTimeout(LIMIT_MS + 3 * 60_000);
  if (!cacheFile) throw new Error('the project names no cacheFile');
  const official = target === 'official';
  // The app's API traffic (the socket stays open, so it does not count).
  let inFlight = 0;
  let lastActivity = Date.now();
  const counted = (url: string) => {
    const { pathname } = new URL(url);
    return pathname.startsWith('/api/') || pathname.startsWith('/room-history/');
  };
  page.on('request', (request) => {
    if (!counted(request.url())) return;
    inFlight += 1;
    lastActivity = Date.now();
  });
  for (const event of ['requestfinished', 'requestfailed'] as const) {
    page.on(event, (request) => {
      if (!counted(request.url())) return;
      inFlight -= 1;
      lastActivity = Date.now();
    });
  }

  const status = page.getByRole('contentinfo', { name: 'Status' });
  const loading = status.getByTitle(/^Loading the world/);
  const deadline = Date.now() + LIMIT_MS;
  let label = '';
  /** Whether the loading progress `label` says everything waited for is known. */
  const loaded = (label: string) => {
    if (label === '' || /waiting for the rate limit/i.test(label)) return true;
    if (!official) return false;
    // `Terrain 12 345 / 25 000 · Owners 2 500 / 25 000 · Portals 40 / 331`: terrain and owners.
    const digits = (text: string) => text.replace(/\D/g, '');
    return [...label.matchAll(/(?:Terrain|Owners) ([\d\s,.]+?) \/ ([\d\s,.]+?)(?: ·|$)/gu)].every(
      ([, done, wanted]) => digits(done ?? '') === digits(wanted ?? ''),
    );
  };
  /** Waits until the app loaded what the map shows, or the deadline passed; logs how. */
  const settle = async (step: string) => {
    const started = Date.now();
    await expect(status).toContainText('Connected', { timeout: 60_000 });
    await expect(status.getByTitle('Zoom level', { exact: true })).toHaveText(/\S/, {
      timeout: 60_000,
    });
    let loadedSince: number | null = null;
    // Before the world starts loading the status bar shows no progress either: on screeps.com
    // it must have shown once (or none for a while: a warm start).
    let shown = false;
    for (;;) {
      label = (await loading.allInnerTexts()).join(' ');
      const now = Date.now();
      shown ||= label !== '';
      loadedSince = loaded(label) ? (loadedSince ?? now) : null;
      const held = loadedSince !== null && now - loadedSince >= QUIET_MS;
      const done = official
        ? held && (shown || now - started >= 15_000)
        : held && inFlight === 0 && now - lastActivity >= QUIET_MS;
      if (done || now > deadline) {
        const seconds = ((now - started) / 1000).toFixed(0);
        console.log(`cache ${step}: ${done ? 'loaded' : `stopped: ${label}`} after ${seconds} s`);
        return;
      }
      await page.waitForTimeout(500);
    }
  };

  const home = await signInReader(page, server, apiToken);
  if (home) {
    // Where the tests on screeps.com go (a first visit opens the same room).
    await page.goto(home.path);
    await settle('home');
  } else {
    // A first visit, as every test's: the map on the account's start room; then all of it.
    await settle('first visit');
    await page
      .getByRole('group', { name: 'Zoom' })
      .getByRole('button', { name: 'Show the whole world' })
      .click();
    await settle('whole world');
  }
  // Leaving the app ends its loading and writing (the portal scan goes on on screeps.com), so
  // the cache is read without competing with it: a static file of the same origin.
  await page.goto('/icon.svg');
  const records = await cachedRecords(page);
  expect(records.terrain ?? 0, 'rooms with cached terrain').toBeGreaterThan(0);
  test.info().annotations.push({ type: 'cache', description: JSON.stringify(records) });

  const origin = new URL(page.url()).origin;
  const state = await page.context().storageState({ indexedDB: true });
  const saved = {
    cookies: [],
    origins: state.origins
      .filter((entry) => entry.origin === origin)
      .map((entry) => ({ ...entry, localStorage: [] })),
  };
  mkdirSync(dirname(cacheFile), { recursive: true });
  writeFileSync(`${cacheFile}.tmp`, JSON.stringify(saved));
  renameSync(`${cacheFile}.tmp`, cacheFile);
});
