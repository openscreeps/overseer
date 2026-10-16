// A plain static file server for the `direct` project: serves `OVERSEER_E2E_DIST` (the built
// web app) on 127.0.0.1:`OVERSEER_E2E_DIRECT_PORT`, so the app talks to the game server
// cross-origin, as when it is hosted anywhere but behind overseer-proxy.

import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize, sep } from 'node:path';

const root = normalize(process.env.OVERSEER_E2E_DIST ?? '../target/e2e/dist');
const port = Number(process.env.OVERSEER_E2E_DIRECT_PORT ?? 4610);
const types = {
  '.css': 'text/css',
  '.html': 'text/html; charset=utf-8',
  '.ico': 'image/x-icon',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.wasm': 'application/wasm',
};

createServer(async (request, response) => {
  const path = decodeURIComponent(new URL(request.url ?? '/', 'http://localhost').pathname);
  const file = normalize(join(root, path.endsWith('/') ? `${path}index.html` : path));
  if (file !== root && !file.startsWith(root + sep)) {
    response.writeHead(403).end();
    return;
  }
  try {
    if (!(await stat(file)).isFile()) throw new Error('not a file');
  } catch {
    response.writeHead(404).end('not found');
    return;
  }
  response.writeHead(200, {
    'Content-Type': types[extname(file)] ?? 'application/octet-stream',
    'Cache-Control': 'no-store',
  });
  createReadStream(file).pipe(response);
}).listen(port, '127.0.0.1', () => {
  console.log(`serving ${root} on http://127.0.0.1:${port}/`);
});
