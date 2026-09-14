import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { createServer as createTcpServer } from 'node:net';
import { createHash, randomBytes } from 'node:crypto';
import { readFile, writeFile, mkdir, access, unlink } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { Cdp, targets } from './cdp.mjs';
import { compare } from './compare.mjs';
import { writeReport } from './report.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../..');
const allFixtures = JSON.parse(await readFile(join(here, 'fixtures.json'), 'utf8'));
const args = process.argv.slice(2).filter(arg => arg !== '--');
const usage = `Pulse native Windows OCR acceptance pipeline (manual only)

  pnpm ocr:qa --run
  pnpm ocr:qa --run --cases=english-spacing,ukrainian-spacing --themes=light --modes=quick
  pnpm ocr:qa --run --repeat=3 --monitor=0

Default: ${allFixtures.length} cards × light/dark × quick/editor = ${allFixtures.length * 4} cases.
Options: --cases=id,... --themes=light,dark --modes=quick,editor --repeat=1 --monitor=0
         --skip-build (use an existing debug executable; report marks it unverified)

Requires an unlocked Windows desktop, Node 22+, Edge, Rust build tools and cached OCR models.
Quit Pulse and Tauri dev first. F8 or Ctrl+C aborts. See scripts/text-extractor/qa/README.md.
Without --run, no windows, input, clipboard access, model inference, or build is started.`;

