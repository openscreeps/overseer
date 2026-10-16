// The shell's in-process `overseer-proxy`: the desktop app reaches every server through a proxy
// of its own on loopback (screeps.com sends no CORS headers), API calls and the socket alike,
// and that proxy answers the app's pages only.

import { expect, ORIGIN, signIn, statusBar, test } from './fixtures';

test('reaches the server through its own loopback proxy, which answers only the app', async ({
  app,
  server,
  credentials,
}) => {
  const page = await app.launch();
  await expect(statusBar(page)).toContainText('Desktop');
  const requests: string[] = [];
  const sockets: string[] = [];
  app.context.on('request', (request) => requests.push(request.url()));
  page.on('websocket', (socket) => sockets.push(socket.url()));
  await signIn(page, server, credentials);
  await expect(statusBar(page)).toContainText('Connected', { timeout: 30_000 });

  const calls = requests.filter((url) => new URL(url).pathname.startsWith('/api/'));
  expect(calls.length).toBeGreaterThan(0);
  const origins = [...new Set(calls.map((url) => new URL(url).origin))];
  expect(origins, calls.join('\n')).toHaveLength(1);
  const proxy = new URL(origins[0] ?? '');
  expect(proxy.hostname).toBe('127.0.0.1');
  expect(proxy.origin).not.toBe(new URL(server.url).origin);
  expect(sockets.map((url) => new URL(url).host)).toContain(proxy.host);

  const discovery = await fetch(new URL('overseer-proxy.json', proxy));
  expect(await discovery.json()).toEqual({ upstream: server.url });

  // What a page may read is up to the proxy's CORS answers: the app's origin only. The proxy
  // answers preflights itself; nothing reaches the server.
  const preflight = (origin: string) =>
    fetch(new URL('api/auth/me', proxy), {
      method: 'OPTIONS',
      headers: {
        Origin: origin,
        'Access-Control-Request-Method': 'GET',
        'Access-Control-Request-Headers': 'x-token',
      },
    });
  const own = await preflight(ORIGIN);
  expect(own.status).toBe(204);
  expect(own.headers.get('access-control-allow-origin')).toBe(ORIGIN);
  const foreign = await preflight('https://example.com');
  expect(foreign.status).toBe(403);
  expect(foreign.headers.get('access-control-allow-origin')).toBeNull();
});
