import { readFile, realpath } from 'node:fs/promises';
import { resolve, sep, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { object, errorText } from '../core/validate.js';
export const webRoot = fileURLToPath(new URL('../',import.meta.url));
const csp = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
/** @param {import('node:http').ServerResponse} response */
export function secureHeaders(response) {
  response.setHeader('Content-Security-Policy',csp); response.setHeader('X-Content-Type-Options','nosniff');
  response.setHeader('Referrer-Policy','no-referrer'); response.setHeader('Cache-Control','no-store');
  response.setHeader('Permissions-Policy','camera=(), microphone=(), geolocation=()');
}
/** @param {import('node:http').IncomingMessage} request */
export function sameOrigin(request) {
  const expected = `http://${request.headers.host}`;
  return request.headers.origin === expected;
}
/** @param {import('node:http').IncomingMessage} request */
async function body(request) {
  if (!String(request.headers['content-type']).startsWith('application/json')) throw new Error('Expected JSON request');
  let size = 0; const chunks = [];
  for await (const chunk of request) {
    size += chunk.length; if (size > 16_384) throw new Error('Request body too large'); chunks.push(chunk);
  }
  return object(JSON.parse(Buffer.concat(chunks).toString('utf8')));
}
/** @param {import('node:http').IncomingMessage} request @param {import('node:http').ServerResponse} response @param {import('./auth.js').MockAuth} auth */
export async function serve(request, response, auth) {
  secureHeaders(response);
  const host = request.headers.host ?? '';
  if (!/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/.test(host)) { response.writeHead(403); response.end('Local development host required'); return; }
  const url = new URL(request.url ?? '/',`http://${host}`);
  try {
    if (url.pathname === '/api/health') {
      response.setHeader('Content-Type','application/json');response.end(JSON.stringify({status:'ok',protocol_version:18,min_protocol_version:17,pairing_open:true}));return;
    }
    if (url.pathname.startsWith('/api/')) {
      if (request.method !== 'POST' || !sameOrigin(request)) { response.writeHead(403); response.end('Same-origin POST required'); return; }
      const data = await body(request);
      const result = url.pathname === '/api/pair/claim' ? auth.claim(data) : url.pathname === '/api/pair/status' ? auth.status(data) : null;
      if (!result) { response.writeHead(404); response.end('Unknown API endpoint'); return; }
      response.setHeader('Content-Type','application/json'); response.end(JSON.stringify(result)); return;
    }
    if (request.method !== 'GET' && request.method !== 'HEAD') { response.writeHead(405); response.end(); return; }
    await staticFile(url.pathname,response,request.method === 'HEAD');
  } catch (error) {
    const missing = error instanceof Error && 'code' in error && error.code === 'ENOENT';
    response.writeHead(missing ? 404 : 400,{'Content-Type':'application/json'}); response.end(JSON.stringify({error:{code:missing ? 'not_found' : 'bad_request',message:errorText(error)}}));
  }
}
/** @param {string} pathname @param {import('node:http').ServerResponse} response @param {boolean} head */
async function staticFile(pathname, response, head) {
  const decoded = decodeURIComponent(pathname);
  if (decoded.includes('\0') || decoded.includes('\\')) throw new Error('Invalid path');
  const relative = decoded === '/' ? 'index.html' : decoded.slice(1);
  if (relative.split('/').some(part => part.startsWith('.') || ['node_modules','dev'].includes(part))) throw new Error('Private development path');
  const root = await realpath(webRoot); const file = await realpath(resolve(root,relative));
  if (!file.startsWith(root + sep)) throw new Error('Path escapes web root');
  const mime = /** @type {Record<string,string>} */ ({'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8','.css':'text/css; charset=utf-8','.svg':'image/svg+xml','.png':'image/png','.webmanifest':'application/manifest+json','.ico':'image/x-icon'});
  const type = mime[extname(file)]; if (!type) throw new Error('Unsupported static file type');
  const bytes = await readFile(file); response.setHeader('Content-Type',type); response.setHeader('Content-Length',bytes.length); response.end(head ? undefined : bytes);
}
