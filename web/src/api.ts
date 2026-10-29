/* Talking to the server: JSON calls, and the event stream that replaces polling. */
import { chat, details, GATE_POLL_MS, gate, lastError, live, setLoad, snap } from './store';
import { t } from './i18n';
import { toast } from './toast';
import type { LoadInfo, Snapshot } from './types';

/** The phone's light page asks the same paths under `/m/api`: its routes are a narrow slice of the planner's with the same shapes. */
let lite = false;
export const useLiteApi = (): void => { lite = true; };
export const isLite = (): boolean => lite;
const route = (path: string): string => (lite && path.startsWith('/api/') ? '/m' + path : path);

export class ApiError extends Error {
  status = 0;
  data: any = null;
}

export async function api<T = any>(path: string, opts: RequestInit = {}): Promise<T> {
  // A reading that breaks on the way (the browser says «Failed to fetch» and has no answer of the server: a connection reset,
  // a socket the server closed at that moment) is asked again, twice, a moment later. Only a reading: asking again is harmless.
  const reading = !opts.method || opts.method.toUpperCase() === 'GET';
  let r: Response;
  for (let attempt = 0; ; attempt++) {
    try {
      r = await fetch(route(path), { credentials: 'same-origin', ...opts });
      break;
    } catch (e) {
      if (!reading || attempt >= 2) throw e;
      await new Promise((done) => setTimeout(done, 300 * (attempt + 1)));
    }
  }
  if (r.status === 401) {
    // the light page's own address answers with the sign-in page (the gate's, or the planner's login) to a visitor who is not in
    location.href = lite ? '/m' : '/login?next=' + encodeURIComponent(location.pathname + location.search);
    throw new ApiError('unauthorized');
  }
  const data = await r.json().catch(() => ({}));
  // Through the gate what only the machine itself may do does not exist (404): say so rather than «not found».
  if (gate.value && r.status === 404 && data.error === 'not found') {
    const e = new ApiError(t('gate.local'));
    e.status = 404;
    throw e;
  }
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
const tunnelListeners = new Set<Listener>();
/** The gate changed (its state, or who is knocking): the plaque asks again. Also when the stream comes back. */
export const onTunnelEvent = (fn: Listener): (() => void) => { tunnelListeners.add(fn); return () => tunnelListeners.delete(fn); };
let stateListener: ((s: Snapshot) => void) | null = null;
export const onState = (fn: (s: Snapshot) => void): void => { stateListener = fn; };

let es: EventSource | null = null;
let fallback: number | null = null;
let polling: Promise<void> | null = null;

/** The `chat_rev` of the last projection (the light page's long-poll has no `chat` event: the number says the transcript grew). */
let chatRev: number | null = null;

function accept(s: Snapshot): void {
  // A GET started before a state event can finish after it. Never roll the page (and the next link edit) back to that
  // older reading. The time allows a restarted server to reset seq; seq orders readings in the same millisecond.
  const current = snap.value;
  if (current && (s.at < current.at || (s.at === current.at && s.seq < current.seq))) return;
  snap.value = s;
  if (typeof s.chat_rev === 'number' && s.chat_rev !== chatRev) {
    const first = chatRev === null;
    chatRev = s.chat_rev;
    if (!first) chatListeners.forEach((fn) => fn());
  }
  live.value = true;
  lastError.value = '';
  stateListener?.(s);
}

async function readState(): Promise<void> {
  try {
    accept(await api<Snapshot>('/api/state'));
    if (!lite) setLoad(await api<LoadInfo>('/api/load'));
  } catch (e) {
    if ((e as Error).message !== 'unauthorized') lastError.value = (e as Error).message;
    // a page that lives on asking has no stream to tell it that the server went away
    if (gate.value === 'poll') live.value = false;
    // the device lost the full version (the owner took it away): the planner's address leads on to the light page
    if (gate.value && (e as ApiError).status === 403) location.href = '/';
  }
}

function pollOnce(): Promise<void> {
  // Actions await refreshNow before reading the current links. Share the read itself, so another caller cannot
  // continue with stale state just because a poll is already in flight.
  return polling ??= readState().finally(() => { polling = null; });
}

export const refreshNow = pollOnce;

/** Opens the event stream. If it breaks, the page asks every 30 s until the stream is back. Where the stream is not to be had
 *  (through a quick tunnel: the server answers `/api/events` with 410) the page does not try it, and asks every few seconds. */
export function connect(): void {
  if (gate.value === 'poll') {
    connectEvery(GATE_POLL_MS);
    return;
  }
  const open = (): void => {
    es?.close();
    es = new EventSource('/api/events');
    es.addEventListener('state', (e) => {
      try { accept(JSON.parse((e as MessageEvent).data)); } catch { /* a broken frame */ }
      if (fallback) { clearInterval(fallback); fallback = null; }
    });
    es.addEventListener('load', (e) => {
      try { setLoad(JSON.parse((e as MessageEvent).data)); } catch { /* a broken frame */ }
    });
    es.addEventListener('chat', () => chatListeners.forEach((fn) => fn()));
    es.addEventListener('tunnel', () => tunnelListeners.forEach((fn) => fn()));
    es.addEventListener('devices', () => tunnelListeners.forEach((fn) => fn()));
    es.onopen = () => tunnelListeners.forEach((fn) => fn());
    es.addEventListener('progress', (e) => {
      try { const p = JSON.parse((e as MessageEvent).data); progressListeners.forEach((fn) => fn(p)); } catch { /* a broken frame */ }
    });
    es.addEventListener('toast', (e) => {
      try {
        const p = JSON.parse((e as MessageEvent).data);
        if (p && typeof p.key === 'string') toast(t(p.key, p.vars ?? {}), { icon: p.icon ?? 'done' });
      } catch { /* a broken frame */ }
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

/** The news without the event stream for the whole planner: the state and the load every `ms`, none while the page is hidden. */
function connectEvery(ms: number): void {
  void pollOnce();
  window.setInterval(() => { if (!document.hidden) void pollOnce(); }, ms);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) void pollOnce(); });
}

const later = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

/**
 * The same news without the event stream (a quick tunnel does not pass one): one request in flight, held by the server until the
 * projection changes (25 s at most, then 204), and asked again. A page that is hidden waits; one that cannot reach the server
 * says so and tries again, a little slower each time.
 */
export function connectPoll(): void {
  let wake: (() => void) | null = null;
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) return;
    wake?.();
    void pollOnce();
  });
  void (async () => {
    let bad = 0;
    for (;;) {
      if (document.hidden) await new Promise<void>((done) => { wake = done; });
      wake = null;
      try {
        const seq = snap.value?.seq;
        if (!seq) {
          accept(await api<Snapshot>('/api/state'));
        } else {
          const r = await fetch(`/m/api/poll?seq=${seq}`, { credentials: 'same-origin', cache: 'no-store' });
          if (r.status === 401) { location.href = '/m'; return; }
          if (r.status === 200) accept(await r.json());
          else if (r.status !== 204) throw new Error(`HTTP ${r.status}`);
        }
        bad = 0;
      } catch (e) {
        if ((e as Error).message === 'unauthorized') return;
        live.value = false;
        bad++;
        await later(Math.min(15000, 1000 * bad));
      }
    }
  })();
}

export function dropDetails(id: string): void {
  if (details.value[id]) {
    const next = { ...details.value };
    delete next[id];
    details.value = next;
  }
}

void chat;
