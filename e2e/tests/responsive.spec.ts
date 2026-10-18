// The layout at every window width: no screen scrolls sideways, no bar cuts an item, and no text
// is cut without an ellipsis. Each test opens screens and resizes the window over a sweep of
// sizes: the full sweep (every 80 px from 320 to 2560 px, every 20 px around each breakpoint of
// `app/tailwind.css`, and common screen sizes) for the screens whose bars change most with the
// width, a quick sweep (the breakpoints and common sizes) for the others. At each size it
// checks the whole page (`audit` below):
// - the page does not scroll sideways;
// - no text and no control is partly outside an element that clips it (`overflow` other than
//   `visible`). A whole item outside is dropped on purpose (a bar drops its lowest-priority items
//   onto a line it clips); an item in a scroll container may scroll out of view; text in an
//   element that ellipsizes it (`text-overflow: ellipsis`) may be cut there;
// - text cut with an ellipsis has its full text in a `title`, an `aria-label` or a `data-tip`;
// - no two items of a bar (top bar, status bar, tabs, toolbars, panel heads) overlap.
//
// The content is long on purpose, as a real server makes it: a long server name, a sharded
// screeps.com-like server with a long shard name and a 9-digit tick, the world loading (its
// room overviews never answer), the API token's rate limits lifted, the account's usage, and a
// lost connection. The local server is no screeps.com: the session is seeded as a
// screeps.com-like profile (`kind: 'official'`, an API token) and the test rewrites what
// screeps.com would answer (`page.route`, `page.routeWebSocket`): `GET /api/version` lists
// shards, `GET /api/game/shards/info` gives their tick durations, `GET /api/user/auth-token`
// lists the token as lifted, and the socket carries the shard in its room channels and a large
// game time (the local server's channels name no shard).

import { inflateSync } from 'node:zlib';
import type { Page, WebSocketRoute } from '@playwright/test';
import { addServer, expect, Session, test, type Player, type Server } from './fixtures';
import { corsHeaders, fulfillJson, seed } from './support/accounts';
import { drawer, selectTab, status, type TabName } from './support/room';

/** The server's name: longer than the top bar's server button. */
const SERVER_NAME = 'Overseer responsive layout check of a long server name';

/** The server's shards; the map shows the longest. */
const SHARDS = ['shard0', 'shard1', 'shard2', 'shard3', 'shardSeasonal'];
const SHARD = 'shardSeasonal';

/** Added to the game time the socket reports: a 9-digit tick. */
const TICK_OFFSET = 123_400_000;

/** The breakpoints of `app/tailwind.css` (`--breakpoint-*`), in CSS pixels. */
const BREAKPOINTS = [521, 641, 761, 1001];

/** Narrowest and widest window checked. */
const NARROWEST = 320;
const WIDEST = 2560;

/** Height of the window for the width sweeps. */
const HEIGHT = 800;

/** The window sizes of a sweep, `[width, height]`. */
type Sweep = [number, number][];

/** Common screen sizes: laptops, desktops, phones (both ways) and a tablet. */
const SIZES: Sweep = [
  [1280, 720],
  [1366, 768],
  [1536, 864],
  [1920, 1080],
  [2560, 1440],
  [390, 844],
  [360, 740],
  [844, 390],
  [820, 1180],
];

/**
 * The full sweep, for the screens whose bars change most with the width: every 80 px from
 * {@link NARROWEST} to {@link WIDEST}, every 20 px from 40 px below to 40 px above each
 * breakpoint (with the pixel under it), then the common screen sizes.
 */
const FULL: Sweep = (() => {
  const widths = new Set<number>([WIDEST]);
  for (let width = NARROWEST; width <= WIDEST; width += 80) widths.add(width);
  for (const breakpoint of BREAKPOINTS) {
    for (let width = breakpoint - 40; width <= breakpoint + 40; width += 20) widths.add(width);
    widths.add(breakpoint - 1);
  }
  const sorted = [...widths].sort((a, b) => a - b);
  return [...sorted.map((width): [number, number] => [width, HEIGHT]), ...SIZES];
})();

