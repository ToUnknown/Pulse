// Explicit local-only preview: real Apple Vision + Google, simulated desktop and clipboard.
// Start only when authorized to send the chosen sample text to Google.
import { createServer } from 'node:http';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
if (process.platform !== 'darwin' || !process.argv.includes('--run')) throw new Error('Use --run on macOS to enable real OCR and Google translation.');
const root = fileURLToPath(new URL('../../', import.meta.url));
const output = resolve(root, 'out/live-translation-preview');
await mkdir(output, { recursive:true });
const port = Number(process.env.PORT || 4188);
const origin = `http://127.0.0.1:${port}`;
const token = randomBytes(24).toString('hex');
const worker = spawn('cargo', ['test', '--lib', 'text_extractor::live_preview::live_preview_worker', '--', '--ignored', '--nocapture', '--test-threads=1'], {
  cwd:resolve(root,'src-tauri'), env:{...process.env,PULSE_LIVE_PREVIEW:'1'}, stdio:['pipe','pipe','inherit'],
});
let nextId = 0;
const pending = new Map();
let readyResolve, readyReject;
const ready = new Promise((yes,no)=>{readyResolve=yes;readyReject=no;});
createInterface({ input:worker.stdout }).on('line',line=>{
  if (line.includes('PULSE_PREVIEW_READY')) readyResolve();
  const prefix = 'PULSE_PREVIEW_RESULT ';
  if (!line.startsWith(prefix)) return;
  const result = JSON.parse(line.slice(prefix.length));
  const entry = pending.get(result.id);
  if (!entry) return;
  pending.delete(result.id); clearTimeout(entry.timeout); entry.resolve(result);
});
worker.on('error',readyReject);
worker.on('exit',()=>{readyReject(new Error('Preview worker stopped'));for(const p of pending.values()){clearTimeout(p.timeout);p.reject(new Error('Preview worker stopped'));}pending.clear();});
await ready;
function call(request) {
  return new Promise((resolve,reject)=>{
    const id=++nextId;
    const timeout=setTimeout(()=>{pending.delete(id);reject(new Error('Preview request timed out'));},90_000);
    pending.set(id,{resolve,reject,timeout});
    worker.stdin.write(JSON.stringify({...request,id})+'\n');
  });
}
const server=createServer(async(req,res)=>{
  try {
    const url=new URL(req.url,origin);
    if(req.headers.host!==`127.0.0.1:${port}`){res.writeHead(403).end();return;}
    if(url.pathname==='/live' && req.method==='POST'){
      if(req.headers.origin!==origin || req.headers['x-preview-token']!==token){res.writeHead(403).end();return;}
      let body='';for await(const chunk of req){body+=chunk;if(body.length>8_100_000){res.writeHead(413).end();return;}}
      const input=JSON.parse(body);
      if(!['ocr','translate'].includes(input.command))throw new Error('Unsupported command');
      const result=await call(input);
      // Evidence is restricted to this explicitly supplied demonstration content.
      await writeFile(resolve(output,`${result.id}-${input.command}.json`),JSON.stringify({...result,input:input.command==='translate'?input:undefined},null,2));
      if(input.command==='ocr')await writeFile(resolve(output,`${result.id}-source.png`),Buffer.from(input.image,'base64'));
      res.writeHead(200,{'Content-Type':'application/json','Cache-Control':'no-store'}).end(JSON.stringify(result));return;
    }
    let name=url.pathname==='/'?'text-extractor.html':url.pathname.slice(1);
    if(!/^[a-z0-9-]+\.(html|css|js)$/.test(name)&&!['translation-icons/translate-icon.svg','translation-icons/key-icon-yellow.svg'].includes(name)){res.writeHead(404).end();return;}
    let body=await readFile(name==='preview-bridge.js'?resolve(root,'scripts/text-extractor/preview-bridge.js'):resolve(root,'src',name),'utf8');
    if(name.endsWith('.html'))body=body.replace('<head>',`<head><script>window.__PULSE_LIVE_PREVIEW__=${JSON.stringify(token)};</script><script src="/preview-bridge.js"></script>`);
    res.writeHead(200,{'Content-Type':name.endsWith('.html')?'text/html':name.endsWith('.css')?'text/css':name.endsWith('.svg')?'image/svg+xml':'text/javascript','Cache-Control':'no-store'}).end(body);
  }catch(error){res.writeHead(500,{'Content-Type':'application/json'}).end(JSON.stringify({error:String(error)}));}
});
server.listen(port,'127.0.0.1',()=>console.log(`Live preview: ${origin}/?platform=macos`));
const stop=()=>{server.close();worker.stdin.end();worker.kill('SIGTERM');};
process.once('SIGINT',stop);process.once('SIGTERM',stop);
