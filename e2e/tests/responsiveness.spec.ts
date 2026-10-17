// The page stays responsive while a whole shard loads in the background: terrain, owners and the
// portal scan of every room (screeps.com's `shard0`: 33 124 rooms, about 500 `roomMap2` answers a
// second) must never hold input or frames for long.
//
// The disposable world is small (11 × 11 rooms), so the first test makes it look like `shard0`:
// the page talks to a stand-in for the server (`support/stand-in.ts`) that says the world is
// 182 × 182 rooms, gives rooms the server does not have a real room's terrain and a made-up
// overview (some owned), and answers their `roomMap2` scans after 0.3 to 0.7 s, as screeps.com
// answers them; the real world's rooms stay real. The stand-in runs in the test process: unlike
// `page.route` and `page.routeWebSocket`, it adds no work to the page that is measured. On the
// official target the second test loads the real `shard0` (cold: a fresh browser profile).
//
// While the world loads behind a room-level view, the test measures what a user would feel: how
// long a task posted to the page's main thread every 250 ms waits before it runs (it waits behind
// whatever the main thread is doing), measured in the page, and the main thread's long tasks
// (over 50 ms, `PerformanceObserver`). Measured in the page: a `page.evaluate` round trip also
// waits for the test's own process. The tests are tagged `@alone`: they run after every other
// test, with the machine to themselves (`playwright.config.ts`).

import type { Page } from '@playwright/test';
import { api, currentTarget, expect, signIn, slug, test, type Server } from './fixtures';
import { startStandIn, type Answer, type StandIn } from './support/stand-in';

/** How many times slower the pages run (`OVERSEER_E2E_SLOW`), as the fixtures apply it. */
const SLOWDOWN = Math.max(1, Number(process.env.OVERSEER_E2E_SLOW ?? 1) || 1);

/** Rooms per side of the made-up shard (screeps.com's `shard0`). */
const SIDE = 182;

/** Rooms scanned before measuring: the first overviews and terrain are in by then. */
const WARM_UP = 1_000;

/** Rooms scanned while measuring (about 20 s at screeps.com's pace). */
const MEASURED = 10_000;

/** Longest wait of a posted task allowed while the world loads (screeps.com: 6 s before). */
const MAX_DELAY_MS = 300 * SLOWDOWN;

/** 95th percentile of the waits allowed. */
const P95_DELAY_MS = 100 * SLOWDOWN;

/** Share of the measured time the main thread may spend in long tasks. */
const LONG_TASK_SHARE = 0.1;

declare global {
  interface Window {
    /** Long tasks since the page loaded: start and duration, in milliseconds. */
    e2eLongTasks?: [number, number][];
    /** How long each posted task waited (see {@link measure}), while measuring. */
    e2eDelays?: number[];
  }
}

/**
 * Records the page's long tasks in `window.e2eLongTasks`, from now on (also the earlier ones the
 * browser buffered) and in every document the page loads later. The page has already loaded
 * the app (`signedInPage`), and a route change only changes the hash: an init script alone
 * would never run.
 */
async function recordLongTasks(page: Page): Promise<void> {
  const record = () => {
    window.e2eLongTasks = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        window.e2eLongTasks?.push([entry.startTime, entry.duration]);
      }
    }).observe({ type: 'longtask', buffered: true });
  };
  await page.addInitScript(record);
  await page.evaluate(record);
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
 * The rooms `server` has, of the 32 × 32 rooms around the zero lines (`POST /api/game/rooms`
 * lists only rooms it has): OpenScreeps' world is centred on them, the community world lies
 * north-west of them (W10N10 to W0N0).
 */
async function realRooms(server: Server): Promise<Set<string>> {
  const rooms: string[] = [];
  for (const h of ['W', 'E'])
    for (const v of ['N', 'S'])
      for (let x = 0; x < 16; x++) for (let y = 0; y < 16; y++) rooms.push(`${h}${x}${v}${y}`);
  const { body } = await api<{ rooms: { room: string }[] }>(server.url, 'api/game/rooms', {
    body: { rooms },
  });
  return new Set(body.rooms.map(({ room }) => room));
}

/** The made-up shard: a stand-in for the server, and how many made-up rooms it scanned. */
interface Shard {
  standIn: StandIn;
  scanned: () => number;
}

/** `answer` with `value` as its JSON body. */
function withJson(answer: Answer, value: unknown): Answer {
  return { ...answer, body: Buffer.from(JSON.stringify(value)) };
}

/** A JSON answer of the stand-in itself. */
function json(value: unknown): Answer {
  return withJson({ status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.alloc(0) }, value);
}

