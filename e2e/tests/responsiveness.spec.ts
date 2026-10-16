// The page stays responsive while a whole shard loads in the background: terrain, owners and the
// portal scan of every room (screeps.com's `shard0`: 33 124 rooms, about 500 `roomMap2` answers a
// second) must never hold input or frames for long.
//
// The disposable world is 11 × 11 rooms, so the first test makes it look like `shard0`: the page
// is told the world is 182 × 182 rooms, rooms outside the real world get a real room's terrain
// and a made-up overview (some owned), and their `roomMap2` scans are answered by the test after
// 0.3 to 0.7 s, as screeps.com answers them; the real world's rooms stay real. On the official
// target the second test loads the real `shard0` (cold: a fresh browser profile).
//
// While the world loads behind a room-level view, the test measures what a user would feel: how
// long a plain `page.evaluate` takes to come back (it waits behind whatever the main thread is
// doing) and the main thread's long tasks (over 50 ms, `PerformanceObserver`).

import type { Page } from '@playwright/test';
import { currentTarget, expect, test, type Server } from './fixtures';

/** How many times slower the pages run (`OVERSEER_E2E_SLOW`), as the fixtures apply it. */
const SLOWDOWN = Math.max(1, Number(process.env.OVERSEER_E2E_SLOW ?? 1) || 1);

/** Rooms per side of the made-up shard (screeps.com's `shard0`). */
const SIDE = 182;

/** Rooms scanned before measuring: the first overviews and terrain are in by then. */
const WARM_UP = 1_000;

/** Rooms scanned while measuring (about 20 s at screeps.com's pace). */
const MEASURED = 10_000;

/** Slowest `page.evaluate` round trip allowed while the world loads (screeps.com: 6 s before). */
const MAX_ROUND_TRIP_MS = 300 * SLOWDOWN;

/** 95th percentile of the round trips allowed. */
const P95_ROUND_TRIP_MS = 100 * SLOWDOWN;

/** Share of the measured time the main thread may spend in long tasks. */
const LONG_TASK_SHARE = 0.1;

declare global {
  interface Window {
    /** Long tasks since the page loaded: start and duration, in milliseconds. */
    e2eLongTasks?: [number, number][];
  }
}

/** Records the page's long tasks in `window.e2eLongTasks` from its first script on. */
async function recordLongTasks(page: Page): Promise<void> {
  await page.addInitScript(() => {
    window.e2eLongTasks = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        window.e2eLongTasks?.push([entry.startTime, entry.duration]);
      }
    }).observe({ type: 'longtask', buffered: true });
  });
}

/** `[x, y]` of a room name such as `W3N2` (`W0` is x = -1, `N0` is y = -1). */
function coords(name: string): [number, number] | null {
  const match = /^([WE])(\d+)([NS])(\d+)$/.exec(name);
  if (!match) return null;
  const x = Number(match[2]);
  const y = Number(match[4]);
  return [match[1] === 'W' ? -x - 1 : x, match[3] === 'N' ? -y - 1 : y];
}

/** A small stable hash of a room name, to vary the made-up rooms. */
function hash(name: string): number {
  let value = 0;
  for (const char of name) value = (value * 31 + char.charCodeAt(0)) >>> 0;
  return value;
}

/** A made-up room's map view: portals stand in sector centres, as on screeps.com. */
function roomMap(name: string): Record<string, number[][]> {
  const [x, y] = coords(name) ?? [0, 0];
  const centre = ((x % 10) + 10) % 10 === 5 && ((y % 10) + 10) % 10 === 5;
  return { w: [[0, 0]], r: [], pb: [], p: centre ? [[25, 25]] : [], s: [], c: [], m: [], k: [] };
}

/** The made-up players owning made-up rooms. */
const USERS = Object.fromEntries(
  Array.from({ length: 40 }, (_, index) => [
    `e2e-user-${index}`,
    {
      _id: `e2e-user-${index}`,
      username: `Player${index}`,
      badge: { type: (index % 20) + 1, color1: '#a03030', color2: '#30a030', color3: '#3030a0', param: 0, flip: false },
    },
  ]),
);

