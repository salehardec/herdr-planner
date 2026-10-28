/* The Coordinator's chat: the last turns of its transcript, older ones on demand, search,
   and sending (text and files: pasted screenshots, images, sound, text). */
import { signal } from '@preact/signals';
import { api, post, refreshNow } from './api';
import { chat, snap } from './store';
import type { TurnData } from './types';
import { t } from './i18n';
import { makeShelf, type SavedFile } from './attach';
import { chatRead, settleRead } from './chat-unread';

/** The files waiting in the message line. */
const shelf = makeShelf();
export const pendingFiles = shelf.files;
export const addFiles = shelf.add;
export const removeFile = shelf.remove;
export const sending = signal(false);
export const optimistic = signal<{ text: string; at: number; attachments: { id: string; name: string }[] } | null>(null);
/** Set when a search hit or a key asks the log to show a given turn. */
export const scrollTo = signal<{ n: number; at: number; block?: 'start' | 'center' } | null>(null);
/** A draft for the message line (a todo item dropped on the chat): the line takes the text once. */
export const draftRequest = signal<{ text: string; at: number } | null>(null);

const update = (patch: Partial<ReturnType<typeof chat.peek>>): void => { chat.value = { ...chat.value, ...patch }; };

function merge(list: TurnData[]): void {
  const map = new Map(chat.value.turns.map((x) => [x.n, x]));
  for (const x of list) map.set(x.n, x);
  update({ turns: [...map.values()].sort((a, b) => a.n - b.n) });
}

export const hasOlder = (): boolean => chat.value.turns.length > 0 && chat.value.turns[0].n > 0;

export async function loadLatest(limit = 3): Promise<void> {
  if (chat.value.loadingLatest) return;
  update({ loadingLatest: true });
  try {
    const r = await api<{ available: boolean; turns: TurnData[]; total: number }>(`/api/chat/history?limit=${limit}`);
    const patch: Partial<ReturnType<typeof chat.peek>> = { available: !!r.available, total: r.total };
    chat.value = { ...chat.value, ...patch };
    if (r.available) {
      const have = chat.value.turns.length ? chat.value.turns[chat.value.turns.length - 1].n : -1;
      // More than a page arrived since the last look: start again from the end.
      if (have >= 0 && r.turns.length && r.turns[0].n > have + 1) update({ turns: [] });
      merge(r.turns);
      settleRead();
    }
  } catch { /* the compact tail stays */ } finally {
    update({ loadingLatest: false });
  }
  await coverRead();
}

/**
 * The turns that were read keep the words they were read as, and an answer may have changed since: they have to be loaded to be
 * checked, though a page that has just started holds only the last few. Loads older turns down to the oldest of them.
 */
async function coverRead(): Promise<void> {
  const rs = chatRead.value;
  const keys = rs ? Object.keys(rs.seen).map(Number) : [];
  if (!keys.length) return;
  const lowest = Math.min(...keys);
  let got = false;
  for (let i = 0; i < 20 && chat.value.turns.length && chat.value.turns[0].n > lowest && hasOlder(); i++) {
    if (!(await loadOlder())) break;
    got = true;
  }
  if (got) settleRead();
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

/* ───────── files ───────── */

/** The todo items whose words and files are in the message line now. They stay in the list until the message is sent. */
export const draftTodos = signal<string[]>([]);
/** What happens to those items once the message has gone (set by the todo actions: they leave the list). */
export const draftSent: { fn: (ids: string[]) => void } = { fn: () => {} };

/** A todo item becomes the draft of a message to the Coordinator, to be corrected and sent from the line. */
export function draftMessage(id: string, text: string, files: SavedFile[]): boolean {
  if (draftTodos.value.includes(id)) return false;
  shelf.adopt(files);
  draftTodos.value = [...draftTodos.value, id];
  draftRequest.value = { text, at: Date.now() };
  return true;
}

/** Sends text (and the ready files) to the Coordinator. */
export async function send(text: string): Promise<boolean> {
  if (sending.value) return false;
  const body = text.trim();
  const files = [...pendingFiles.value];
  if (!body && !files.length) return false;
  sending.value = true;
  try {
    const got = await shelf.settled();
    if (!got.ok) { update({ error: t('chat.imgFailed', { why: got.why }) }); return false; }
    const attachments = got.saved;
    update({ error: '' });
    optimistic.value = { text: body, at: Date.now(), attachments };
    notePending(body, attachments);
    try {
      await post('/api/chat', { text: body, attachments });
      const keys = new Set(files.map((p) => p.key));
      for (const k of keys) shelf.remove(k);
      const sentTodos = draftTodos.value;
      draftTodos.value = [];
      if (sentTodos.length) draftSent.fn(sentTodos);
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