/** Makes `server` look like a 182 × 182 shard (see the file's comment), through a stand-in. */
async function fakeShard(server: Server): Promise<Shard> {
  const real = await realRooms(server);
  let terrains: string[] = [];
  let scanned = 0;
  const isReal = (name: string) => real.has(name) || !coords(name);
  const waiting = new Map<string, NodeJS.Timeout>();
  const standIn = await startStandIn(server.url, {
    http: async (request, forward) => {
      const [path = '', query = ''] = request.path.split('?');
      if (path === 'api/game/world-size') return json({ ok: 1, width: SIDE, height: SIDE });
      if (path === 'api/game/room-objects') {
        const room = new URLSearchParams(query).get('room') ?? '';
        return isReal(room) ? undefined : json({ ok: 1, objects: [], users: {} });
      }
      if (path === 'api/game/rooms') {
        const { rooms } = JSON.parse(request.body.toString()) as { rooms: string[] };
        const answer = await forward();
        const body = JSON.parse(answer.body.toString()) as { rooms: { room: string; terrain: string }[] };
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
        return withJson(answer, { ...body, rooms: [...body.rooms, ...made] });
      }
      if (path === 'api/game/map-stats') {
        const { rooms } = JSON.parse(request.body.toString()) as { rooms: string[] };
        const answer = await forward();
        const body = JSON.parse(answer.body.toString()) as {
          stats: Record<string, unknown>;
          users: Record<string, unknown>;
        };
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
        return withJson(answer, { ...body, users: { ...USERS, ...body.users } });
      }
      return undefined;
    },
    socket: (message, send) => {
      const subscribe = /^subscribe roomMap2:(?:[^/]+\/)?([WE]\d+[NS]\d+)$/.exec(message);
      if (subscribe?.[1] && !isReal(subscribe[1])) {
        const room = subscribe[1];
        const channel = message.slice('subscribe '.length);
        const answer = setTimeout(() => {
          waiting.delete(channel);
          scanned += 1;
          send(JSON.stringify([channel, roomMap(room)]));
        }, 300 + Math.random() * 400);
        waiting.set(channel, answer);
        return true;
      }
      const unsubscribe = /^unsubscribe roomMap2:(?:[^/]+\/)?([WE]\d+[NS]\d+)$/.exec(message);
      if (unsubscribe?.[1] && !isReal(unsubscribe[1])) {
        const channel = message.slice('unsubscribe '.length);
        clearTimeout(waiting.get(channel));
        waiting.delete(channel);
        return true;
      }
      return false;
    },
    closed: () => {
      for (const answer of waiting.values()) clearTimeout(answer);
      waiting.clear();
    },
  });
  return { standIn, scanned: () => scanned };
}

/** What the main thread did while the world loaded. */
interface Measurement {
  /** How long each posted task waited before it ran, in milliseconds. */
  delays: number[];
  /** Time in long tasks, in milliseconds. */
  longTaskMs: number;
  /** The longest task, in milliseconds. */
  longestTaskMs: number;
  /** How long the measurement took, in milliseconds. */
  elapsedMs: number;
}

/**
 * What the main thread did while `loading()` was true: every 250 ms (a sampling interval, not a
 * wait for the app) the page posts itself a task (`MessageChannel`, which no timer clamps) and
 * records how long it waited, and the long tasks meanwhile.
 */
async function measure(page: Page, loading: () => boolean): Promise<Measurement> {
  const since = await page.evaluate(() => {
    const delays: number[] = [];
    window.e2eDelays = delays;
    const channel = new MessageChannel();
    let posted = 0;
    const post = () => {
      if (window.e2eDelays !== delays) return;
      posted = performance.now();
      channel.port2.postMessage(null);
    };
    channel.port1.onmessage = () => {
      delays.push(performance.now() - posted);
      setTimeout(post, 250);
    };
    post();
    return performance.now();
  });
  const started = Date.now();
  while (loading()) await page.waitForTimeout(250);
  const { delays, tasks } = await page.evaluate((from) => {
    const delays = window.e2eDelays ?? [];
    delete window.e2eDelays;
    return { delays, tasks: (window.e2eLongTasks ?? []).filter(([start]) => start >= from) };
  }, since);
  return {
    delays,
    longTaskMs: tasks.reduce((total, [, duration]) => total + duration, 0),
    longestTaskMs: Math.max(0, ...tasks.map(([, duration]) => duration)),
    elapsedMs: Date.now() - started,
  };
}

/** Checks the measurements against the limits, with the numbers in the message. */
function expectResponsive(result: Measurement): void {
  const sorted = [...result.delays].sort((a, b) => a - b);
  const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? 0;
  const max = sorted.at(-1) ?? 0;
  const summary = JSON.stringify({ ...result, delays: undefined, p95, max, samples: sorted.length });
  // In the report also when it passes, to follow the numbers from run to run.
  test.info().annotations.push({ type: 'responsiveness', description: summary });
  expect(sorted.length, summary).toBeGreaterThan(10);
  expect(max, `longest wait: ${summary}`).toBeLessThanOrEqual(MAX_DELAY_MS);
  expect(p95, `95th percentile wait: ${summary}`).toBeLessThanOrEqual(P95_DELAY_MS);
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
  test.describe(() => {
    // The run's warm cache is the real server's world; the stand-in is another server.
    test.use({ cache: 'cold' });

    test('a whole shard loading in the background never holds the page', { tag: ['@openscreeps', '@alone'] }, async ({
      page,
      server,
      player,
    }) => {
      test.setTimeout(240_000);
      const shard = await fakeShard(server);
      try {
        const standIn: Server = { ...server, url: shard.standIn.url, slug: slug(shard.standIn.url) };
        await signIn(page, standIn, player);
        await recordLongTasks(page);
        await openRoom(page, standIn, player.room, null);
        await expect.poll(shard.scanned, { timeout: 60_000 * SLOWDOWN }).toBeGreaterThanOrEqual(WARM_UP);
        const result = await measure(page, () => shard.scanned() < WARM_UP + MEASURED);
        expectResponsive(result);
      } finally {
        await shard.standIn.close();
      }
    });
  });

  test('screeps.com’s shard loading in the background never holds the page', { tag: '@alone' }, async ({
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
