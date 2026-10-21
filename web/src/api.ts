/* Talking to the server: JSON calls, and the event stream that replaces polling. */
import { chat, details, lastError, live, snap } from './store';
import type { Snapshot } from './types';

export class ApiError extends Error {
  status = 0;
  data: any = null;
}

export async function api<T = any>(path: string, opts: RequestInit = {}): Promise<T> {
  const r = await fetch(path, { credentials: 'same-origin', ...opts });
  if (r.status === 401) {
    location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search);
    throw new ApiError('unauthorized');
  }
  const data = await r.json().catch(() => ({}));
  if (!r.ok) {
    const e = new ApiError(data.error || `HTTP ${r.status}`);
    e.status = r.status;
    e.data = data;
    throw e;
  }
  return data as T;
}

export const post = <T = any>(path: string, body?: unknown): Promise<T> =>
  api<T>(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body ?? {}) });

type Listener = () => void;
const chatListeners = new Set<Listener>();
export const onChatEvent = (fn: Listener): (() => void) => { chatListeners.add(fn); return () => chatListeners.delete(fn); };
const progressListeners = new Set<(p: any) => void>();
export const onProgress = (fn: (p: any) => void): (() => void) => { progressListeners.add(fn); return () => progressListeners.delete(fn); };
let stateListener: ((s: Snapshot) => void) | null = null;
export const onState = (fn: (s: Snapshot) => void): void => { stateListener = fn; };

let es: EventSource | null = null;
let fallback: number | null = null;
let polling = false;

function accept(s: Snapshot): void {
  snap.value = s;
  live.value = true;
  lastError.value = '';
  stateListener?.(s);
}

async function pollOnce(): Promise<void> {
  if (polling) return;
  polling = true;
  try {
    accept(await api<Snapshot>('/api/state'));
  } catch (e) {
    if ((e as Error).message !== 'unauthorized') lastError.value = (e as Error).message;
  } finally {
    polling = false;
  }
}

export const refreshNow = pollOnce;

/** Opens the event stream. If it breaks, the page asks every 30 s until the stream is back. */
export function connect(): void {
  const open = (): void => {
    es?.close();
    es = new EventSource('/api/events');
    es.addEventListener('state', (e) => {
      try { accept(JSON.parse((e as MessageEvent).data)); } catch { /* a broken frame */ }
      if (fallback) { clearInterval(fallback); fallback = null; }
    });
    es.addEventListener('chat', () => chatListeners.forEach((fn) => fn()));
    es.addEventListener('progress', (e) => {
      try { const p = JSON.parse((e as MessageEvent).data); progressListeners.forEach((fn) => fn(p)); } catch { /* a broken frame */ }
    });
    es.onerror = () => {
      live.value = false;
      if (!fallback) fallback = window.setInterval(() => { void pollOnce(); }, 30000);
      void pollOnce();
    };
  };
  open();
  document.addEventListener('visibilitychange', () => { if (!document.hidden) void pollOnce(); });
}

export function dropDetails(id: string): void {
  if (details.value[id]) {
    const next = { ...details.value };
    delete next[id];
    details.value = next;
  }
}

void chat;
