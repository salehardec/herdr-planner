/* Herdr Planner — the Galaxy of the board. Boots the page: the world (three.js), the panels (Preact),
   the event stream, the keyboard. */
import './styles/base.css';
import { effect } from '@preact/signals';
import { render, h } from 'preact';
import { SPRITE } from './ui/glyphs';
import { App } from './ui/App';
import {
  aim, answered, armed, attention, currentView, edge, filter, flight, fresh, galaxyId, hover, invertY, level, locked, mode, narrow, notifyOn,
  panels, QUERY, requests, selected, sensitivity, showQuiet, simple, snap, speed, universe, webgl, views, SIMPLE_KEY, persist,
  linkStatus, linkSel,
} from './store';
import { connect, dropDetails, onState } from './api';
import { setLang, lang, t } from './i18n';
import { createWorld } from './world';
import { clearStack, getWorld, goHome, goOverview, navigateTo, openRequest, setWorld, enterGalaxy, back } from './nav';
import { escape, installKeys, setMode } from './keys';
import { effectiveWidth, installScale } from './scale';
import { chatWide, installChatWide, installWideRoom } from './chat-wide';
import { connectTasks } from './actions';
import { installTodoDnD } from './ui/Todo';
import { toast } from './toast';
import { warpSound } from './sound';
import { firstSentence } from './util/format';
import { Status } from './types';
import type { Snapshot } from './types';

const $ = (id: string): HTMLElement => document.getElementById(id) as HTMLElement;

document.documentElement.lang = lang.value;
document.body.insertAdjacentHTML('afterbegin', SPRITE);

installScale();
narrow.value = effectiveWidth() < 900;
const world = createWorld({
  canvas: $('scene') as HTMLCanvasElement,
  labelsEl: $('labels'),
  warpEl: $('warp'),
  simple: simple.value,
  bloom: !QUERY.has('nobloom') && QUERY.get('bloom') !== '0',
  autoSimple: !QUERY.has('noauto') && !QUERY.has('calm'),
  sensitivity: sensitivity.value,
  invertY: invertY.value,
});
setWorld(world);
webgl.value = world.webgl;
if (!world.webgl) {
  panels.nav.value = true;
  panels.focus.value = false;
}

/* ───────── what the world tells the page ───────── */

const h2 = world.hooks;
h2.onPick = (e) => { void navigateTo(e); };
h2.onDive = (e) => { void navigateTo(e); };
h2.onEmpty = () => { linkSel.value = null; };
h2.onLink = (l, galaxy) => {
  linkSel.value = { ...l, galaxy };
  selected.value = { kind: 'planet', galaxy, id: l.to };
  world.setSelection(selected.value);
  panels.focus.value = true;
};
h2.onConnect = (from, to) => { void connectTasks(from, to, galaxyId.value); };
h2.onHover = (e) => { hover.value = e; };
h2.onLevel = (l, u) => { level.value = l; universe.value = u; };
h2.onEdge = (v) => { edge.value = v; };
h2.onLock = (v) => { locked.value = v; };
h2.onFlight = (phase, target) => { flight.value = { phase, target }; if (phase === 'warp') warpSound(); };
h2.onAim = (e, hold) => { aim.value = { entity: e, hold }; };
h2.onSpeed = (now, cruise) => { speed.value = { now, cruise }; };
h2.onMode = (m) => { mode.value = m; };
h2.onAutoSimple = () => {
  simple.value = true;
  persist(SIMPLE_KEY, 'on');
  toast(t('toast.autoSimple'), { action: () => { simple.value = false; persist(SIMPLE_KEY, 'off'); }, label: t('toast.restore') });
};

/* ───────── page → world ───────── */

effect(() => {
  const s = snap.value;
  if (!s) return;
  const doc = document.documentElement;
  for (const [k, v] of Object.entries(s.theme.colors)) if (typeof v === 'string' && v.startsWith('#')) doc.style.setProperty(`--c-${k}`, v);
  doc.dataset.mode = s.theme.mode === 'light' ? 'light' : 'dark';
  world.setPalette(s.theme.colors, s.theme.mode === 'light' ? 'light' : 'dark');
});

effect(() => {
  const s = snap.value;
  if (!s) return;
  const view = currentView.value;
  const others = views.value.filter((v) => v.id !== view.id).map((v) => ({ id: v.id, label: v.home ? s.host : v.label, ok: v.ok }));
  world.setData({
    view,
    others,
    limits: s.limits,
    coordinator: view.home ? s.coordinator : null,
    attention: attention.value,
    fresh: fresh.value,
    showQuiet: showQuiet.value,
    bridges: s.bridges ?? [],
    filter: filter.value,
  });
});

