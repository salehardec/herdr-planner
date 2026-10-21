/* The Coordinator's chat: the last turns of its transcript, older ones on demand, search,
   and sending (text and pasted images). */
import { signal } from '@preact/signals';
import { api, post, refreshNow } from './api';
import { chat, galaxyId, snap } from './store';
import type { TurnData } from './types';
import { toast } from './toast';
import { t } from './i18n';

export const IMG_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);
export const IMG_MAX = 10 * 1024 * 1024;
export const IMG_MAX_COUNT = 8;

export interface PendingImage {
  key: number;
  name: string;
  size: number;
  state: 'uploading' | 'ready' | 'error';
  local: string;
  id: string;
  error: string;
  task: Promise<void> | null;
}

export const pendingImages = signal<PendingImage[]>([]);
export const sending = signal(false);
export const optimistic = signal<{ text: string; at: number; attachments: { id: string; name: string }[] } | null>(null);
/** Set when a search hit or a key asks the log to show a given turn. */
export const scrollTo = signal<{ n: number; at: number } | null>(null);
let imgSeq = 0;

const update = (patch: Partial<ReturnType<typeof chat.peek>>): void => { chat.value = { ...chat.value, ...patch }; };

function merge(list: TurnData[]): void {
  const map = new Map(chat.value.turns.map((x) => [x.n, x]));
  for (const x of list) map.set(x.n, x);
  update({ turns: [...map.values()].sort((a, b) => a.n - b.n) });
}

export const hasOlder = (): boolean => chat.value.turns.length > 0 && chat.value.turns[0].n > 0;

export async function loadLatest(): Promise<void> {
  if (chat.value.loadingLatest) return;
  update({ loadingLatest: true });
  try {
    const r = await api<{ available: boolean; turns: TurnData[]; total: number }>('/api/chat/history?limit=3');
    const patch: Partial<ReturnType<typeof chat.peek>> = { available: !!r.available, total: r.total };
    chat.value = { ...chat.value, ...patch };
    if (r.available) {
      const have = chat.value.turns.length ? chat.value.turns[chat.value.turns.length - 1].n : -1;
      // More than a page arrived since the last look: start again from the end.
      if (have >= 0 && r.turns.length && r.turns[0].n > have + 1) update({ turns: [] });
      merge(r.turns);
    }
  } catch { /* the compact tail stays */ } finally {
    update({ loadingLatest: false });
  }
}

/** Older turns, five at a time. Returns true when something was added. */
export async function loadOlder(): Promise<boolean> {
  const c = chat.value;
  if (c.loading || !hasOlder()) return false;
  update({ loading: true });
  try {
    const r = await api<{ turns: TurnData[] }>('/api/chat/history?limit=5&before=' + c.turns[0].n);
    merge(r.turns || []);
    return (r.turns || []).length > 0;
  } catch { return false; } finally {
    update({ loading: false });
  }
}

/** Shows turn `n` (from a search hit or a key): loads the turns around it and asks the log to scroll there. */
export async function openChatAt(n: number): Promise<void> {
  if (!chat.value.turns.some((x) => x.n === n)) {
    try {
      const r = await api<{ turns: TurnData[] }>(`/api/chat/history?limit=6&before=${n + 3}`);
      merge(r.turns || []);
    } catch { return; }
  }
  update({ highlight: n, q: '', hits: [] });
  scrollTo.value = { n, at: Date.now() };
  setTimeout(() => { if (chat.value.highlight === n) update({ highlight: null }); }, 2400);
}

let searchTimer = 0;
let searchSeq = 0;

export function searchChat(q: string): void {
  update({ q });
  window.clearTimeout(searchTimer);
  const text = q.trim();
  if (text.length < 2) { update({ hits: [], hitsBusy: false }); return; }
  const mine = ++searchSeq;
  update({ hitsBusy: true });
  searchTimer = window.setTimeout(async () => {
    try {
      const r = await api<{ hits: ReturnType<typeof chat.peek>['hits'] }>('/api/chat/history?limit=20&q=' + encodeURIComponent(text));
      if (mine === searchSeq) update({ hits: r.hits ?? [], hitsBusy: false });
    } catch { if (mine === searchSeq) update({ hitsBusy: false }); }
  }, 150);
}

