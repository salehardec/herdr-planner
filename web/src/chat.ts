/* The Coordinator's chat: the last turns of its transcript, older ones on demand, search,
   and sending (text and files: pasted screenshots, images, sound, text). */
import { signal } from '@preact/signals';
import { api, post, refreshNow } from './api';
import { chat } from './store';
import { prepareAnswer } from './nav';
import { reconcileEchoes, restoreEchoes, type Echo } from './chat-echo';
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
export const ECHO_KEY = 'planner.chat.echoes';
const readEchoes = (): Echo[] => {
  try {
    const restored = restoreEchoes(localStorage.getItem(ECHO_KEY));
    const live = reconcileEchoes(restored, []);
    // Do not carry an expired command through every reload, even when no transcript is available.
    if (live.length !== restored.length) {
      if (live.length) localStorage.setItem(ECHO_KEY, JSON.stringify(live)); else localStorage.removeItem(ECHO_KEY);
    }
    return live;
  } catch { return []; }
};
const echoes = signal<Echo[]>(readEchoes());
const saveEchoes = (list: Echo[]): void => {
  echoes.value = list;
  try { if (list.length) localStorage.setItem(ECHO_KEY, JSON.stringify(list)); else localStorage.removeItem(ECHO_KEY); } catch { /* private mode */ }
};
function settleEchoes(): void {
  const list = reconcileEchoes(echoes.peek(), chat.peek().turns);
  const base = Math.min(...list.filter((e) => e.delivered === undefined).map((e) => e.base));
  // A delivered receipt below every remaining send's boundary can no longer be mistaken for that send.
  saveEchoes(list.filter((e) => e.delivered === undefined || e.delivered > base));
}
export const pendingMessages = (): Echo[] => reconcileEchoes(echoes.value, chat.value.turns).filter((e) => e.delivered === undefined);
export const forgetPending = (id: string): void => saveEchoes(echoes.peek().filter((e) => e.id !== id));
/** Set when a search hit or a key asks the log to show a given turn. */
export const scrollTo = signal<{ n: number; at: number; block?: 'start' | 'center' } | null>(null);
/** A draft for the message line (a todo item dropped on the chat): the line takes the text once. */
export const draftRequest = signal<{ text: string; at: number } | null>(null);

const update = (patch: Partial<ReturnType<typeof chat.peek>>): void => { chat.value = { ...chat.value, ...patch }; };

function merge(list: TurnData[]): void {
  const map = new Map(chat.value.turns.map((x) => [x.n, x]));
  for (const x of list) map.set(x.n, x);
  update({ turns: [...map.values()].sort((a, b) => a.n - b.n) });
  settleEchoes();
}

export const hasOlder = (): boolean => chat.value.turns.length > 0 && chat.value.turns[0].n > 0;

let latestAgain = 0;
export async function loadLatest(limit = 3): Promise<void> {
  if (chat.value.loadingLatest) { latestAgain = Math.max(latestAgain, limit); return; }
  update({ loadingLatest: true });
  try {
    const r = await api<{ available: boolean; turns: TurnData[]; total: number }>(`/api/chat/history?limit=${limit}`);
    const patch: Partial<ReturnType<typeof chat.peek>> = { available: !!r.available, total: r.total };
    chat.value = { ...chat.value, ...patch };
    if (r.available) {
      const have = chat.value.turns.length ? chat.value.turns[chat.value.turns.length - 1].n : -1;
      // More than one page arrived since the last look. Load the gap before counting: positions without their turns could be
      // notices, so counting them would make reconnects and polling invent unread messages.
      const gap = have >= 0 && r.turns.length && r.turns[0].n > have + 1;
      if (gap) update({ turns: [] });
      merge(r.turns);
      if (gap) {
        for (let i = 0; i < 40 && chat.value.turns[0]?.n > have + 1 && hasOlder(); i++) {
          if (!(await loadOlderWait(100))) break;
        }
      }
      // Reloads and reconnects may return only the tail while an echo belongs in an older page.
      // Cover its transcript boundary before deciding it is still waiting.
      const base = Math.min(...echoes.peek().filter((e) => e.delivered === undefined).map((e) => e.base + 1));
      for (let i = 0; i < 40 && chat.value.turns[0]?.n > base && hasOlder(); i++) {
        if (!(await loadOlderWait(100))) break;
      }
      settleRead();
      // Still part of this load: whoever waits for the history (the jump to the first unread) waits for the covering, too.
      await coverRead();
    }
  } catch { /* the compact tail stays */ } finally {
    update({ loadingLatest: false });
    if (latestAgain) { const limit = latestAgain; latestAgain = 0; void loadLatest(limit); }
  }
}

/**
 * The answers that were read keep the words they were read as, and an answer may have changed since: they have to be loaded to
 * be checked, though a page that has just started holds only the last few. Board notices have the zero signature and cannot
 * become answers, so they do not pull old history back into a reloaded chat.
 */
async function coverRead(): Promise<void> {
  const rs = chatRead.value;
  if (!rs) return;
  const keys = Object.entries(rs.seen).filter(([, sig]) => sig !== 0).map(([n]) => Number(n));
  // Everything above the stored place is loaded too: a turn that is not there could be a board line or an answer, and an
  // unread answer must not drop out of the count only because notices came after it.
  const first = rs.base + 1 < chat.value.total ? rs.base + 1 : Infinity;
  const lowest = Math.min(first, ...keys);
  if (lowest === Infinity) return;
  let got = false;
  for (let i = 0; i < 20 && chat.value.turns.length && chat.value.turns[0].n > lowest && hasOlder(); i++) {
    if (!(await loadOlderWait(100))) break;
    got = true;
  }
  if (got) settleRead();
}

/** Like `loadOlder`, but a load that is already going (the reader scrolled to the top) is waited for, not taken for «nothing more». */
export async function loadOlderWait(limit = 5): Promise<boolean> {
  for (let i = 0; i < 100 && chat.value.loading; i++) await new Promise((r) => setTimeout(r, 100));
  return loadOlder(limit);
}

/** Older turns, five at a time. Returns true when something was added. */
export async function loadOlder(limit = 5): Promise<boolean> {
  const c = chat.value;
  if (c.loading || !hasOlder()) return false;
  update({ loading: true });
  try {
    const r = await api<{ turns: TurnData[] }>('/api/chat/history?limit=' + limit + '&before=' + c.turns[0].n);
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
export function notePending(text: string, attachments: { id: string; name: string }[] = []): string {
  const turns = chat.value.turns;
  const id = Array.from(crypto.getRandomValues(new Uint32Array(4)), (n) => n.toString(16)).join('-');
  saveEchoes([...echoes.peek(), { id, text, attachments, base: turns.length ? turns[turns.length - 1].n : -1, at: Date.now() }]);
  return id;
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
    const receipt = notePending(body, attachments);
    const answered = prepareAnswer('coordinator', 'coordinator');
    try {
      await post('/api/chat', { text: body, attachments });
      void answered();
      const keys = new Set(files.map((p) => p.key));
      for (const k of keys) shelf.remove(k);
      const sentTodos = draftTodos.value;
      draftTodos.value = [];
      if (sentTodos.length) draftSent.fn(sentTodos);
      setTimeout(() => { void refreshNow(); void loadLatest(); }, 800);
      setTimeout(() => { void refreshNow(); void loadLatest(); }, 2400);
      return true;
    } catch (e) {
      forgetPending(receipt);
      update({ error: t('chat.sendFailed', { why: (e as Error).message }) });
      return false;
    }
  } finally {
    sending.value = false;
  }
}
