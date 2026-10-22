/* What the human has not read yet in the Coordinator's chat: the answers (and board notices with their answers) that have not
   been in the visible part of an open chat while the window was active. «Read» is kept per turn in the browser, so it outlives
   a reload. A turn is told apart by its position in the transcript (`n`, which only grows). */
import { computed, signal } from '@preact/signals';
import { chat } from './store';
import { settled, unreadOf, withRead, type ReadState } from './chat-read';

export const READ_KEY = 'herdr-planner.chat.read';
/** How long a message stays in view before it counts as read (the light fades after that). */
export const SEE_MS = 900;

/* ───────── state in the browser ───────── */

function load(): ReadState | null {
  try {
    const v = JSON.parse(localStorage.getItem(READ_KEY) || 'null');
    if (v && typeof v.base === 'number' && v.seen && typeof v.seen === 'object' && !Array.isArray(v.seen)) return { base: v.base, seen: v.seen };
  } catch { /* nothing stored, or private mode */ }
  return null;
}

function save(rs: ReadState): void {
  try { localStorage.setItem(READ_KEY, JSON.stringify(rs)); } catch { /* private mode: the state lives until a reload */ }
}

export const chatRead = signal<ReadState | null>(load());
// Another tab of the planner read something: take its word.
window.addEventListener('storage', (e) => { if (e.key === READ_KEY) chatRead.value = load(); });

/** The positions of the unread turns, oldest first. */
export const unreadTurns = computed(() => unreadOf(chatRead.value, chat.value.turns, chat.value.total));
export const unreadCount = computed(() => unreadTurns.value.length);

/** The history has come: fixes the place the first time and after a restarted conversation. */
export function settleRead(): void {
  const c = chat.value;
  if (c.available !== true) return;
  const next = settled(chatRead.value, c.turns, c.total);
  if (next !== chatRead.value) { chatRead.value = next; save(next); }
}

/** Marks turns as read, each as the words that were in view (turn, signature). */
export function markRead(marks: [number, number][]): void {
  const rs = chatRead.value;
  if (!rs || !marks.length) return;
  const next = withRead(rs, marks, chat.peek().total, chat.peek().turns);
  chatRead.value = next;
  save(next);
}

/* ───────── is the window in use ───────── */

const inUse = (): boolean => document.visibilityState === 'visible' && document.hasFocus();
/** The planner's window is the one in front: shown and focused. A message read in any other state is still news. */
export const windowActive = signal(inUse());
const refresh = (): void => { windowActive.value = inUse(); };
for (const ev of ['focus', 'blur', 'pageshow']) window.addEventListener(ev, refresh);
document.addEventListener('visibilitychange', refresh);
