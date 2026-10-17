// The server's HTTP API, used to set up what a UI test needs: fresh accounts, players with a
// spawn and a running bot, and session tokens. Every helper takes the server base URL (with a
// trailing slash, as `OVERSEER_LIVE_URL`).
//
// On the `official` target (screeps.com) the API is read-only: {@link api} refuses every request
// that {@link isMutating} names, before it leaves.

/**
 * What the tests run against (`OVERSEER_E2E_TARGET`): `openscreeps`, a disposable OpenScreeps
 * world with its admin API; `community`, a disposable world of the stock community server
 * (screeps-launcher with screepsmod-auth and screepsmod-admin-utils) with its CLI; `official`,
 * screeps.com, read-only.
 */
export type Target = 'openscreeps' | 'official' | 'community';

/** The server under test, from `OVERSEER_E2E_TARGET` (`openscreeps` unless set). */
export function currentTarget(): Target {
  const target = process.env.OVERSEER_E2E_TARGET ?? 'openscreeps';
  if (target !== 'openscreeps' && target !== 'official' && target !== 'community') {
    throw new Error(
      `OVERSEER_E2E_TARGET is \`${target}\`, not \`openscreeps\`, \`official\` or \`community\``,
    );
  }
  return target;
}

/** The server's display name in seeded profiles of the disposable server. */
export const SERVER_NAME = 'Local e2e';

/** The harness's own player on a disposable world (`openscreeps`, `community`): no spawn. */
export interface HarnessPlayer {
  user: string;
  password: string;
}

/** The disposable server's administration (`openscreeps` only). */
export interface Admin {
  /** Bearer token of the server's admin API (`POST /admin`). */
  token: string;
}

/** The community server's CLI (`community` only). */
export interface Cli {
  /** The CLI endpoint (`OVERSEER_LIVE_CLI_URL`, `http://127.0.0.1:<port>/cli`). */
  url: string;
}

/** The server under test: the disposable world started by `cargo xtask e2e`, or screeps.com. */
export interface Server {
  /** Base URL with a trailing slash, e.g. `http://127.0.0.1:41234/` or `https://screeps.com/`. */
  url: string;
  /** The app's route slug of the server, e.g. `127.0.0.1-41234` or `screeps.com`. */
  slug: string;
  /** Display name of the server's seeded profile: {@link SERVER_NAME} or `screeps.com`. */
  name: string;
  /** The profile kind the app gives the server. */
  kind: 'private' | 'official';
  /** The harness's player; on the disposable targets (`openscreeps`, `community`). */
  player?: HarnessPlayer;
  /** The admin API; only on the `openscreeps` target. */
  admin?: Admin;
  /** The server's CLI; only on the `community` target. */
  cli?: Cli;
}

/** `server`'s admin API; fails on a server without one (`openscreeps` only). */
export function adminOf(server: Server): Admin {
  if (!server.admin) throw new Error(`${server.url} has no admin API`);
  return server.admin;
}

/**
 * Runs the JavaScript `command` in the community server's CLI (`community` only) and returns its
 * result as text: a string as it is, anything else as `util.inspect` shows it, a promise once it
 * settles. Throws when the command fails (the CLI answers `Error: …`).
 */
export async function cli(server: Server, command: string): Promise<string> {
  if (!server.cli) throw new Error(`${server.url} has no CLI`);
  const reply = await fetch(server.cli.url, { method: 'POST', body: command });
  const text = (await reply.text()).trimEnd();
  if (!reply.ok || text.startsWith('Error: ')) {
    throw new Error(`CLI \`${command.slice(0, 80)}\`: HTTP ${reply.status} ${text.slice(0, 300)}`);
  }
  return text;
}

/**
 * POST endpoints that only read (relative to the server base URL); everything else but `GET`,
 * `HEAD` and `OPTIONS` may change the account or the world.
 */
export const READ_ONLY_POSTS = [
  'api/game/map-stats',
  'api/game/rooms',
  'api/game/check-unique-flag-name',
  'api/game/check-unique-object-name',
  'api/game/gen-unique-flag-name',
  'api/game/gen-unique-object-name',
];

/** Whether a request with `method` to `url` may change something on the server. */
export function isMutating(method: string, url: string): boolean {
  const verb = method.toUpperCase();
  if (verb === 'GET' || verb === 'HEAD' || verb === 'OPTIONS') return false;
  if (verb !== 'POST') return true;
  const { pathname } = new URL(url);
  return !READ_ONLY_POSTS.some((path) => pathname.endsWith(`/${path}`));
}

/** A registered account (password sign-in). */
export interface Account {
  username: string;
  email: string;
  password: string;
}