function options() {
  const value = {};
  for (const arg of args) {
    if (arg === '--run' || arg === '--skip-build') continue;
    const match = /^--(cases|themes|modes|repeat|monitor)=(.+)$/u.exec(arg);
    if (!match || value[match[1]] !== undefined) throw new Error(`Invalid/duplicate option: ${arg}`);
    value[match[1]] = match[2];
  }
  const list = (name, choices) => {
    const selected = value[name]?.split(',') || choices;
    if (!selected.length || selected.some(item => !choices.includes(item)) || new Set(selected).size !== selected.length) throw new Error(`Invalid --${name}`);
    return selected;
  };
  const repeat = Number(value.repeat || 1), monitor = value.monitor === undefined ? null : Number(value.monitor);
  if (!Number.isInteger(repeat) || repeat < 1 || repeat > 20 || (monitor !== null && (!Number.isInteger(monitor) || monitor < 0))) throw new Error('Invalid repeat or monitor number');
  return { cases: list('cases', allFixtures.map(f => f.id)), themes: list('themes', ['light', 'dark']), modes: list('modes', ['quick', 'editor']), repeat, monitor, skipBuild: args.includes('--skip-build') };
}
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
async function exists(path) { try { await access(path); return true; } catch { return false; } }
async function readOptional(path) { try { return await readFile(path); } catch (error) { if (error.code === 'ENOENT') return null; throw error; } }
async function freePort() {
  const server = createTcpServer();
  await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
  const port = server.address().port; await new Promise(resolve => server.close(resolve)); return port;
}
function child(command, args, extra = {}) {
  const process = spawn(command, args, { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], ...extra });
  process.failure = null;
  process.exited = false;
  process.on('error', error => { process.failure = error; });
  process.once('exit', () => { process.exited = true; });
  return process;
}
async function stopChild(process) {
  if (!process || process.exited || process.failure) return;
  // Only a PID started by this harness may be terminated; never kill by image name.
  const killer = child('taskkill.exe', ['/PID', String(process.pid), '/T', '/F']);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Could not stop owned process ${process.pid}`)), 8000);
    killer.once('error', error => { clearTimeout(timeout); reject(error); });
    killer.once('exit', code => { clearTimeout(timeout); code === 0 || process.exited ? resolve() : reject(new Error(`Could not stop owned process ${process.pid}`)); });
  });
  const deadline = Date.now() + 5000;
  while (!process.exited && Date.now() < deadline) await delay(50);
  if (!process.exited) throw new Error(`Owned process ${process.pid} has not exited`);
}

class Desktop {
  constructor() {
    this.process = child('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-STA', '-ExecutionPolicy', 'Bypass', '-File', join(here, 'desktop.ps1')], { stdio: ['pipe', 'pipe', 'pipe'] });
    this.next = 0; this.pending = new Map(); this.stderr = '';
    this.process.stderr.on('data', chunk => { this.stderr += chunk.toString(); });
    createInterface({ input: this.process.stdout }).on('line', line => {
      let message; try { message = JSON.parse(line); } catch { return; }
      const item = this.pending.get(message.id); if (!item) return;
      clearTimeout(item.timeout); this.pending.delete(message.id);
      message.error ? item.reject(new Error(message.error)) : item.resolve(message.result);
    });
    const fail = error => { for (const item of this.pending.values()) { clearTimeout(item.timeout); item.reject(error); } this.pending.clear(); };
    this.process.stdin.on('error', fail);
    this.process.once('error', fail);
    this.process.once('exit', () => fail(new Error(`Desktop bridge stopped. ${this.stderr}`)));
  }
  call(action, params = {}) {
    if (this.process.failure || this.process.exited) return Promise.reject(new Error(`Desktop bridge unavailable. ${this.stderr}`));
    return new Promise((resolve, reject) => {
      const id = ++this.next;
      const timeout = setTimeout(() => { this.pending.delete(id); reject(new Error(`Desktop ${action} timed out. ${this.stderr}`)); }, 15000);
      this.pending.set(id, { resolve, reject, timeout });
      this.process.stdin.write(JSON.stringify({ id, action, ...params }) + '\n', error => {
        if (error) { clearTimeout(timeout); this.pending.delete(id); reject(error); }
      });
    });
  }
}

async function run() {
  const config = options();
  if (process.platform !== 'win32') throw new Error('Run this pipeline on an unlocked Windows desktop. Nothing has been launched.');
  if (Number(process.versions.node.split('.')[0]) < 22) throw new Error('Node 22 or newer is required.');
  if (!process.env.APPDATA || !process.env.LOCALAPPDATA) throw new Error('Windows user profile directories are unavailable.');
  const id = new Date().toISOString().replace(/[:.]/gu, '-');
  const output = join(root, 'out', 'text-extractor-qa', id);
  await mkdir(output, { recursive: true });
  const report = { id, platform: `${process.platform}/${process.arch}`, config, started: new Date().toISOString(), revision: 'unknown', cleanupErrors: [], results: [] };
  for (let repetition = 1; repetition <= config.repeat; repetition++) for (const theme of config.themes) for (const mode of config.modes) for (const fixture of allFixtures.filter(f => config.cases.includes(f.id))) {
    report.results.push({ key: `${fixture.id}-${theme}-${mode}-${repetition}`, fixtureId: fixture.id, theme, mode, repetition, expected: fixture.expected, status: 'pending' });
  }
  let bridge, pulse, edge, builder, server, cards, prefsPath, prefsBackup, originalPrefs, installedPrefs, prefsChanged = false;
  let stopped = false, appLog = '', appStream, edgeStream;
  const connections = new Map();
  const abort = () => { stopped = true; };
  process.on('SIGINT', abort); process.on('SIGTERM', abort);
  async function state() {
    if (stopped) throw new Error('Run aborted');
    if (pulse && (pulse.failure || pulse.exited)) throw new Error('Owned Pulse process stopped unexpectedly');
    const desktop = await bridge.call('state');
    const unexpected = pulse && desktop.windows.find(w => w.pid === pulse.pid && /^(Pulse )?Settings$/u.test(w.title));
    if (unexpected) throw new Error('Pulse opened Settings during OCR. See pulse.log for the underlying error.');
    return desktop;
  }
  async function until(description, probe, timeout = 30000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
      await state();
      const result = await probe(); if (result) return result;
      await delay(100);
    }
    throw new Error(`Timed out: ${description}`);
  }
  async function connect(target) {
    if (!connections.has(target.id)) connections.set(target.id, new Cdp(target.webSocketDebuggerUrl));
    return connections.get(target.id);
  }
  try {
    report.revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8', windowsHide: true }).trim();
    report.workingTree = execFileSync('git', ['status', '--porcelain'], { cwd: root, encoding: 'utf8', windowsHide: true }).trim();
    // Model hashes are derived from the production manifest, not a second drifting list.
    const modelSource = await readFile(join(root, 'src-tauri/src/text_extractor/windows/ocr_models.rs'), 'utf8');
    const version = /const CACHE_VERSION: &str = "([^"]+)"/u.exec(modelSource)?.[1];
    const pinned = [...modelSource.matchAll(/name: "(\w+\.onnx)"[\s\S]*?sha256: "([a-f0-9]{64})"/gu)];
    if (!version || pinned.length !== 2) throw new Error('Cannot read the current production OCR model manifest. Update the harness before running.');
    report.models = [];
    for (const [, name, digest] of pinned) {
      const path = join(process.env.LOCALAPPDATA, 'app.pulse.desktop', 'ocr', version, name);
      const bytes = await readOptional(path);
      if (!bytes || hash(bytes) !== digest) throw new Error(`Verified cached ${name} is required. Complete model setup in Pulse first; this pipeline does not download models.`);
      report.models.push({ name, sha256: digest, cacheVersion: version });
    }
    const edgePath = (await Promise.all([
      join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft/Edge/Application/msedge.exe'),
      join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft/Edge/Application/msedge.exe'),
      join(process.env.LOCALAPPDATA, 'Microsoft/Edge/Application/msedge.exe'),
    ].map(async path => await exists(path) ? path : null))).find(Boolean);
    if (!edgePath) throw new Error('Microsoft Edge was not found.');
    bridge = new Desktop();
    const initial = await state();
    if (initial.pulsePids.length) throw new Error('Quit Pulse and stop Tauri dev before running. Existing Pulse processes are left untouched.');
    const monitor = config.monitor === null ? initial.monitors.find(m => m.primary) : initial.monitors[config.monitor];
    if (!monitor) throw new Error('Requested monitor does not exist.');
    report.monitors = initial.monitors; report.monitor = monitor;
    if (!config.skipBuild) {
      console.log('Building the Windows debug app with cached dependencies…');
      builder = child('cargo.exe', ['build', '--locked', '--offline', '--no-default-features'], { cwd: join(root, 'src-tauri') });
      const buildStream = createWriteStream(join(output, 'build.log')); builder.stdout.pipe(buildStream, { end: false }); builder.stderr.pipe(buildStream, { end: false });
      try {
        await until('Windows build', () => {
          if (builder.failure) throw builder.failure;
          if (builder.exited && builder.exitCode !== 0) throw new Error('Build failed; inspect build.log.');
          return builder.exited && builder.exitCode === 0;
        }, 600000);
      } finally { builder.stdout.unpipe(buildStream); builder.stderr.unpipe(buildStream); buildStream.end(); }
      report.build = 'built from this checkout';
    } else report.build = 'existing binary; source revision is NOT verified';
    const binary = join(root, 'src-tauri/target/debug/pulse.exe');
    report.binarySha256 = hash(await readFile(binary));
    // Refuse a concurrently started user app immediately before changing preferences.
    if ((await state()).pulsePids.length) throw new Error('Pulse started during setup; stop it before running.');
    await bridge.call('init');
    prefsPath = join(process.env.APPDATA, 'app.pulse.desktop', 'text-extractor.json');
    originalPrefs = await readOptional(prefsPath);
    const preferences = originalPrefs ? JSON.parse(originalPrefs.toString('utf8')) : {};
    installedPrefs = Buffer.from(JSON.stringify({ ...preferences, enabled: true, shortcut: 'Super+Shift+KeyT', quickShortcut: 'Control+Super+Shift+KeyT', editorMode: 'basic', quickMode: 'basic' }));
    prefsBackup = join(output, 'preferences-backup.json');
    await writeFile(prefsBackup, JSON.stringify({ path: prefsPath, originalBase64: originalPrefs?.toString('base64') ?? null, installedBase64: installedPrefs.toString('base64') }, null, 2));
    await mkdir(dirname(prefsPath), { recursive: true });
    prefsChanged = true; await writeFile(prefsPath, installedPrefs);
    const pulsePort = await freePort(), edgePort = await freePort();
    if (pulsePort === edgePort) throw new Error('Debug port collision. Rerun after cleanup.');
    pulse = child(binary, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${pulsePort} --remote-debugging-address=127.0.0.1` } });
    appStream = createWriteStream(join(output, 'pulse.log'));
    for (const stream of [pulse.stdout, pulse.stderr]) stream.on('data', chunk => { appLog = (appLog + chunk.toString()).slice(-100000); if (!appStream.writableEnded) appStream.write(chunk); });
    await until('cached OCR sessions to initialize', () => appLog.includes('Offline OCR models are ready.'), 60000);
    // Pulse serializes migrated preferences at startup. Compare semantically, then
    // keep the exact bytes as the restoration guard against concurrent edits.
    const startupPrefs = await readFile(prefsPath);
    const saved = JSON.parse(startupPrefs.toString('utf8'));
    if (!saved.enabled || saved.editorMode !== 'basic' || saved.quickMode !== 'basic') throw new Error('Pulse did not load Basic-only QA settings.');
    installedPrefs = startupPrefs;
    await writeFile(prefsBackup, JSON.stringify({ path: prefsPath, originalBase64: originalPrefs?.toString('base64') ?? null, installedBase64: installedPrefs.toString('base64') }, null, 2));
    const token = randomBytes(24).toString('hex');
    const cardsBytes = await readFile(join(here, 'cards.html'));
    const fixturesBytes = await readFile(join(here, 'fixtures.json'));
    server = createServer((request, response) => {
      const url = new URL(request.url, 'http://127.0.0.1');
      if (url.searchParams.get('token') !== token) { response.writeHead(403).end(); return; }
      if (request.method === 'POST' && url.pathname === '/abort') { stopped = true; response.writeHead(204).end(); return; }
      if (request.method !== 'GET' || !['/', '/fixtures.json'].includes(url.pathname)) { response.writeHead(404).end(); return; }
      response.writeHead(200, { 'Content-Type': url.pathname === '/' ? 'text/html; charset=utf-8' : 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' });
      response.end(url.pathname === '/' ? cardsBytes : fixturesBytes);
    });
    await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
    const url = `http://127.0.0.1:${server.address().port}/?token=${token}&run=${id}`;
    edge = child(edgePath, [`--user-data-dir=${join(output, 'edge-profile')}`, `--remote-debugging-port=${edgePort}`, '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--new-window', url]);
    edgeStream = createWriteStream(join(output, 'edge.log')); edge.stderr.pipe(edgeStream);
    const page = await until('owned Edge card tab', async () => (await targets(edgePort).catch(() => [])).find(t => t.type === 'page' && t.url === url));
    cards = await connect(page); await cards.send('Page.enable');
    await until('card page to load', () => cards.evaluate('window.cardsReady === true'));
    let cardWindow = await until('native card window', async () => (await state()).windows.find(w => w.title === `Pulse OCR cards ${id}` || w.title.startsWith(`Pulse OCR cards ${id} - `)));
    await bridge.call('position', { handle: cardWindow.handle, x: monitor.left, y: monitor.top, width: monitor.width, height: monitor.height });
    const browserWindow = await cards.send('Browser.getWindowForTarget');
    await cards.send('Browser.setWindowBounds', { windowId: browserWindow.windowId, bounds: { windowState: 'fullscreen' } });
    await until('card window to fill selected monitor', async () => {
      cardWindow = (await state()).windows.find(w => w.handle === cardWindow.handle);
      return cardWindow && cardWindow.left === monitor.left && cardWindow.top === monitor.top && cardWindow.width === monitor.width && cardWindow.height === monitor.height;
    });
    // Windows can resize the native frame before Edge finishes its web viewport
    // transition. Wait for both coordinate systems before placing a selection.
    let viewport;
    try {
      await until('fullscreen card viewport', async () => {
        viewport = await cards.evaluate('({ width:innerWidth,height:innerHeight,dpr:devicePixelRatio,outerWidth,outerHeight })');
        return Math.abs(viewport.width * viewport.dpr - monitor.width) <= 2 && Math.abs(viewport.height * viewport.dpr - monitor.height) <= 2;
      }, 5000);
    } finally { report.viewport = viewport; }
    console.log(`Running ${report.results.length} native Basic OCR cases. F8 stops the run.`);
    for (const result of report.results) {
      const fixture = allFixtures.find(f => f.id === result.fixtureId);
      const caseDir = join(output, result.key); await mkdir(caseDir);
      await writeFile(join(caseDir, 'expected.txt'), fixture.expected + '\n');
      result.checks = {}; result.started = new Date().toISOString();
      const started = performance.now();
      let selector, selectorTarget, observer;
      try {
        const rectangle = await cards.evaluate(`window.showOcrCard(${JSON.stringify(fixture.id)}, ${JSON.stringify(result.theme)})`);
        report.dpr = rectangle.dpr;
        if (rectangle.overflow || rectangle.x < 0 || rectangle.y < 0 || rectangle.x + rectangle.width > rectangle.viewportWidth || rectangle.y + rectangle.height > rectangle.viewportHeight) throw new Error('Fixture does not fit this desktop. Use a larger display or lower display scaling.');
        if (Math.abs(rectangle.viewportWidth * rectangle.dpr - monitor.width) > 2 || Math.abs(rectangle.viewportHeight * rectangle.dpr - monitor.height) > 2) throw new Error(`Browser/native DPI geometry mismatch: ${JSON.stringify(rectangle)} versus ${JSON.stringify(monitor)}. Selection is blocked rather than guessing screen coordinates.`);
        const crop = { x: Math.round(monitor.left + rectangle.x * rectangle.dpr), y: Math.round(monitor.top + rectangle.y * rectangle.dpr), width: Math.round(rectangle.width * rectangle.dpr), height: Math.round(rectangle.height * rectangle.dpr) };
        result.crop = crop;
        await bridge.call('foreground', { handle: cardWindow.handle });
        await until('card foreground', async () => (await state()).foregroundPid === cardWindow.pid);
        const point = await bridge.call('point', { x: crop.x + Math.floor(crop.width / 2), y: crop.y + Math.floor(crop.height / 2) });
        if (point.handle !== cardWindow.handle) throw new Error('Another window covers the card.');
        // Pulse chooses the monitor under the cursor when the shortcut is pressed.
        await bridge.call('move', { pid: cardWindow.pid, x: crop.x + Math.floor(crop.width / 2), y: crop.y + Math.floor(crop.height / 2) });
        await bridge.call('capture', { ...crop, path: join(caseDir, 'source.png'), pid: cardWindow.pid });
        result.source = `${result.key}/source.png`;
        const sentinel = `Pulse OCR QA untouched ${randomBytes(12).toString('hex')}`;
        await bridge.call('clipboardSet', { text: sentinel });
        const shortcutAt = performance.now();
        await bridge.call('hotkey', { pid: cardWindow.pid, keys: result.mode === 'quick' ? [0x11, 0x5b, 0x10, 0x54] : [0x5b, 0x10, 0x54] });
        selector = await until('native selector', async () => {
          for (const target of await targets(pulsePort).catch(() => [])) {
            if (!target.url.includes('text-extractor.html')) continue;
            const connection = await connect(target);
            const ready = await connection.evaluate('document.body.dataset.phase === "selecting" && document.hasFocus()').catch(() => false);
            if (ready) { selectorTarget = target; return connection; }
          }
          return false;
        }, 10000);
        result.selectorMs = Math.round(performance.now() - shortcutAt);
        const capture = await selector.evaluate('window.__TAURI__.core.invoke("text_extractor_capture")');
        if (capture.defaultMode !== 'basic' || capture.mode !== result.mode || capture.width !== monitor.width || capture.height !== monitor.height) throw new Error('Selector mode or monitor differs from the requested Basic-only case.');
        result.checks.basicMode = true;
        // Observation only: do not inject OCR responses or call recognition commands.
        result.phases = [];
        observer = event => {
          if (event.method === 'Runtime.bindingCalled' && event.params.name === 'pulseQaObserve') {
            const sample = JSON.parse(event.params.payload); result.phases.push(sample);
            if (sample.resultVisible) result.resultWasVisible = true;
          }
        };
        selector.listeners.add(observer);
        await selector.send('Runtime.enable'); await selector.send('Runtime.addBinding', { name: 'pulseQaObserve' });
        await selector.evaluate(`(() => {
          const start = performance.now();
          const sample = () => pulseQaObserve(JSON.stringify({ phase: document.body.dataset.phase, ms: Math.round(performance.now() - start), resultVisible: !document.querySelector('#result').hidden }));
          new MutationObserver(sample).observe(document.body, { attributes: true, subtree: true, attributeFilter: ['data-phase', 'hidden'] }); sample();
        })()`);
        const left = crop.x, top = crop.y, right = crop.x + crop.width, bottom = crop.y + crop.height;
        await bridge.call('drag', { pid: pulse.pid, x1: fixture.reverseDrag ? right : left, y1: fixture.reverseDrag ? bottom : top, x2: fixture.reverseDrag ? left : right, y2: fixture.reverseDrag ? top : bottom });
        const released = performance.now();
        if (result.mode === 'quick') {
          let observedNotice = false, observedSuccess = false;
          const outcome = await until('Quick Copy completion or No text found notice', async () => {
            const desktop = await state();
            const notice = desktop.windows.find(w => w.pid === pulse.pid && w.title === 'Pulse Quick Copy Notice');
            if (notice) {
              const noticeTarget = (await targets(pulsePort)).find(t => t.url.includes('quick-copy-notice.html'));
              if (noticeTarget) {
                const noticePage = await connect(noticeTarget);
                const noticeState = await noticePage.evaluate(`(() => { const n = document.querySelector('.notice'); return { text:n.textContent,kind:n.dataset.kind,background:getComputedStyle(n).backgroundColor }; })()`);
                observedNotice = noticeState.text === 'No text found';
                observedSuccess = noticeState.text === 'Text copied' && noticeState.kind === 'success';
                result.noticeStyle = noticeState;
                result.notice = { left: notice.left, top: notice.top, width: notice.width, height: notice.height };
              }
            }
            const clipboard = (await bridge.call('clipboardGet')).text;
            return observedNotice || (clipboard !== sentinel && observedSuccess) ? { clipboard, observedNotice, observedSuccess } : false;
          });
          result.recognitionMs = Math.round(performance.now() - released);
          result.actual = outcome.clipboard === sentinel ? '' : outcome.clipboard;
          await until('quick selector to close', async () => !(await targets(pulsePort)).some(t => t.id === selectorTarget.id));
          result.checks.noResultUi = !result.resultWasVisible;
          result.checks.quickPathObserved = result.phases.some(sample => sample.phase === 'copying');
          result.checks.clipboard = fixture.expected ? outcome.clipboard !== sentinel : outcome.clipboard === sentinel;
          result.checks.noTextNotice = fixture.expected ? !outcome.observedNotice : outcome.observedNotice;
          result.checks.successNotice = fixture.expected ? outcome.observedSuccess : !outcome.observedSuccess;
          if (outcome.observedSuccess) {
            const [r, g, b] = result.noticeStyle.background.match(/[\d.]+/gu).map(Number);
            result.checks.greenSuccessNotice = g > r && g > b;
          }
          if (result.notice) {
            const n = result.notice;
            result.checks.noticeTopCenter = Math.abs(n.left + n.width / 2 - monitor.left - monitor.width / 2) <= 3 && n.top >= monitor.top && n.top - monitor.top <= 40 * rectangle.dpr;
          }
          result.checks.dismissed = true;
        } else {
          const editor = await until('editable OCR result', async () => {
            const value = await selector.evaluate(`({ phase: document.body.dataset.phase, text: document.querySelector('#extracted-text').value, disabled: document.querySelector('#extracted-text').disabled, readOnly: document.querySelector('#extracted-text').readOnly, copyDisabled: document.querySelector('#copy').disabled, error: document.querySelector('#extraction-error').textContent })`);
            if (value.phase === 'error') throw new Error(`Editor error: ${value.error}`);
            return value.phase === 'ready' ? value : false;
          });
          result.recognitionMs = Math.round(performance.now() - released);
          result.actual = editor.text;
          if (result.actual.length > 10000) throw new Error('Unexpected OCR response length');
          result.checks.editable = !editor.disabled && !editor.readOnly;
          // Capture only the result panel, not other applications on the monitor.
          const clip = await selector.evaluate(`(() => { const r = document.querySelector('#result').getBoundingClientRect(); return { x:r.x,y:r.y,width:r.width,height:r.height,scale:1 }; })()`);
          const screenshot = await selector.send('Page.captureScreenshot', { format: 'png', clip });
          await writeFile(join(caseDir, 'editor.png'), Buffer.from(screenshot.data, 'base64')); result.editor = `${result.key}/editor.png`;
          if (fixture.expected) {
            let copiedText = editor.text;
            if (fixture.editText) {
              await selector.evaluate('document.querySelector("#extracted-text").focus(); document.querySelector("#extracted-text").select();');
              await selector.send('Input.insertText', { text: fixture.editText });
              copiedText = await selector.evaluate('document.querySelector("#extracted-text").value');
              result.checks.editing = copiedText === fixture.editText;
            }
            const button = await selector.evaluate(`(() => { const b = document.querySelector('#copy'), r = b.getBoundingClientRect(); return { disabled:b.disabled, x:r.x+r.width/2, y:r.y+r.height/2, dpr:devicePixelRatio }; })()`);
            if (button.disabled) throw new Error('Copy button is disabled for a nonempty card');
            await bridge.call('click', { pid: pulse.pid, x: Math.round(monitor.left + button.x * button.dpr), y: Math.round(monitor.top + button.y * button.dpr) });
            const copied = await until('editor Copy to write clipboard', async () => { const value = (await bridge.call('clipboardGet')).text; return value === sentinel ? false : { text: value }; });
            result.checks.clipboard = copied.text === copiedText;
          } else {
            result.checks.copyDisabled = editor.copyDisabled;
            result.checks.clipboard = (await bridge.call('clipboardGet')).text === sentinel;
            await bridge.call('hotkey', { pid: pulse.pid, keys: [0x1b] });
          }
          await until('editor to dismiss', async () => !(await targets(pulsePort)).some(t => t.id === selectorTarget.id));
          result.checks.dismissed = true;
        }
        result.comparison = compare(fixture.expected, result.actual);
        result.status = result.comparison.pass && Object.values(result.checks).every(Boolean) ? 'passed' : 'failed';
      } catch (error) {
        result.status = 'blocked'; result.error = error.message;
        // A native failure leaves the matrix incomplete. Do not mask it with a retry.
        throw error;
      } finally {
        if (selector && observer) selector.listeners.delete(observer);
        if (result.actual !== undefined) {
          result.comparison = compare(fixture.expected, result.actual);
          await writeFile(join(caseDir, 'actual.txt'), result.actual + '\n');
        }
        result.totalMs = Math.round(performance.now() - started);
        console.log(`${result.status.toUpperCase()} ${result.key}`);
        await writeReport(output, report);
      }
    }
  } catch (error) {
    report.error = error.message; process.exitCode = 1;
  } finally {
    const cleanup = async (label, action) => { try { await action(); } catch (error) { report.cleanupErrors.push(`${label}: ${error.message}`); } };
    for (const connection of connections.values()) connection.close();
    await cleanup('Stop build', () => stopChild(builder));
    await cleanup('Stop owned Pulse', () => stopChild(pulse));
    await cleanup('Stop owned Edge', () => stopChild(edge));
    if (server) await cleanup('Close card server', () => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
    if (prefsChanged) await cleanup('Restore extraction preferences', async () => {
      if (pulse && !pulse.exited && !pulse.failure) throw new Error(`Pulse may still be running. Quit it, then use ${prefsBackup} to restore preferences.`);
      const current = await readOptional(prefsPath);
      if (!current || !current.equals(installedPrefs)) throw new Error(`Preferences changed during the run; left untouched. Original settings are in ${prefsBackup}.`);
      if (originalPrefs) await writeFile(prefsPath, originalPrefs); else await unlink(prefsPath);
      await unlink(prefsBackup);
    });
    if (bridge) await cleanup('Restore clipboard and desktop', async () => {
      try { await bridge.call('shutdown'); } finally { bridge.process.stdin.end(); }
      const deadline = Date.now() + 5000;
      while (!bridge.process.exited && Date.now() < deadline) await delay(50);
      if (!bridge.process.exited) throw new Error('Desktop bridge is still restoring the clipboard; leave its process running.');
      if (bridge.stderr.length) throw new Error(bridge.stderr);
    });
    appStream?.end(); edgeStream?.end();
    report.finished = new Date().toISOString();
    await writeReport(output, report);
    process.off('SIGINT', abort); process.off('SIGTERM', abort);
    if (report.status !== 'passed') process.exitCode = 1;
    console.log(`Report: ${join(output, 'report.html')}\n${report.status.toUpperCase()}: ${JSON.stringify(report.counts)}`);
    if (report.error) console.error(report.error);
    for (const error of report.cleanupErrors) console.error(error);
  }
}

// All declarations are initialized before this opt-in entry point. In particular,
// importing helpers or asking for usage never opens a window or starts a process.
if (!args.includes('--run') || args.includes('--help')) {
  console.log(usage);
} else {
  try { await run(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
