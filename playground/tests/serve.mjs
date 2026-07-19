import { createServer } from 'node:http';
import { readFile, realpath, stat } from 'node:fs/promises';
import { dirname, extname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = await realpath(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
const port = Number(process.argv[2] ?? 8787);

if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error(`invalid port: ${process.argv[2] ?? ''}`);
}

const contentTypes = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.ico', 'image/x-icon'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.map', 'application/json; charset=utf-8'],
  ['.png', 'image/png'],
  ['.svg', 'image/svg+xml; charset=utf-8'],
  ['.wasm', 'application/wasm'],
  ['.webmanifest', 'application/manifest+json'],
]);

async function resolveFile(pathname) {
  const decoded = decodeURIComponent(pathname);
  const candidate = resolve(root, `.${decoded === '/' ? '/index.html' : decoded}`);
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) return null;

  const metadata = await stat(candidate);
  const file = metadata.isDirectory() ? resolve(candidate, 'index.html') : candidate;
  const canonical = await realpath(file);
  return canonical === root || canonical.startsWith(`${root}${sep}`) ? canonical : null;
}

const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url ?? '/', 'http://localhost').pathname;
    const file = await resolveFile(pathname);
    if (!file) {
      response.writeHead(403).end('Forbidden');
      return;
    }

    const body = await readFile(file);
    response.writeHead(200, {
      'cache-control': 'no-store',
      'content-length': body.byteLength,
      'content-type': contentTypes.get(extname(file)) ?? 'application/octet-stream',
    });
    response.end(request.method === 'HEAD' ? undefined : body);
  } catch (error) {
    const status = error?.code === 'ENOENT' || error?.code === 'ENOTDIR' ? 404 : 500;
    response.writeHead(status).end(status === 404 ? 'Not Found' : 'Internal Server Error');
  }
});

server.listen(port, '127.0.0.1', () => {
  console.log(`Playground server listening on http://127.0.0.1:${port}`);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