/** An account with a spawn and {@link BOT} running. */
export interface Player extends Account {
  /** User id. */
  id: string;
  /** The room of the spawn. */
  room: string;
  /** The spawn's position. */
  spawn: { x: number; y: number };
}

/**
 * A small bot: keeps two creeps alive that wander randomly, logs the tick to the console and
 * counts ticks in `Memory.ticks`.
 */
export const BOT = `module.exports.loop = function () {
  for (const name in Memory.creeps) if (!Game.creeps[name]) delete Memory.creeps[name];
  for (const spawn of Object.values(Game.spawns)) {
    if (!spawn.spawning && Object.keys(Game.creeps).length < 2) spawn.spawnCreep([MOVE], 'c' + Game.time);
  }
  for (const creep of Object.values(Game.creeps)) creep.move(1 + Math.floor(Math.random() * 8));
  Memory.ticks = (Memory.ticks || 0) + 1;
  console.log('tick ' + Game.time);
};
`;

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set: run the UI tests with \`cargo xtask e2e\``);
  return value;
}

/** The server from the harness's environment. */
export function liveServer(): Server {
  const url = required('OVERSEER_LIVE_URL');
  const target = currentTarget();
  if (target === 'official') {
    return { url, slug: slug(url), name: 'screeps.com', kind: 'official' };
  }
  const server: Server = {
    url,
    slug: slug(url),
    name: SERVER_NAME,
    kind: 'private',
    player: { user: required('OVERSEER_LIVE_USER'), password: required('OVERSEER_LIVE_PASSWORD') },
  };
  if (target === 'openscreeps') server.admin = { token: required('OVERSEER_LIVE_ADMIN_TOKEN') };
  else server.cli = { url: required('OVERSEER_LIVE_CLI_URL') };
  return server;
}

/** The API token of the `official` target's account (`OVERSEER_LIVE_TOKEN`); never print it. */
export function liveToken(): string | undefined {
  return currentTarget() === 'official' ? required('OVERSEER_LIVE_TOKEN') : undefined;
}

/** The app's route slug of a server URL: host and port joined by `-` (`127.0.0.1-41234`). */
export function slug(url: string): string {
  const { hostname, port } = new URL(url);
  return port ? `${hostname}-${port}` : hostname;
}

/** A name nobody used yet: `prefix` and 8 random characters (letters and digits). */
export function unique(prefix: string): string {
  return `${prefix}${Math.random().toString(36).slice(2, 10).padEnd(8, '0')}`;
}

/** A failed API call: the server's `error` string, or the HTTP status. */
export class ApiError extends Error {}

/**
 * Calls `path` (`api/...`) and returns the JSON reply; throws {@link ApiError} on failure. On the
 * `official` target, a request {@link isMutating} names throws before it is sent.
 */
export async function api<T = Record<string, unknown>>(
  url: string,
  path: string,
  options: { token?: string; body?: unknown } = {},
): Promise<{ body: T; token?: string }> {
  const headers: Record<string, string> = {};
  if (options.token) {
    headers['X-Token'] = options.token;
    headers['X-Username'] = options.token;
  }
  let body: string | undefined;
  if (options.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    body = JSON.stringify(options.body);
  }
  const method = body === undefined ? 'GET' : 'POST';
  const target = new URL(path, url);
  if (currentTarget() === 'official' && isMutating(method, target.href)) {
    throw new Error(`${method} ${path} may change the account: refused on screeps.com`);
  }
  const reply = await fetch(target, { method, headers, body });
  const text = await reply.text();
  let json: Record<string, unknown> = {};
  try {
    json = JSON.parse(text) as Record<string, unknown>;
  } catch {
    // Not JSON: reported by status below.
  }
  if (typeof json.error === 'string') throw new ApiError(`${path}: ${json.error}`);
  if (!reply.ok) throw new ApiError(`${path}: HTTP ${reply.status} ${text.slice(0, 200)}`);
  return { body: json as T, token: reply.headers.get('X-Token') ?? undefined };
}

/** An authenticated session that follows token rotation (`X-Token`). */
export class Session {
  private constructor(
    readonly url: string,
    private current: string,
  ) {}

  /** Signs in with a username or e-mail and a password. */
  static async signIn(url: string, login: string, password: string): Promise<Session> {
    const { body } = await api<{ token: string }>(url, 'api/auth/signin', {
      body: { email: login, password },
    });
    return new Session(url, body.token);
  }

  /** The latest token. */
  get token(): string {
    return this.current;
  }

