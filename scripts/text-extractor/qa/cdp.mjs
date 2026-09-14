export async function targets(port) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(1500) });
  if (!response.ok) throw new Error(`Debug endpoint HTTP ${response.status}`);
  return response.json();
}
export class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url); this.pending = new Map(); this.next = 0; this.listeners = new Set();
    this.opened = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Debug connection timed out')), 5000);
      this.socket.addEventListener('open', () => { clearTimeout(timeout); resolve(); }, { once: true });
      this.socket.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('Debug connection failed')); }, { once: true });
    });
    this.socket.addEventListener('message', event => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const item = this.pending.get(message.id); if (!item) return;
        this.pending.delete(message.id); clearTimeout(item.timeout);
        message.error ? item.reject(new Error(message.error.message)) : item.resolve(message.result);
      } else for (const listener of this.listeners) listener(message);
    });
    this.socket.addEventListener('close', () => {
      for (const item of this.pending.values()) { clearTimeout(item.timeout); item.reject(new Error('Debug target closed')); }
      this.pending.clear();
    });
  }
  async send(method, params = {}) {
    await this.opened;
    return new Promise((resolve, reject) => {
      if (this.socket.readyState !== WebSocket.OPEN) { reject(new Error('Debug target closed')); return; }
      const id = ++this.next;
      const timeout = setTimeout(() => { this.pending.delete(id); reject(new Error(`${method} timed out`)); }, 10000);
      this.pending.set(id, { resolve, reject, timeout });
      try { this.socket.send(JSON.stringify({ id, method, params })); }
      catch (error) { clearTimeout(timeout); this.pending.delete(id); reject(error); }
    });
  }
  async evaluate(expression) {
    const response = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
    return response.result.value;
  }
  close() { this.socket.close(); }
}
