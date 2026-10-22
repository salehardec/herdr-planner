/* The chat on the whole window: navigator | (todos) | chat | focus, the world dimmed behind. Whether it is wide and
   whether the todos are open beside it is remembered in this browser. */
import { effect, signal } from '@preact/signals';
import { panels, persist } from './store';
import { rootPx, scaleTick } from './scale';
import { navWidth } from './nav-width';

export const WIDE_KEY = 'herdr-planner.chatWide';
export const WIDE_TODO_KEY = 'herdr-planner.chatWideTodo';

const read = (k: string): boolean => { try { return localStorage.getItem(k) === '1'; } catch { return false; } };

export const chatWide = signal(read(WIDE_KEY));

/** Wide or back to the usual place at the bottom. Wide brings the todos back as they were left in it. */
export function setChatWide(on: boolean): void {
  if (chatWide.value === on) return;
  chatWide.value = on;
  persist(WIDE_KEY, on ? '1' : '0');
  if (on) { panels.chat.value = true; panels.todo.value = read(WIDE_TODO_KEY); }
}

/** Esc and the fold button: the todos beside the chat, then the wide chat, then the chat itself. */
export function chatBack(): void {
  // On a window too narrow for columns (as the interface sees it, scale included) the chat is not shown wide.
  const wide = chatWide.value && !document.documentElement.classList.contains('bp1000');
  if (panels.chat.value && wide && panels.todo.value) { panels.todo.value = false; return; }
  if (panels.chat.value && wide) { setChatWide(false); return; }
  panels.chat.value = false;
}

/** After a reload: a wide chat is open again, with its todos. Then the todos' state is kept while it is wide. */
export function installChatWide(): void {
  if (chatWide.value) { panels.chat.value = true; panels.todo.value = read(WIDE_TODO_KEY); }
  effect(() => {
    if (chatWide.value && panels.chat.value) persist(WIDE_TODO_KEY, panels.todo.value ? '1' : '0');
  });
}

/* The middle between the navigator and the focus card (as wide as the interface's own size makes it, rem) must be wide enough for what
   opens in it. Two measures, two classes on #win: `chat-tight` (less than a chat that can be used, 30 rem) and `wide-tight` (less than the
   todos' column and such a chat).
   With the todos beside the chat the middle needs room for both: a column of todos (20 rem) and a chat that can be
   used (30 rem). Where the middle between the navigator and the focus card is smaller, the focus card steps aside
   while the todos are open (it comes back when they close), so the chat is never squeezed. */
const TODO_COL_REM = 20;
const CHAT_MIN_REM = 30;
const GAP_REM = 0.75;

export function installWideRoom(): void {
  const win = document.getElementById('win');
  if (!win) return;
  // A probe that is as wide as the middle would be with the navigator and the focus card both in their places.
  const probe = document.createElement('div');
  probe.setAttribute('aria-hidden', 'true');
  probe.style.cssText = 'position:absolute;top:0;height:0;visibility:hidden;pointer-events:none';
  win.appendChild(probe);
  const measure = (): void => {
    void scaleTick.value;
    void navWidth.value;
    probe.style.left = panels.nav.value ? 'var(--mid-l-nav)' : '0.75rem';
    probe.style.right = panels.focus.value ? 'var(--mid-r-focus)' : '0.75rem';
    const room = probe.getBoundingClientRect().width;
    const need = (TODO_COL_REM + CHAT_MIN_REM + GAP_REM) * rootPx();
    win.classList.toggle('wide-tight', room < need);
    // not even room for the chat alone (its header needs about 27 rem): the focus card steps aside while the chat or the todos are open
    win.classList.toggle('chat-tight', room < (CHAT_MIN_REM + GAP_REM) * rootPx());
  };
  effect(measure);
  addEventListener('resize', measure);
  if (typeof ResizeObserver !== 'undefined') new ResizeObserver(measure).observe(win);
}
