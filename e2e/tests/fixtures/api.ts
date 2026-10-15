// The server's HTTP API, used to set up what a UI test needs: fresh accounts, players with a
// spawn and a running bot, and session tokens. Every helper takes the server base URL (with a
// trailing slash, as `OVERSEER_LIVE_URL`).

/** The disposable server started by `cargo xtask e2e`. */
export interface Server {
  /** Base URL with a trailing slash, e.g. `http://127.0.0.1:41234/`. */
  url: string;
  /** The app's route slug of the server, e.g. `127.0.0.1-41234`. */
  slug: string;
  /** The harness's own player (no spawn). */
  adminUser: string;
  adminPassword: string;
  /** Bearer token of the server's admin API (`POST /admin`). */
  adminToken: string;
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
  return {
    url,
    slug: slug(url),
    adminUser: required('OVERSEER_LIVE_USER'),
    adminPassword: required('OVERSEER_LIVE_PASSWORD'),
    adminToken: required('OVERSEER_LIVE_ADMIN_TOKEN'),
  };
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

/** Calls `path` (`api/...`) and returns the JSON reply; throws {@link ApiError} on failure. */
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
  const reply = await fetch(new URL(path, url), {
    method: body === undefined ? 'GET' : 'POST',
    headers,
    body,
  });
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
