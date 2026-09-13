// Local UI fixture only. No credentials, native capture, clipboard writes, or API requests.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const source = fileURLToPath(new URL('../../src/', import.meta.url));
const fixture = fileURLToPath(new URL('./preview-bridge.js', import.meta.url));
const port = Number(process.env.PORT || 4178);
createServer(async (req, res) => {
  try {
    const path = new URL(req.url, `http://localhost:${port}`).pathname;
    const name = path === '/' ? 'text-extractor.html' : path.slice(1);
    if (!/^[a-z0-9-]+\.(html|css|js)$/.test(name)) { res.writeHead(404).end(); return; }
    let body = await readFile(name === 'preview-bridge.js' ? fixture : `${source}${name}`, 'utf8');
    if (name.endsWith('.html')) body = body.replace('<head>', '<head><script src="/preview-bridge.js"></script>');
    res.setHeader('Content-Type', name.endsWith('.html') ? 'text/html' : name.endsWith('.css') ? 'text/css' : 'text/javascript');
    res.setHeader('Cache-Control', 'no-store');
    res.end(body);
  } catch { res.writeHead(404).end(); }
}).listen(port, '127.0.0.1', () => console.log(`Text Extractor UI fixture: http://127.0.0.1:${port}`));
