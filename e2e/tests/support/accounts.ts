// Helpers of the account specs (servers, sign-in, sessions, profile, rate limits): seeding the
// app's saved profiles the way it stores them, the server's admin API, and rewriting the
// server's answers (`page.route`) for what a disposable server cannot do on its own.

import type { Locator, Page, Request, Route } from '@playwright/test';
import { adminOf, Session, type Account, type Server } from '../fixtures';

/**
 * The alerts of `scope` that say something: forms keep an empty alert ready for their error, so
 * that it is announced when it appears.
 */
export function alerts(scope: Locator): Locator {
  return scope.getByRole('alert').filter({ hasText: /\S/ });
}

/** A saved server as the app stores it in `overseer.profiles` (version 2). */
export interface SavedServer {
  name: string;
  url: string;
  kind: 'official' | 'season' | 'private';
  /** The name that fills the sign-in form. */
  login?: string;
  /** Who is signed in; absent when signed out. */
  user?: { id: string; username?: string; api_token?: boolean };
}

/**
 * Seeds the app's storage of `page` once per tab (reloads keep what the app wrote since):
 * `servers` as the saved profiles and `tokens` (base URL → token) as the stored session tokens.
 */
export async function seed(
  page: Page,
  servers: SavedServer[],
  tokens: Record<string, string> = {},
): Promise<void> {
  await page.addInitScript(
    ({ profiles, tokens }) => {
      if (sessionStorage.getItem('overseer.e2e.seeded')) return;
      sessionStorage.setItem('overseer.e2e.seeded', '1');
      localStorage.setItem('overseer.profiles', JSON.stringify(profiles));
      for (const [url, token] of Object.entries(tokens)) {
        localStorage.setItem(`overseer.secret.token:${url}`, token);
      }
    },
    { profiles: { version: 2, servers }, tokens },
  );
}

/** Signs `account` in on `server` through the API: the saved user and a fresh session token. */
export async function sessionOf(
  server: Server,
  account: Account,
): Promise<{ user: { id: string; username: string }; token: string }> {
  const session = await Session.signIn(server.url, account.username, account.password);
  const me = await session.me();
  return { user: { id: me._id, username: me.username }, token: session.token };
}

/** The stored session token of the server at `url`, `null` when none. */
export function storedToken(page: Page, url: string): Promise<string | null> {
  return page.evaluate((key) => localStorage.getItem(key), `overseer.secret.token:${url}`);
}

/** Runs `action` of the server's admin API (`POST /admin`) and returns its reply. */
export async function admin(
  server: Server,
  action: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const reply = await fetch(new URL('admin', server.url), {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${adminOf(server).token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(action),
  });
  const text = await reply.text();
  if (!reply.ok) throw new Error(`admin ${String(action.action)}: HTTP ${reply.status} ${text}`);
  return JSON.parse(text) as Record<string, unknown>;
}

/** Lets a cross-origin page (direct mode) read a rewritten response and the headers it needs. */
export function corsHeaders(request: Request, exposed = ''): Record<string, string> {
  const origin = request.headers()['origin'];
  const expose = [exposed, 'X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset']
    .filter(Boolean)
    .join(', ');
  return origin
    ? { 'access-control-allow-origin': origin, 'access-control-expose-headers': expose }
    : {};
}

/** Answers `route` with the JSON `body` (HTTP 200), readable cross-origin. */
export function fulfillJson(route: Route, body: unknown): Promise<void> {
  return route.fulfill({
    status: 200,
    contentType: 'application/json',
    headers: corsHeaders(route.request()),
    body: JSON.stringify(body),
  });
}

/**
 * Makes the server password-protected for `page`, as a server started with a server password
 * answers: every `api/*` request without the right `X-Server-Password` gets
 * `{"error": "incorrect server password"}`. Returns the passwords the requests carried.
 */
export async function protectWithServerPassword(page: Page, password: string): Promise<string[]> {
  const sent: string[] = [];
  await page.route(
    (url) => url.pathname.includes('/api/'),
    async (route) => {
      const request = route.request();
      if (request.method() === 'OPTIONS') return route.fallback();
      const given = request.headers()['x-server-password'] ?? '';
      sent.push(given);
      if (given === password) return route.fallback();
      return fulfillJson(route, { error: 'incorrect server password' });
    },
  );
  return sent;
}

/** Opens the menu of the top bar's account button and chooses `item`. */
export async function accountMenu(page: Page, username: string, item: string): Promise<void> {
  await page.getByRole('button', { name: `Account: ${username}` }).click();
  await page.getByRole('menuitem', { name: item }).click();
}