/**
 * The quick sweep, for the other screens: the narrowest and widest windows, each breakpoint and
 * the pixel under it, a phone and a laptop width, then the common screen sizes.
 */
const QUICK: Sweep = [
  ...[NARROWEST, 480, ...BREAKPOINTS.flatMap((width) => [width - 1, width]), 1440, WIDEST].map(
    (width): [number, number] => [width, HEIGHT],
  ),
  ...SIZES,
];

/**
 * What is wrong with the layout of the page as it is now (run in the page; see the head of this
 * file). Each finding names the element and what is wrong, without numbers that change from
 * run to run.
 */
function audit(): string[] {
  const findings = new Set<string>();
  const root = document.documentElement;
  const styles = new Map<Element, CSSStyleDeclaration>();
  const style = (element: Element) => {
    let found = styles.get(element);
    if (!found) {
      found = getComputedStyle(element);
      styles.set(element, found);
    }
    return found;
  };
  /** The start of `text` in quotes, numbers as `#` (ticks and times change from run to run). */
  const excerpt = (text: string | null) =>
    JSON.stringify((text ?? '').replace(/\s+/g, ' ').trim().replace(/\d+/g, '#').slice(0, 32));
  const describe = (element: Element) => {
    const tag = element.tagName.toLowerCase();
    const label = element.getAttribute('aria-label') ?? element.getAttribute('title');
    const text = excerpt(element.textContent);
    return `<${tag}${label ? ` ${excerpt(label)}` : ''}>${text === '""' ? '' : ` ${text}`}`;
  };
  const shown = (element: Element) =>
    element.checkVisibility({ visibilityProperty: true, opacityProperty: true });

  if (root.scrollWidth > root.clientWidth + 1) findings.add('the page scrolls sideways');

  /**
   * The part of `box` (of a text or a control in `start`) left once every ancestor that clips
   * has clipped it; `null` when an ancestor hides it whole. Records a finding for each cut.
   */
  const visible = (box: DOMRect, start: Element, what: string, text: boolean) => {
    let [left, top, right, bottom] = [box.left, box.top, box.right, box.bottom];
    if (right - left < 1 || bottom - top < 1) return null;
    // A line of text is taller than its line box when the line height is tight: its glyphs
    // still show.
    const slack = text ? (bottom - top) * 0.3 : 0.5;
    for (let element: Element | null = start; element; element = element.parentElement) {
      const css = style(element);
      const clipsX = css.overflowX !== 'visible';
      const clipsY = css.overflowY !== 'visible';
      if (!clipsX && !clipsY && element !== root) continue;
      // Visually hidden (`sr-only`): not shown at all.
      if (element !== root && (element.clientWidth <= 1 || element.clientHeight <= 1)) return null;
      const frame = element.getBoundingClientRect();
      const inner =
        element === root
          ? { left: 0, top: 0, right: root.clientWidth, bottom: window.innerHeight }
          : {
              left: frame.left + element.clientLeft,
              top: frame.top + element.clientTop,
              right: frame.left + element.clientLeft + element.clientWidth,
              bottom: frame.top + element.clientTop + element.clientHeight,
            };
      const outside =
        right <= inner.left + 0.5 ||
        left >= inner.right - 0.5 ||
        bottom <= inner.top + slack ||
        top >= inner.bottom - slack;
      if (outside) return null;
      const cutX = left < inner.left - 0.5 || right > inner.right + 0.5;
      const cutY = top < inner.top - slack || bottom > inner.bottom + slack;
      const by = element === root ? 'the window' : describe(element);
      if ((clipsX || element === root) && cutX) {
        const ellipsized = text && clipsX && css.textOverflow === 'ellipsis';
        if (!['auto', 'scroll'].includes(css.overflowX) && !ellipsized) {
          findings.add(`${what} is cut ${right > inner.right ? 'at the right' : 'at the left'} by ${by}`);
        }
        left = Math.max(left, inner.left);
        right = Math.min(right, inner.right);
      }
      if (clipsY && cutY && element !== root) {
        if (!['auto', 'scroll'].includes(css.overflowY)) {
          findings.add(`${what} is cut ${bottom > inner.bottom ? 'at the bottom' : 'at the top'} by ${by}`);
        }
        top = Math.max(top, inner.top);
        bottom = Math.min(bottom, inner.bottom);
      }
    }
    return new DOMRect(left, top, right - left, bottom - top);
  };

  /** What a bar shows: each text and control with its visible box. */
  const bars = new Map<Element, { node: Node; box: DOMRect }[]>();
  const barOf = (element: Element) =>
    element.closest(
      'header, footer, [role="tablist"], [role="toolbar"], [role="dialog"] > div:first-child',
    );
  /** Positioned on purpose over another item (an icon in a field, a badge). */
  const floats = (element: Element, bar: Element) => {
    for (let at: Element | null = element; at && at !== bar; at = at.parentElement) {
      if (['absolute', 'fixed'].includes(style(at).position)) return true;
    }
    return false;
  };
  const record = (node: Node, element: Element, box: DOMRect | null) => {
    const bar = box && barOf(element);
    if (!bar || !box || floats(element, bar)) return;
    const items = bars.get(bar) ?? [];
    items.push({ node, box });
    bars.set(bar, items);
  };

  const texts = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  for (let node = texts.nextNode(); node; node = texts.nextNode()) {
    const parent = node.parentElement;
    if (!parent || !/\S/.test(node.textContent ?? '') || !shown(parent)) continue;
    if (['SCRIPT', 'STYLE', 'OPTION', 'TEXTAREA'].includes(parent.tagName)) continue;
    range.selectNodeContents(node);
    const box = visible(range.getBoundingClientRect(), parent, `text ${excerpt(node.textContent)}`, true);
    record(node, parent, box);
  }
  const controls = document.body.querySelectorAll(
    'button, a[href], input, select, textarea, [role="tab"], [role="button"], progress, img, svg',
  );
  for (const control of controls) {
    if (control.closest('svg') !== control && control.tagName.toLowerCase() === 'svg') continue;
    if (!shown(control)) continue;
    const start = control.parentElement;
    if (!start) continue;
    const box = visible(control.getBoundingClientRect(), start, describe(control), false);
    if (!['svg', 'img'].includes(control.tagName.toLowerCase())) record(control, control, box);
  }

  // Ellipsized text names its full text.
  for (const element of document.body.querySelectorAll('*')) {
    const css = style(element);
    if (css.textOverflow !== 'ellipsis' || css.overflowX === 'visible') continue;
    if (element.clientWidth <= 1 || element.scrollWidth <= element.clientWidth + 1) continue;
    if (!shown(element)) continue;
    const named = element.closest('[title], [aria-label], [data-tip]');
    const full = (element.textContent ?? '').trim();
    const says =
      named &&
      [named.getAttribute('title'), named.getAttribute('aria-label'), named.getAttribute('data-tip')]
        .filter((value): value is string => value !== null)
        .some((value) => value.toLowerCase().includes(full.toLowerCase()));
    if (!says) findings.add(`${describe(element)} is ellipsized without its full text in a title`);
  }

  // No two items of a bar overlap.
  for (const [bar, items] of bars) {
    for (let i = 0; i < items.length; i += 1) {
      for (let j = i + 1; j < items.length; j += 1) {
        const [a, b] = [items[i], items[j]];
        if (a.node.contains(b.node) || b.node.contains(a.node)) continue;
        const width = Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left);
        const height = Math.min(a.box.bottom, b.box.bottom) - Math.max(a.box.top, b.box.top);
        if (width > 1 && height > 1) {
          const name = (node: Node) => (node instanceof Element ? describe(node) : excerpt(node.textContent));
          findings.add(`${name(a.node)} overlaps ${name(b.node)} in ${describe(bar)}`);
        }
      }
    }
  }
  return [...findings];
}