/**
 * Makes the page see the disposable world as a 182 × 182 shard (see the file's comment).
 * Returns how many made-up rooms were scanned so far.
 */
async function fakeShard(page: Page): Promise<() => number> {
  let real: ((name: string) => boolean) | undefined;
  let terrains: string[] = [];
  let scanned = 0;
  const isReal = (name: string) => real?.(name) ?? true;

  await page.route(
    (url) => url.pathname.endsWith('/api/game/world-size'),
    async (route) => {
      const response = await route.fetch();
      const { width, height } = (await response.json()) as { width: number; height: number };
      // The real world, centred on the zero lines like screeps.com's.
      real = (name) => {
        const at = coords(name);
        if (!at) return true;
        const [x, y] = at;
        return (
          x >= -Math.ceil(width / 2) &&
          x < Math.floor(width / 2) &&
          y >= -Math.ceil(height / 2) &&
          y < Math.floor(height / 2)
        );
      };
      await route.fulfill({ json: { ok: 1, width: SIDE, height: SIDE } });
    },
  );
  await page.route(
    (url) => url.pathname.endsWith('/api/game/rooms'),
    async (route) => {
      const { rooms } = route.request().postDataJSON() as { rooms: string[] };
      const response = await route.fetch();
      const body = (await response.json()) as { rooms: { room: string; terrain: string }[] };
      if (terrains.length === 0) terrains = body.rooms.map((room) => room.terrain);
      const known = new Set(body.rooms.map((room) => room.room));
      const made = rooms
        .filter((room) => !known.has(room) && !isReal(room) && terrains.length > 0)
        .map((room) => ({
          _id: room,
          room,
          type: 'terrain',
          terrain: terrains[hash(room) % terrains.length],
        }));
      await route.fulfill({ json: { ...body, rooms: [...body.rooms, ...made] } });
    },
  );
  await page.route(
    (url) => url.pathname.endsWith('/api/game/map-stats'),
    async (route) => {
      const response = await route.fetch();
      const body = (await response.json()) as {
        stats: Record<string, unknown>;
        users: Record<string, unknown>;
      };
      const { rooms } = route.request().postDataJSON() as { rooms: string[] };
      for (const room of rooms) {
        if (isReal(room) || room in body.stats) continue;
        const value = hash(room);
        const stat: Record<string, unknown> = {
          status: 'normal',
          minerals0: { type: 'H', density: 1 + (value % 4) },
        };
        if (value % 13 === 0) stat.own = { user: `e2e-user-${value % 40}`, level: 1 + (value % 8) };
        body.stats[room] = stat;
      }
      await route.fulfill({ json: { ...body, users: { ...USERS, ...body.users } } });
    },
  );
  await page.route(
    (url) => url.pathname.endsWith('/api/game/room-objects'),
    async (route) => {
      const room = new URL(route.request().url()).searchParams.get('room') ?? '';
      if (isReal(room)) return route.fallback();
      return route.fulfill({ json: { ok: 1, objects: [], users: {} } });
    },
  );
  await page.routeWebSocket(/\/socket\/websocket/, (ws) => {
    const server = ws.connectToServer();
    const waiting = new Map<string, NodeJS.Timeout>();
    ws.onMessage((message) => {
      const text = String(message);
      const subscribe = /^subscribe roomMap2:(?:[^/]+\/)?([WE]\d+[NS]\d+)$/.exec(text);
      if (subscribe && !isReal(subscribe[1])) {
        const room = subscribe[1];
        const channel = text.slice('subscribe '.length);
        const answer = setTimeout(() => {
          waiting.delete(channel);
          scanned += 1;
          ws.send(JSON.stringify([channel, roomMap(room)]));
        }, 300 + Math.random() * 400);
        waiting.set(channel, answer);
        return;
      }
      const unsubscribe = /^unsubscribe roomMap2:(?:[^/]+\/)?([WE]\d+[NS]\d+)$/.exec(text);
      if (unsubscribe && !isReal(unsubscribe[1])) {
        const channel = text.slice('unsubscribe '.length);
        clearTimeout(waiting.get(channel));
        waiting.delete(channel);
        return;
      }
      server.send(message);
    });
    ws.onClose(() => {
      for (const answer of waiting.values()) clearTimeout(answer);
      waiting.clear();
    });
  });
  return () => scanned;
}