/** A message sent from here stays in the open chat until the transcript has it. */
export function notePending(text: string, attachments: { id: string; name: string }[] = []): void {
  const turns = chat.value.turns;
  update({ pending: { text, attachments, base: turns.length ? turns[turns.length - 1].n : -1, at: Date.now() } });
}

export function pendingMessage(): ReturnType<typeof chat.peek>['pending'] {
  const p = chat.value.pending;
  if (!p) return null;
  const seen = chat.value.turns.some((x) => x.n > p.base && (!p.text || x.q.includes(p.text)));
  if (seen || Date.now() - p.at > 600000) { queueMicrotask(() => update({ pending: null })); return null; }
  return p;
}

/* ───────── images ───────── */

async function upload(item: PendingImage, file: File): Promise<void> {
  const form = new FormData();
  form.append('file', file, item.name);
  try {
    const r = await fetch('/api/chat/attach', { method: 'POST', credentials: 'same-origin', body: form });
    if (r.status === 401) { location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search); throw new Error('unauthorized'); }
    const data = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    item.id = data.attachment?.id || '';
    item.name = data.attachment?.name || item.name;
    if (!item.id) throw new Error(t('chat.noFile'));
    item.state = 'ready';
  } catch (e) {
    item.state = 'error';
    item.error = (e as Error).message;
    toast(t('chat.imgFailed', { why: item.error }));
  }
  pendingImages.value = [...pendingImages.value];
}

export function addFiles(files: FileList | File[] | null | undefined): void {
  for (const file of [...(files ?? [])]) {
    if (pendingImages.value.length >= IMG_MAX_COUNT) { toast(t('chat.imgMax', { n: IMG_MAX_COUNT })); break; }
    const type = String(file.type || '').toLowerCase();
    if (type && !IMG_TYPES.has(type)) { toast(t('chat.imgType')); continue; }
    if (!file.size) { toast(t('chat.imgEmpty')); continue; }
    if (file.size > IMG_MAX) { toast(t('chat.imgBig')); continue; }
    const item: PendingImage = {
      key: ++imgSeq, name: file.name || t('chat.image'), size: file.size, state: 'uploading', local: URL.createObjectURL(file), id: '', error: '', task: null,
    };
    pendingImages.value = [...pendingImages.value, item];
    item.task = upload(item, file);
  }
}

export function removeImage(key: number): void {
  const i = pendingImages.value.findIndex((p) => p.key === key);
  if (i < 0) return;
  const [p] = pendingImages.value.splice(i, 1);
  if (p.local) URL.revokeObjectURL(p.local);
  pendingImages.value = [...pendingImages.value];
}

/** Sends text (and the ready images) to the Coordinator of the galaxy the operator is in. */
export async function send(text: string): Promise<boolean> {
  if (sending.value) return false;
  const body = text.trim();
  const imgs = [...pendingImages.value];
  if (!body && !imgs.length) return false;
  sending.value = true;
  try {
    await Promise.all(imgs.map((p) => p.task).filter(Boolean));
    const failed = imgs.find((p) => p.state !== 'ready');
    if (failed) { update({ error: t('chat.imgFailed', { why: failed.error || t('chat.tryAgain') }) }); return false; }
    const attachments = imgs.map((p) => ({ id: p.id, name: p.name }));
    update({ error: '' });
    optimistic.value = { text: body, at: Date.now(), attachments };
    notePending(body, attachments);
    try {
      await post('/api/chat', { text: body, attachments, galaxy: galaxyId.value || undefined });
      for (const p of imgs) if (p.local) URL.revokeObjectURL(p.local);
      const keys = new Set(imgs.map((p) => p.key));
      pendingImages.value = pendingImages.value.filter((p) => !keys.has(p.key));
      setTimeout(() => { void refreshNow(); void loadLatest(); }, 800);
      setTimeout(() => { void refreshNow(); void loadLatest(); }, 2400);
      return true;
    } catch (e) {
      optimistic.value = null;
      update({ pending: null, error: t('chat.sendFailed', { why: (e as Error).message }) });
      return false;
    }
  } finally {
    sending.value = false;
  }
}

/** Drops the optimistic echo once the server's history has the message. */
export function reconcile(): void {
  const o = optimistic.value;
  const hist = snap.value?.coordinator.history ?? [];
  const last = hist[hist.length - 1];
  if (o && last && last.text === o.text && last.attachments.length === o.attachments.length) optimistic.value = null;
}