/** Waits until the page has laid out and drawn the window's new size (two frames). */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
      }),
  );
}

/**
 * Resizes the window of `page` to each size of `sizes` in turn and returns what is wrong
 * ({@link audit}), each finding with the screen and the sizes it shows at.
 */
async function sweep(page: Page, screen: string, sizes: Sweep): Promise<string[]> {
  /** The steps of the sweep each finding shows at. */
  const found = new Map<string, number[]>();
  for (const [step, [width, height]] of sizes.entries()) {
    await page.setViewportSize({ width, height });
    await settle(page);
    for (const finding of await page.evaluate(audit)) {
      found.set(finding, [...(found.get(finding) ?? []), step]);
    }
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await settle(page);
  // Consecutive steps as one range: `480–620, 844×390`.
  const at = (steps: number[]) => {
    const runs: number[][] = [];
    for (const step of steps) {
      const run = runs.at(-1);
      if (run && run.at(-1) === step - 1) run.push(step);
      else runs.push([step]);
    }
    const name = (step: number) => {
      const [width, height] = sizes[step];
      return height === HEIGHT ? `${width}` : `${width}×${height}`;
    };
    return runs
      .map((run) => (run.length > 1 ? `${name(run[0])}–${name(run.at(-1) ?? 0)}` : name(run[0])))
      .join(', ');
  };
  return [...found].map(([finding, steps]) => `${screen}: ${finding} (at ${at(steps)})`);
}

/**
 * Opens each screen of `screens` in turn (its function) and sweeps it ({@link FULL} or
 * {@link QUICK}); fails with every finding.
 */
async function check(page: Page, screens: [string, Sweep, () => Promise<void>][]): Promise<void> {
  const findings: string[] = [];
  for (const [screen, sizes, open] of screens) {
    await open();
    findings.push(...(await sweep(page, screen, sizes)));
  }
  expect(findings, findings.join('\n')).toEqual([]);
}

/** The rewritten socket of {@link stress}: closes on demand, as a lost connection does. */
interface Socket {
  /** Closes the page's sockets and refuses new ones. */
  cut(): void;
}

/** A socket frame of the server as text: `gz:` frames inflated. */
function plain(message: string | Buffer): string {
  const text = typeof message === 'string' ? message : message.toString('utf8');
  return text.startsWith('gz:') ? inflateSync(Buffer.from(text.slice(3), 'base64')).toString('utf8') : text;
}

/**
 * Signs `player` in on `server` as a sharded screeps.com-like server with long content (see the
 * head of this file) and opens the player's room on {@link SHARD}, once its tick shows.
 */
async function stress(page: Page, server: Server, player: Player): Promise<Socket> {
  const session = await Session.signIn(server.url, player.username, player.password);
  const me = await session.me();
  await seed(
    page,
    [
      {
        name: SERVER_NAME,
        url: server.url,
        kind: 'official',
        login: player.username,
        user: { id: me._id, username: me.username, api_token: true },
      },
    ],
    { [server.url]: session.token },
  );
  await page.route('**/api/version', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const version = (await (await route.fetch()).json()) as { serverData: Record<string, unknown> };
    version.serverData.shards = SHARDS;
    return fulfillJson(route, version);
  });
  await page.route('**/api/game/shards/info', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const shards = SHARDS.map((name) => ({
      name,
      cpuLimit: 0,
      lastTicks: [3100, 3100],
      rooms: 0,
      tick: 3100,
      users: 0,
    }));
    return fulfillJson(route, { ok: 1, shards });
  });
  // Lifted for 1 h 52 min; the token named is the one the server issues right now (it rotates
  // the session token on every authenticated request).
  const lifted = Date.now() + 112 * 60_000;
  await page.route('**/api/user/auth-token', async (route) => {
    const request = route.request();
    if (request.method() !== 'GET') return route.fallback();
    const issued = await route.fetch({
      url: request.url().replace(/api\/user\/auth-token.*$/, 'api/auth/me'),
      method: 'GET',
    });
    const token = issued.headers()['x-token'] ?? request.headers()['x-token'] ?? '';
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      headers: { ...corsHeaders(request), 'x-token': token },
      body: JSON.stringify({
        ok: 1,
        tokens: [{ _id: 'k1', full: true, token: token.slice(0, 8), noRatelimitUntil: lifted }],
      }),
    });
  });
  // The room overviews never come: the world keeps loading.
  await page.route('**/api/game/map-stats', (route) =>
    route.request().method() === 'POST' ? new Promise<void>(() => {}) : route.fallback(),
  );
  const sockets: WebSocketRoute[] = [];
  let down = false;
  await page.routeWebSocket(
    (url) => url.pathname.startsWith('/socket'),
    (socket) => {
      if (down) {
        void socket.close();
        return;
      }
      sockets.push(socket);
      const upstream = socket.connectToServer();
      socket.onMessage((message) => {
        upstream.send(String(message).replace(/^((?:un)?subscribe [\w-]+:)[\w-]+\//, '$1'));
      });
      upstream.onMessage((message) => {
        socket.send(
          plain(message)
            .replace(/^\["(room|roomMap2):(?!shard)/, `["$1:${SHARD}/`)
            .replace(/"gameTime":(\d+)/g, (_, time: string) => `"gameTime":${Number(time) + TICK_OFFSET}`),
        );
      });
    },
  );
  await page.goto(`/#/${server.slug}/${SHARD}/${player.room}`);
  await expect(status(page).getByTitle('Game tick')).toHaveText(
    new RegExp(`^${SHARD} · tick \\d{9} · .+/tick$`),
    { timeout: 30_000 },
  );
  await expect(status(page).getByTitle(/rate limits are lifted/)).toBeVisible({ timeout: 30_000 });
  await expect(status(page).getByRole('status').filter({ hasText: /Owners/ })).toBeVisible();
  return {
    cut() {
      down = true;
      for (const socket of sockets) void socket.close();
    },
  };
}

/** Sets the drawer to its narrowest (`Home` on its edge) or widest (`End`) width. */
async function drawerWidth(page: Page, key: 'Home' | 'End'): Promise<void> {
  await page.setViewportSize({ width: 1440, height: 900 });
  const edge = page.getByRole('separator', { name: 'Resize drawer' });
  await edge.focus();
  await page.keyboard.press(key);
  await edge.blur();
}

test.describe('responsive layout', { tag: '@openscreeps' }, () => {
  test.skip(({ target }) => target === 'community', 'the stock community server lacks the contract');
  // The world loads from nothing (its loading progress shows). No motion: a layout that
  // changes with the width (the drawer becomes a bottom sheet) animates in, and the audit
  // measures where things end up.
  test.use({ cache: 'cold', reducedMotion: 'reduce' });
  test.afterEach(async ({ page }) => {
    await page.unrouteAll({ behavior: 'ignoreErrors' });
  });

  test('the world with each drawer tab', async ({ page, server, player }) => {
    test.setTimeout(120_000);
    await stress(page, server, player);
    await expect(drawer(page)).toBeVisible();
    await check(page, [
      ['Room tab', FULL, () => selectTab(page, 'Room')],
      ['Objects tab', QUICK, () => selectTab(page, 'Objects')],
      ['Console tab', QUICK, () => selectTab(page, 'Console')],
      ['Memory tab', QUICK, () => selectTab(page, 'Memory')],
    ]);
  });

  test('the drawer at its narrowest and widest', async ({ page, server, player }) => {
    test.setTimeout(120_000);
    await stress(page, server, player);
    const at = (tab: TabName, key: 'Home' | 'End') => async () => {
      await selectTab(page, tab);
      await drawerWidth(page, key);
    };
    await check(page, [
      ['Objects tab, narrowest drawer', QUICK, at('Objects', 'Home')],
      ['Console tab, narrowest drawer', QUICK, at('Console', 'Home')],
      ['Room tab, widest drawer', QUICK, at('Room', 'End')],
      ['Memory tab, widest drawer', QUICK, at('Memory', 'End')],
    ]);
  });

  test.describe('on a touch screen', () => {
    // A finger: the controls and rows grow to finger size (`pointer: coarse`).
    test.use({ hasTouch: true });

    test('the world and the account menu', async ({ page, server, player }) => {
      test.setTimeout(120_000);
      await stress(page, server, player);
      expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
      await check(page, [
        ['touch, Room tab', QUICK, () => selectTab(page, 'Room')],
        ['touch, Objects tab', QUICK, () => selectTab(page, 'Objects')],
        [
          'touch, account menu',
          QUICK,
          () => page.getByRole('button', { name: `Account: ${player.username}` }).click(),
        ],
      ]);
    });
  });

  test('the menus and panels of the top bar', async ({ page, server, player }) => {
    test.setTimeout(120_000);
    await stress(page, server, player);
    const account = page.getByRole('button', { name: `Account: ${player.username}` });
    const choose = (item: string) => async () => {
      // The account menu of the screen before may still be open.
      await page.keyboard.press('Escape');
      await account.click();
      await page.getByRole('menuitem', { name: item }).click();
    };
    await check(page, [
      ['server menu', QUICK, () => page.getByRole('button', { name: /^Server: / }).click()],
      ['account menu', QUICK, () => account.click()],
      ['Usage panel', QUICK, choose('Usage')],
      ['Rate limits panel', QUICK, choose('Rate limits')],
    ]);
  });

  test('the world while the connection is lost', async ({ page, server, player }) => {
    test.setTimeout(120_000);
    const socket = await stress(page, server, player);
    socket.cut();
    await expect(status(page)).toContainText(/Reconnecting in \d+ s/, { timeout: 30_000 });
    await check(page, [['reconnecting', FULL, async () => {}]]);
  });

  test('the pages of a server', async ({ page, server, player }) => {
    test.setTimeout(120_000);
    await stress(page, server, player);
    const heading = page.getByRole('heading', { name: player.username, level: 1 });
    await check(page, [
      [
        'profile',
        QUICK,
        async () => {
          await page.getByRole('button', { name: `Account: ${player.username}` }).click();
          await page.getByRole('menuitem', { name: 'Profile' }).click();
          await expect(heading).toBeVisible();
        },
      ],
      [
        'player',
        QUICK,
        async () => {
          await page.goto(`/#/${server.slug}/@user/${player.username}`);
          await expect(heading).toBeVisible();
        },
      ],
      [
        'room window',
        QUICK,
        async () => {
          await page.goto(`/#/${server.slug}/${SHARD}/${player.room}/window`);
          await expect(page.getByRole('heading', { level: 1 })).toContainText(player.room);
        },
      ],
      [
        'servers',
        QUICK,
        async () => {
          await page.goto('/#/');
          await expect(page.getByRole('region', { name: 'Servers' })).toBeVisible();
        },
      ],
    ]);
  });

  test('signing in and registering', async ({ page, server }) => {
    await addServer(page, server, SERVER_NAME);
    await check(page, [
      [
        'sign-in',
        QUICK,
        async () => {
          await page.getByRole('button', { name: `Sign in ${SERVER_NAME}` }).click();
          await expect(page.getByLabel('Username or e-mail')).toBeVisible();
        },
      ],
      [
        'register',
        QUICK,
        async () => {
          await page
            .getByRole('group', { name: 'Account' })
            .getByRole('button', { name: 'Register', exact: true })
            .click();
          await expect(page.getByRole('region', { name: `Register on ${SERVER_NAME}` })).toBeVisible();
        },
      ],
    ]);
  });
});