effect(() => { world.setSimple(simple.value); document.getElementById('win')?.classList.toggle('calm', simple.value); });
effect(() => { world.setSensitivity(sensitivity.value, invertY.value); });
effect(() => {
  const w = document.getElementById('win');
  w?.classList.toggle('manual', mode.value === 'manual');
  w?.classList.toggle('todo-open', panels.todo.value);
  w?.classList.toggle('chat-open', panels.chat.value);
  w?.classList.toggle('chat-wide', panels.chat.value && chatWide.value);
  w?.classList.toggle('nav-off', !panels.nav.value);
  w?.classList.toggle('focus-off', !panels.focus.value);
});
effect(() => { world.setSelection(selected.value); });

/* ───────── first frame, changes, notifications ───────── */

let first = true;
let prevStatus = new Map<string, Status>();
let seenRequests = new Set<string>();
let audio: AudioContext | null = null;

function beep(): void {
  try {
    audio = audio ?? new (window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext)();
    const o = audio.createOscillator();
    const g = audio.createGain();
    o.type = 'sine';
    o.frequency.value = 880;
    g.gain.value = 0.04;
    o.connect(g).connect(audio.destination);
    o.start();
    o.stop(audio.currentTime + 0.12);
  } catch { /* no sound device */ }
}

onState((s: Snapshot) => {
  // Freshly done tasks glow for nine seconds.
  const now = Date.now();
  const next = new Map<string, Status>();
  for (const tk of s.tasks) {
    next.set(tk.id, tk.status);
    if (prevStatus.has(tk.id) && prevStatus.get(tk.id) !== tk.status) dropDetails(tk.id);
    if (!first && tk.status === 'done' && prevStatus.has(tk.id) && prevStatus.get(tk.id) !== 'done') {
      fresh.value = { ...fresh.value, [tk.id]: now };
      setTimeout(() => { const f = { ...fresh.value }; delete f[tk.id]; fresh.value = f; }, 9000);
    }
  }
  prevStatus = next;
  // A selected thing that has left the board is no longer selected.
  const sel = selected.value;
  if (sel && sel.kind === 'planet' && sel.galaxy === galaxyId.value && !currentView.value.tasks.some((x) => x.id === sel.id) && !s.archived.some((x) => x.id === sel.id)) {
    selected.value = null;
  }
  // New requests to the human: a sound and a notification, if switched on.
  const list = requests.value;
  const keys = new Set(list.map((r) => r.key));
  const fresh2 = list.filter((r) => !seenRequests.has(r.key));
  seenRequests = keys;
  if (!first && notifyOn.value && fresh2.length) {
    beep();
    if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
      const r = fresh2[0];
      try { new Notification('Herdr Planner: ' + r.title, { body: firstSentence(r.msg || r.text, 120), tag: r.key }); } catch { /* blocked */ }
    }
  }
  document.title = (attention.value ? `(${attention.value}) ` : '') + 'Herdr Planner';
  if (first) {
    first = false;
    void world.flyTo({ kind: 'overview' }, { instant: true });
    const open = list.find((r) => r.id !== 'coordinator');
    void open;
  }
});

/* ───────── start ───────── */

installChatWide();
installWideRoom();
installKeys();
installTodoDnD($('scene'));
const onResize = (): void => { narrow.value = effectiveWidth() < 900; };
window.addEventListener('resize', onResize);
onResize();
if (narrow.value) { panels.nav.value = false; panels.focus.value = false; }

render(h(App, null), $('app'));
connect();

/* handles for the checks and for the console */
const w = window as unknown as Record<string, unknown>;
w.world = world;
w.plannerDebug = () => ({
  galaxy: galaxyId.value, selected: selected.value, mode: mode.value, level: level.value, universe: universe.value,
  panels: Object.fromEntries(Object.entries(panels).map(([k, v]) => [k, v.value])), filter: filter.value,
  requests: requests.value.length, answered: [...answered.value], link: linkStatus.value, flight: flight.value, simple: simple.value,
  armed: armed.value,
});
w.setLang = setLang;
w.plannerSnap = () => snap.value;
/* the page's own commands, for the checks */
w.planner = { navigateTo, goHome, goOverview, openRequest, enterGalaxy, escape, back, setMode, connectTasks: (a: string, b: string) => connectTasks(a, b, galaxyId.value) };
void getWorld; void clearStack;