/** What the main thread did while the world loaded. */
interface Measurement {
  /** Every `page.evaluate` round trip, in milliseconds. */
  roundTrips: number[];
  /** Time in long tasks, in milliseconds. */
  longTaskMs: number;
  /** The longest task, in milliseconds. */
  longestTaskMs: number;
  /** How long the measurement took, in milliseconds. */
  elapsedMs: number;
}

/**
 * What the main thread did while `loading()` was true: a round trip every 250 ms (a sampling
 * interval, not a wait for the app) and the long tasks meanwhile.
 */
async function measure(page: Page, loading: () => boolean): Promise<Measurement> {
  const since = await page.evaluate(() => performance.now());
  const started = Date.now();
  const roundTrips: number[] = [];
  while (loading()) {
    const sent = Date.now();
    await page.evaluate(() => 0);
    roundTrips.push(Date.now() - sent);
    await page.waitForTimeout(250);
  }
  const tasks = await page.evaluate(
    (from) => (window.e2eLongTasks ?? []).filter(([start]) => start >= from),
    since,
  );
  return {
    roundTrips,
    longTaskMs: tasks.reduce((total, [, duration]) => total + duration, 0),
    longestTaskMs: Math.max(0, ...tasks.map(([, duration]) => duration)),
    elapsedMs: Date.now() - started,
  };
}

/** Checks the measurements against the limits, with the numbers in the message. */
function expectResponsive(result: Measurement): void {
  const sorted = [...result.roundTrips].sort((a, b) => a - b);
  const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? 0;
  const max = sorted.at(-1) ?? 0;
  const summary = JSON.stringify({ ...result, roundTrips: undefined, p95, max, samples: sorted.length });
  expect(sorted.length, summary).toBeGreaterThan(10);
  expect(max, `slowest round trip: ${summary}`).toBeLessThanOrEqual(MAX_ROUND_TRIP_MS);
  expect(p95, `95th percentile round trip: ${summary}`).toBeLessThanOrEqual(P95_ROUND_TRIP_MS);
  expect(result.longTaskMs, `time in long tasks: ${summary}`).toBeLessThanOrEqual(
    result.elapsedMs * LONG_TASK_SHARE,
  );
}

/** Opens `room` on the map and waits until the session is connected. */
async function openRoom(page: Page, server: Server, room: string, shard: string | null) {
  await page.goto(`/#/${server.slug}/${shard ?? '-'}/${room}`);
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Connected', {
    timeout: 30_000,
  });
  await expect(page.getByTestId('world-map')).toBeVisible();
}

test.describe('responsiveness', () => {
  test('a whole shard loading in the background never holds the page', { tag: '@openscreeps' }, async ({
    signedInPage: page,
    server,
    home,
  }) => {
    test.setTimeout(240_000);
    await recordLongTasks(page);
    const scanned = await fakeShard(page);
    await openRoom(page, server, home.room, home.shard);
    await expect.poll(scanned, { timeout: 60_000 * SLOWDOWN }).toBeGreaterThanOrEqual(WARM_UP);
    const result = await measure(page, () => scanned() < WARM_UP + MEASURED);
    expectResponsive(result);
  });

  test('screeps.com’s shard loading in the background never holds the page', async ({
    signedInPage: page,
    server,
    home,
  }) => {
    test.skip(currentTarget() !== 'official', 'the disposable world is small: the test above makes it a shard');
    test.setTimeout(240_000);
    await recordLongTasks(page);
    await openRoom(page, server, home.room, home.shard);
    // Portal scans of the whole shard take about 70 s; measure 30 s of them.
    const until = Date.now() + 30_000;
    const result = await measure(page, () => Date.now() < until);
    expectResponsive(result);
  });
});
