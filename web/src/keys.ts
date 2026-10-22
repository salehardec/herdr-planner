/* The keyboard. Letters are read by physical key (`code`), so they work in any layout.
   Esc is exactly one step back (see nav.ts). */
import { batch } from '@preact/signals';
import {
  answered, armed, attention, chat, currentView, linkMode, mode, panels, requests, searchState, selected,
  simple, filter, universe, SIMPLE_KEY, persist, locked, level, selectedTask,
} from './store';
import { back, getWorld, goHome, goOverview, navigateTo, openRequest, pushFrame, queueCursor, fromQueue } from './nav';
import { STATUS_ORDER } from './types';
import type { Entity } from './types';
import { toast } from './toast';
import { t } from './i18n';
import { addDialogs, closeDialogs, dialogOpen } from './dialogs';
import { resetScale, stepScale } from './scale';
import { chatBack } from './chat-wide';
import { card, closeCard } from './ui/hovercard';

export function setMode(m: 'auto' | 'manual', lock = false): void {
  const w = getWorld();
  if (!w || mode.value === m) {
    if (m === 'manual' && lock) w?.setMode('manual', { lock: true });
    return;
  }
  if (m === 'manual') pushFrame();
  mode.value = m;
  w.setMode(m, { lock });
}

export const toggleMode = (): void => setMode(mode.value === 'auto' ? 'manual' : 'auto');

export function setLinkMode(v: boolean): void {
  linkMode.value = v;
  getWorld()?.setLinkMode(v);
  if (v) toast(t('toast.linkMode'), { icon: 'link', timeout: 5000 });
}

export function openChat(open = true): void {
  panels.chat.value = open;
  if (open) setTimeout(() => (document.getElementById('ask-in') as HTMLTextAreaElement | null)?.focus({ preventScroll: true }), 30);
}

export function focusSearch(): void {
  const el = document.getElementById('q') as HTMLInputElement | null;
  if (el) { el.focus(); el.select(); }
  searchState.value = { ...searchState.value, open: true };
}

export function toggleSimple(): void {
  simple.value = !simple.value;
  persist(SIMPLE_KEY, simple.value ? 'on' : 'off');
}

function sortedPlanets(): Entity[] {
  const order = (s: string): number => STATUS_ORDER.indexOf(s as never);
  return currentView.value.tasks
    .filter((x) => x.status !== 'done')
    .slice()
    .sort((a, b) => order(a.status) - order(b.status) || a.n - b.n)
    .map((x) => ({ kind: 'planet', id: x.id }) as Entity);
}

function nextPlanet(delta: number): void {
  const list = sortedPlanets();
  if (!list.length) return;
  const cur = selected.value;
  const i = list.findIndex((e) => cur && e.id === cur.id);
  void navigateTo(list[(i + delta + list.length) % list.length]);
}

function nextUniverse(delta: number): void {
  const names = [...new Set(currentView.value.projects.map((p) => p.name))].sort();
  if (!names.length) return;
  const cur = universe.value ?? '';
  const i = names.indexOf(cur);
  void navigateTo({ kind: 'universe', id: names[(i + delta + names.length) % names.length] });
}

/** Esc: the topmost overlay, then panels, then one step back along the way. */
export async function escape(fromTodo = false): Promise<void> {
  if (searchState.value.open) { searchState.value = { ...searchState.value, open: false }; (document.getElementById('q') as HTMLInputElement | null)?.blur(); return; }
  if (dialogOpen()) { closeDialogs(); return; }
  for (const k of ['help', 'limits', 'settings', 'herdr', 'quiet'] as const) {
    if (panels[k].value) { panels[k].value = false; return; }
  }
  if (armed.value) { armed.value = null; return; }
  // Esc typed in the drawer of todos folds the drawer; elsewhere the chat (the latest opened thing at the bottom) goes first.
  if (fromTodo && panels.todo.value) { panels.todo.value = false; return; }
  if (panels.chat.value) { chatBack(); if (!panels.chat.value) (document.activeElement as HTMLElement | null)?.blur(); return; }
  if (linkMode.value) { setLinkMode(false); return; }
  if (panels.todo.value) { panels.todo.value = false; return; }
  if (panels.log.value) { panels.log.value = false; return; }
  if (panels.archive.value) { panels.archive.value = false; return; }
  // The «Ждёт тебя» list is the place a request was opened from: Esc on the request returns to it (back()),
  // and Esc on the list itself closes it.
  if (panels.wait.value && !fromQueue.value) { panels.wait.value = false; return; }
  if (await back()) return;
  // Nothing on the stack: step out of what is selected, in place.
  if (selected.value) {
    batch(() => { selected.value = null; });
    getWorld()?.setSelection(null);
    return;
  }
  if (getWorld()?.level === 'nebula') await getWorld()?.leaveNebula();
}

