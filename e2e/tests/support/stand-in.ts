// A stand-in for a game server, in the test process: it passes every HTTP request and socket
// message on to the real server (`upstream`), except those its hooks answer themselves. The page
// talks to it as to any private server, cross-origin: it answers with CORS headers of its own.
//
// Use it where a test must change what the server answers without touching the page: unlike
// `page.route` and `page.routeWebSocket`, it runs no code in the page and holds no request in
// the browser, so measurements of the page see only the app.

import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { WebSocket, WebSocketServer } from 'ws';

/** An HTTP answer. */
export interface Answer {
  status: number;
  headers: Record<string, string>;
  body: Buffer;
}

/** An HTTP request the stand-in received. */
export interface Request {
  method: string;
  /** Path and query, without the leading `/`. */
  path: string;
  body: Buffer;
}

/** What the stand-in answers itself. */
export interface Hooks {
  /**
   * The answer to `request`, or `undefined` to pass the server's answer on. `forward` asks the
   * server.
   */
  http?: (request: Request, forward: () => Promise<Answer>) => Promise<Answer | undefined>;
  /**
   * Handles a socket message of the page; returns whether it did (else it goes to the server).
   * `send` sends the page a message on that socket.
   */
  socket?: (message: string, send: (message: string) => void) => boolean;
  /** A socket of the page closed. */
  closed?: () => void;
}

/** A running stand-in. */
export interface StandIn {
  /** Its base URL, with a trailing slash. */
  url: string;
  close(): Promise<void>;
}

/** Headers of the server's answer that the stand-in sets itself or that no longer apply. */
const DROPPED =
  /^(access-control-.*|content-encoding|content-length|transfer-encoding|connection|keep-alive)$/i;

/** Lets the page (any origin) send `request` and read the answer, as `overseer-proxy` does. */
function cors(request: IncomingMessage): Record<string, string> {
  return {
    'access-control-allow-origin': request.headers.origin ?? '*',
    'access-control-allow-methods': 'GET, POST, PUT, DELETE, OPTIONS',
    'access-control-allow-headers': request.headers['access-control-request-headers'] ?? '*',
    'access-control-expose-headers':
      'X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset',
  };
}

async function body(request: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/** The server's answer to `request`, sent with the page's headers. */
async function forwarded(upstream: string, incoming: IncomingMessage, request: Request): Promise<Answer> {
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(incoming.headers)) {
    if (typeof value === 'string' && !/^(host|origin|connection|content-length)$/i.test(name)) {
      headers[name] = value;
    }
  }
  const answer = await fetch(new URL(request.path, upstream), {
    method: request.method,
    headers,
    body: request.method === 'GET' || request.method === 'HEAD' ? undefined : request.body,
  });
  const kept: Record<string, string> = {};
  answer.headers.forEach((value, name) => {
    if (!DROPPED.test(name)) kept[name] = value;
  });
  return { status: answer.status, headers: kept, body: Buffer.from(await answer.arrayBuffer()) };
}

/** Passes the page's socket `page` on to the server's, but for what `hooks` handle. */
function bridge(upstream: string, page: WebSocket, hooks: Hooks): void {
  const target = new URL('socket/websocket', upstream);
  target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:';
  const real = new WebSocket(target);
  const waiting: string[] = [];
  real.on('open', () => {
    for (const message of waiting.splice(0)) real.send(message);
  });
  real.on('message', (data) => {
    if (page.readyState === WebSocket.OPEN) page.send(data.toString());
  });
  real.on('close', () => page.close());
  real.on('error', () => page.close());
  const send = (message: string) => {
    if (page.readyState === WebSocket.OPEN) page.send(message);
  };
  page.on('message', (data) => {
    const message = data.toString();
    if (hooks.socket?.(message, send)) return;
    if (real.readyState === WebSocket.OPEN) real.send(message);
    else waiting.push(message);
  });
  page.on('close', () => {
    real.close();
    hooks.closed?.();
  });
}

/** Starts a stand-in for the server at `upstream` (a base URL with a trailing slash). */
export async function startStandIn(upstream: string, hooks: Hooks): Promise<StandIn> {
  const handle = async (incoming: IncomingMessage, response: ServerResponse) => {
    if (incoming.method === 'OPTIONS') {
      response.writeHead(204, cors(incoming)).end();
      return;
    }
    const request: Request = {
      method: incoming.method ?? 'GET',
      path: (incoming.url ?? '/').replace(/^\//, ''),
      body: await body(incoming),
    };
    const forward = () => forwarded(upstream, incoming, request);
    const answer = (await hooks.http?.(request, forward)) ?? (await forward());
    response.writeHead(answer.status, { ...answer.headers, ...cors(incoming) }).end(answer.body);
  };
  const server = createServer((incoming, response) => {
    handle(incoming, response).catch((error: unknown) => {
      if (!response.headersSent) response.writeHead(502, cors(incoming));
      response.end(String(error));
    });
  });
  const sockets = new WebSocketServer({ server, path: '/socket/websocket' });
  sockets.on('connection', (page) => bridge(upstream, page, hooks));
  const listening = Promise.withResolvers<void>();
  server.listen(0, '127.0.0.1', listening.resolve);
  await listening.promise;
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://127.0.0.1:${port}/`,
    close: () => {
      const closed = Promise.withResolvers<void>();
      for (const client of sockets.clients) client.terminate();
      sockets.close();
      server.closeAllConnections();
      server.close(() => closed.resolve());
      return closed.promise;
    },
  };
}
