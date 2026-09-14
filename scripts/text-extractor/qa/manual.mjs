// Visible cards only: no app configuration, clipboard, input, or OCR automation.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, mkdir, access } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'win32') throw new Error('Open these manual test cards on the Windows desktop running Pulse.');
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../..');
const token = randomBytes(24).toString('hex');
const id = new Date().toISOString().replace(/[:.]/gu, '-');
const profile = join(root, 'out/text-extractor-manual', id, 'edge-profile');
const html = await readFile(join(here, 'cards.html'));
const fixtures = await readFile(join(here, 'fixtures.json'));
let edgePath;
for (const path of [
  join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft/Edge/Application/msedge.exe'),
  join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft/Edge/Application/msedge.exe'),
  join(process.env.LOCALAPPDATA, 'Microsoft/Edge/Application/msedge.exe'),
]) {
  try { await access(path); edgePath = path; break; } catch { /* Try the next installation location. */ }
}
if (!edgePath) throw new Error('Microsoft Edge was not found.');
await mkdir(profile, { recursive: true });
const server = createServer((request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1');
  if (url.searchParams.get('token') !== token) { response.writeHead(403).end(); return; }
  if (request.method !== 'GET' || !['/', '/fixtures.json'].includes(url.pathname)) { response.writeHead(404).end(); return; }
  response.writeHead(200, { 'Content-Type': url.pathname === '/' ? 'text/html; charset=utf-8' : 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(url.pathname === '/' ? html : fixtures);
});
await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/?token=${token}&manual=1&case=mixed-languages&run=${id}`;
const browser = spawn(edgePath, [`--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-background-mode', '--start-fullscreen', '--new-window', url], { stdio: 'ignore' });
const close = () => { server.closeAllConnections(); server.close(); };
browser.once('exit', close);
browser.once('error', error => { console.error(error.message); process.exitCode = 1; close(); });
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { browser.kill(); close(); });
console.log(JSON.stringify({ url, browserPid: browser.pid, serverPid: process.pid }));