  async call<T = Record<string, unknown>>(path: string, body?: unknown): Promise<T> {
    const reply = await api<T>(this.url, path, { token: this.current, body });
    if (reply.token) this.current = reply.token;
    return reply.body;
  }

  /** `GET /api/auth/me`. */
  me(): Promise<{ _id: string; username: string; email?: string }> {
    return this.call('api/auth/me');
  }
}

/** Registers a new account (`POST /api/register/submit`), optionally with code. */
export async function register(
  url: string,
  options: { modules?: Record<string, string> } = {},
): Promise<Account> {
  const username = unique('ui');
  const account = { username, email: `${username}@e2e.invalid`, password: unique('pw-') };
  await api(url, 'api/register/submit', { body: { ...account, modules: options.modules } });
  return account;
}

/** Registers a new account running {@link BOT} and places its spawn in a free room. */
export async function createPlayer(url: string): Promise<Player> {
  const account = await register(url, { modules: { main: BOT } });
  const session = await Session.signIn(url, account.username, account.password);
  const { _id: id } = await session.me();
  const rooms = await freeRooms(session);
  // Start at a random room so concurrent tests rarely race for one.
  const offset = Math.floor(Math.random() * rooms.length);
  for (const room of [...rooms.slice(offset), ...rooms.slice(0, offset)]) {
    const { terrain } = await session.call<{ terrain: { terrain: string }[] }>(
      `api/game/room-terrain?room=${room}&encoded=1`,
    );
    const tiles = terrain[0]?.terrain;
    if (!tiles) continue;
    for (const spawn of spawnSpots(tiles).slice(0, 4)) {
      try {
        await session.call('api/game/place-spawn', { room, ...spawn, name: 'Spawn1' });
        return { ...account, id, room, spawn };
      } catch (error) {
        if (!(error instanceof ApiError)) throw error;
      }
    }
  }
  throw new Error('no room accepts a spawn');
}

/**
 * Removes the user `id` with everything they own (`user-remove` of the server's admin API): the
 * room is free again and the bot stops.
 */
export async function removeUser(server: Server, id: string): Promise<void> {
  const reply = await fetch(new URL('admin', server.url), {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${adminOf(server).token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ action: 'user-remove', userId: id }),
  });
  if (!reply.ok) throw new ApiError(`admin user-remove: HTTP ${reply.status} ${await reply.text()}`);
}

/**
 * Takes `player` out of the disposable world once its test is done. Every bot runs each tick
 * until then, so players that pile up slow the world down (requests take seconds) and fill its
 * rooms. With the admin API (`openscreeps`) the account goes with everything it owns
 * ({@link removeUser}); without one (`community`) the player respawns
 * (`POST /api/user/respawn`): the server removes its rooms and objects, the account stays. A
 * player already out of the world (`invalid status`: the test respawned it) needs nothing.
 */
export async function retirePlayer(server: Server, player: Player): Promise<void> {
  if (server.admin) return removeUser(server, player.id);
  const session = await Session.signIn(server.url, player.username, player.password);
  try {
    await session.call('api/user/respawn', {});
  } catch (error) {
    if (!(error instanceof ApiError && error.message.endsWith(': invalid status'))) throw error;
  }
}

/** Rooms with status `normal` that nobody owns. */
async function freeRooms(session: Session): Promise<string[]> {
  const rooms: string[] = [];
  for (const h of ['W', 'E'])
    for (const v of ['N', 'S'])
      for (let x = 0; x <= 10; x++) for (let y = 0; y <= 10; y++) rooms.push(`${h}${x}${v}${y}`);
  const { stats } = await session.call<{
    stats: Record<string, { status?: string; own?: unknown }>;
  }>('api/game/map-stats', { rooms, statName: 'owner0' });
  return Object.entries(stats)
    .filter(([, stat]) => stat.status === 'normal' && !stat.own)
    .map(([room]) => room);
}

/** Tiles with open plain ground around them, away from the exits, nearest the centre first. */
function spawnSpots(terrain: string): { x: number; y: number }[] {
  const spots: { x: number; y: number }[] = [];
  for (let y = 6; y < 44; y++)
    for (let x = 6; x < 44; x++) {
      let open = true;
      // Encoded terrain: one digit per tile, row by row; `0` is plain.
      for (let dy = -2; dy <= 2 && open; dy++)
        for (let dx = -2; dx <= 2 && open; dx++) open = terrain[(y + dy) * 50 + x + dx] === '0';
      if (open) spots.push({ x, y });
    }
  const distance = ({ x, y }: { x: number; y: number }) => (x - 25) ** 2 + (y - 25) ** 2;
  return spots.sort((a, b) => distance(a) - distance(b));
}
