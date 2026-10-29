/* The phone's light page (m.html): the planner without the world. The same store, the same dictionary, the same cards' logic;
   the news come by long-poll. No three.js, no panels of the Galaxy. */
import '../styles/tokens.css';
import './mobile.css';
import { effect } from '@preact/signals';
import { h, render } from 'preact';
import { SPRITE } from '../ui/glyphs';
import { connectPoll, refreshNow, useLiteApi } from '../api';
import { lang, setLang } from '../i18n';
import { armed, attention, requests, snap } from '../store';
import { taskOpener } from '../task-open';
import { installRoomInput } from '../restroom/input';
import { baseKeys } from '../restroom/notes';
import { game, hooks, noteBase, noteLater, origin, roomOpen, wallId } from '../restroom/state';
import { App } from './App';
import { loadSession, session, sheet, tab, themePref } from './state';

useLiteApi();
installRoomInput();
hooks.calm = () => true;
hooks.entered = () => { noteBase.value = baseKeys(requests.value); noteLater.value = new Set(); };
hooks.fly = (to) => {
  sheet.value = to.id === 'coordinator' || to.type === 'runner-alert' ? null : { kind: 'task', id: to.id };
  if (to.id === 'coordinator' || to.type === 'runner-alert') tab.value = 'chat';
};
document.body.insertAdjacentHTML('afterbegin', SPRITE);
// A phone that never chose a language gets the one it speaks.
try {
  if (!localStorage.getItem('herdr-planner.lang') && !/^ru\b/i.test(navigator.language || '')) lang.value = 'en';
} catch { /* private mode */ }
document.documentElement.lang = lang.value;

taskOpener.open = (task) => { sheet.value = { kind: 'task', id: task.id }; };

/* The board's colours (the server put them in the page; the snapshot keeps them current) and the mode: the phone's own choice
   wins. A mode that is not the board's has no colours of the board's — the page's own palette for it is in mobile.css. */
const dark = matchMedia('(prefers-color-scheme: dark)');
const doc = document.documentElement;
const boardStyle = document.getElementById('board-theme') as HTMLStyleElement | null;
doc.dataset.boardMode = doc.dataset.mode === 'light' ? 'light' : 'dark';
const COLORS = new Set<string>();
const paint = (): void => {
  const s = snap.value;
  const boardMode = (s?.theme.mode ?? doc.dataset.boardMode ?? 'dark') === 'light' ? 'light' : 'dark';
  doc.dataset.boardMode = boardMode;
  const pref = themePref.value;
  const mode = pref === 'system' ? (dark.matches ? 'dark' : 'light') : pref;
  doc.dataset.mode = mode;
  const own = mode === boardMode;
  if (boardStyle) boardStyle.disabled = !own;
  for (const k of COLORS) doc.style.removeProperty(`--c-${k}`);
  COLORS.clear();
  if (!own || !s) return;
  for (const [k, v] of Object.entries(s.theme.colors)) {
    if (typeof v === 'string' && v.startsWith('#')) { doc.style.setProperty(`--c-${k}`, v); COLORS.add(k); }
  }
};
effect(paint);
dark.addEventListener('change', paint);
effect(() => { document.documentElement.lang = lang.value; });
effect(() => {
  const n = requests.value.length;
  document.title = (attention.value && n ? `(${attention.value}) ` : '') + 'Herdr Planner';
});

render(h(App, null), document.getElementById('app') as HTMLElement);
connectPoll();
void loadSession();
window.setInterval(() => { void loadSession(); }, 300000);

/* handles for the checks and for the console */
const w = window as unknown as Record<string, unknown>;
w.mobilePage = true;
w.plannerSnap = () => snap.value;
w.plannerDebug = () => ({ armed: armed.value, tab: tab.value, sheet: sheet.value });
w.mobile = { tab, sheet, session, refresh: refreshNow, setLang };

w.restroomGame = game;
w.restroomState = () => ({ open: roomOpen.value, wall: wallId.value, origin: origin.value });