const typing = (el: EventTarget | null): boolean => {
  const e = el as HTMLElement | null;
  return !!e && !!e.matches && e.matches('textarea,input,select,[contenteditable="true"]');
};

export function installKeys(): void {
  document.addEventListener('keydown', (e) => {
    const target = e.target;
    const code = e.code;
    // A dialog or the result window owns the keys: Esc still closes it, but «/», letters and digits must not
    // act behind it (the chat would open with its input hidden under the scrim).
    if (dialogOpen() && e.key !== 'Escape') return;
    // Search: from anywhere, the same key in every layout.
    if ((e.ctrlKey || e.metaKey) && code === 'KeyK') { e.preventDefault(); focusSearch(); return; }
    // The size of the interface (the browser's own page zoom is left alone: its menu and the wheel still work).
    if ((e.ctrlKey || e.metaKey) && !e.altKey) {
      if (code === 'Equal' || code === 'NumpadAdd') { e.preventDefault(); stepScale(1); return; }
      if (code === 'Minus' || code === 'NumpadSubtract') { e.preventDefault(); stepScale(-1); return; }
      if (code === 'Digit0' || code === 'Numpad0') { e.preventDefault(); resetScale(); return; }
    }
    if (e.key === 'Escape') {
      // the details card of the top bar goes first: one Esc, one thing
      if (card.value) { e.preventDefault(); closeCard(); return; }
      const inTodo = !!(target as HTMLElement | null)?.closest?.('#todo');
      if (typing(target) && (target as HTMLElement).id !== 'q' && (target as HTMLElement).id !== 'ask-in') (target as HTMLElement).blur();
      if (locked.value) return;
      e.preventDefault();
      void escape(inTodo);
      return;
    }
    if (typing(target) || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '/') { e.preventDefault(); openChat(true); return; }
    if (/^Digit[1-9]$/.test(code)) {
      const list = requests.value.filter((x) => x.type !== 'coordinator-options');
      const i = Number(code.slice(5)) - 1;
      if (list[i] && mode.value === 'auto') { e.preventDefault(); void openRequest(i); }
      return;
    }
    switch (code) {
      case 'KeyP': e.preventDefault(); toggleMode(); return;
      case 'KeyN': panels.nav.value = !panels.nav.value; return;
      case 'KeyF': panels.focus.value = !panels.focus.value; return;
      case 'KeyJ': panels.log.value = !panels.log.value; return;
      case 'KeyB': panels.wait.value = !panels.wait.value; if (panels.wait.value) fromQueue.value = false; return;
      case 'KeyR': panels.archive.value = !panels.archive.value; panels.focus.value = true; return;
      case 'KeyT': panels.todo.value = !panels.todo.value; return;
      case 'KeyL': setLinkMode(!linkMode.value); return;
      case 'KeyQ': e.preventDefault(); panels.limits.value = !panels.limits.value; return;
      case 'KeyH': void goHome(); return;
      case 'Digit0': case 'Home': e.preventDefault(); void goOverview(); return;
      case 'Enter': if (selected.value) { const w = getWorld(); if (w) void w.flyTo({ kind: 'entity', entity: selected.value }); } return;
      case 'Tab': if (!(target as HTMLElement | null)?.closest?.('button,a')) { e.preventDefault(); nextPlanet(e.shiftKey ? -1 : 1); } return;
      case 'BracketLeft': nextUniverse(-1); return;
      case 'BracketRight': nextUniverse(1); return;
      case 'Equal': case 'NumpadAdd': getWorld()?.zoomBy(0.86); return;
      case 'Minus': case 'NumpadSubtract': getWorld()?.zoomBy(1.16); return;
    }
    if (e.key === '?') { panels.help.value = !panels.help.value; return; }
    if (mode.value === 'auto') {
      if (code === 'ArrowLeft') { e.preventDefault(); getWorld()?.orbitBy(0.12, 0); }
      else if (code === 'ArrowRight') { e.preventDefault(); getWorld()?.orbitBy(-0.12, 0); }
      else if (code === 'ArrowUp') { e.preventDefault(); getWorld()?.orbitBy(0, -0.08); }
      else if (code === 'ArrowDown') { e.preventDefault(); getWorld()?.orbitBy(0, 0.08); }
    }
  });
}

void answered; void attention; void chat; void filter; void queueCursor; void level; void selectedTask; void addDialogs;
