/* The width of the panels that stand at a side of the window — the navigator, the focus card, «Ждёт тебя» — and of the chat
   docked in the middle: the operator drags the edge of a panel (or uses the arrow keys on it), and the width is remembered
   in this browser, in rem so that it keeps its proportion at any scale. A double click on the edge goes back to the default.
   Local storage is the page's own memory (as it is for the scale and the language): it lives with the browser profile, so a
   reload and a new start of the board and of the planner find the widths again.
   The page reads the widths as --nav-user, --focus-user, --wait-user and --chat-user on #win (base.css and the panels' sheets);
   a width kept from a bigger window is cut to what fits now, and the kept figure stays for when the window is big again. */
import { effect, signal } from '@preact/signals';
import { panels, persist } from './store';
import { effectiveWidth, rootPx, scaleTick } from './scale';

export type PanelId = 'nav' | 'focus' | 'wait' | 'chat';

export interface PanelSpec {
  /** how the panel stands: a column at the left edge, a column at the right edge, or centred in the middle */
  dock: 'left' | 'right' | 'center';
  /** the element the width belongs to */
  el: string;
  /** the key in this browser's storage and the variable on #win */
  key: string;
  css: string;
  /** the narrowest and the widest the panel may ever be, in rem (the window may still cut it smaller) */
  min: number;
  cap: number;
}

/** Every panel that can be resized. The narrowest figures are the widths at which their content still reads. */
export const PANELS: Record<PanelId, PanelSpec> = {
  nav: { dock: 'left', el: 'nav', key: 'herdr-planner.navW', css: '--nav-user', min: 16, cap: 48 },
  focus: { dock: 'right', el: 'focus', key: 'herdr-planner.focusW', css: '--focus-user', min: 16, cap: 48 },
  wait: { dock: 'left', el: 'drawer-wait', key: 'herdr-planner.waitW', css: '--wait-user', min: 15, cap: 40 },
  chat: { dock: 'center', el: 'chat', key: 'herdr-planner.chatW', css: '--chat-user', min: 30, cap: 80 },
};

const IDS = Object.keys(PANELS) as PanelId[];

/* The chat needs this much between the sides to be used (the figure the sheets and chat-wide.ts work with, in rem), and a
   side panel takes its width and this much beside it: 0.75 rem from the window's edge and 0.75 before the middle. */
const MID_MIN = 30;
const COLUMN = 1.5;
/** A side with no panel at all takes this much. */
const EDGE = 0.75;

const OPEN: Record<PanelId, () => boolean> = {
  nav: () => panels.nav.value,
  focus: () => panels.focus.value,
  wait: () => panels.wait.value,
  chat: () => panels.chat.value,
};

const read = (id: PanelId): number | null => {
  try {
    const v = Number(localStorage.getItem(PANELS[id].key));
    if (v >= PANELS[id].min && v <= PANELS[id].cap) return v;
  } catch { /* private mode */ }
  return null;
};

/** The widths the operator chose, in rem; null: the default of the style sheet. Read in a view to follow them. */
export const panelWidths = signal<Record<PanelId, number | null>>(
  Object.fromEntries(IDS.map((id) => [id, read(id)])) as Record<PanelId, number | null>,
);

/** The width a panel asks for, in rem: what the operator chose, or what the sheet gives it now (measured, while the choice
    is the sheet's own). A closed panel takes nothing. */
function wanted(id: PanelId): number {
  if (!OPEN[id]()) return 0;
  const chosen = panelWidths.value[id];
  if (chosen !== null) return Math.min(chosen, PANELS[id].cap);
  const el = document.getElementById(PANELS[id].el);
  return el ? el.getBoundingClientRect().width / rootPx() : PANELS[id].min;
}

interface Columns {
  /** the left and the right side columns with their gaps, and the middle between them, in rem */
  left: number;
  right: number;
  middle: number;
}

/** The two side columns and the middle between them as they stand now — the figures base.css calls --mid-l and --mid-r, and what
    the chat and the todos are given. A width the operator chose is cut to what the window leaves, the two columns together
    keeping the chat's 30 rem: the left one first, then the right against the left as it stands. A panel still on the sheet's own
    width is not cut here (the sheet's layout for narrow windows is the designer's). The left column is the wider of the navigator
    and «Ждёт тебя»: the list stands over the navigator. */
function columns(): Columns {
  // the window's width in rem (effectiveWidth() is in design pixels, 16 to the rem); the columns leave a gap at each edge
  const win = effectiveWidth() / 16;
  const nav = wanted('nav');
  const wait = wanted('wait');
  const leftId: PanelId = wait > nav ? 'wait' : 'nav';
  const focus = wanted('focus');
  const room = win - 2 * COLUMN - MID_MIN;
  let left = Math.max(nav, wait);
  let right = focus;
  if (left > 0 && panelWidths.value[leftId] !== null) left = Math.max(PANELS[leftId].min, Math.min(left, room - right));
  if (right > 0 && panelWidths.value.focus !== null) right = Math.max(PANELS.focus.min, Math.min(right, room - left));
  const l = left > 0 ? left + COLUMN : EDGE;
  let r = right > 0 ? right + COLUMN : EDGE;
  // the chat needs 30 rem and a gap; where the middle would be less, the focus card steps aside while the chat is open
  // (the `chat-tight` class, chat-wide.ts), so its column is not counted here either
  if (OPEN.chat() && win - l - r < MID_MIN + COLUMN) r = EDGE;
  return { left: l, right: r, middle: win - l - r };
}

/** The width the panel is drawn at now, in rem; null: the sheet's own (no width has been chosen, or the sheet's width is not
    cut here). The column is what the window leaves beside the other one. */
function shown(id: PanelId, c: Columns): number | null {
  const chosen = panelWidths.value[id];
  if (chosen === null) return null;
  const room = id === 'chat' ? c.middle : id === 'focus' ? c.right - COLUMN : c.left - COLUMN;
  return Math.max(PANELS[id].min, Math.min(room, chosen));
}

/** The most a panel may take at the size of the window now: the middle keeps the room the chat needs to be read. */
export function maxWidth(id: PanelId): number {
  const p = PANELS[id];
  const c = columns();
  const win = effectiveWidth() / 16;
  let room: number;
  if (p.dock === 'left') room = win - c.right - COLUMN - MID_MIN;
  else if (p.dock === 'right') room = win - c.left - COLUMN - MID_MIN;
  else room = c.middle;
  return Math.max(p.min, Math.min(p.cap, room));
}

/** The width the panel has now (what it measures, chosen or default), in rem. */
export function widthNow(id: PanelId): number {
  const el = document.getElementById(PANELS[id].el);
  return el ? el.getBoundingClientRect().width / rootPx() : panelWidths.value[id] ?? PANELS[id].min;
}

/** The width to give the panel: cut to its own limits and to what the window leaves now; a quarter of a rem at a time. */
const fit = (id: PanelId, rem: number): number => {
  const p = PANELS[id];
  return Math.round(Math.max(p.min, Math.min(maxWidth(id), rem)) * 4) / 4;
};

export function setWidth(id: PanelId, rem: number | null, save = true): void {
  const p = PANELS[id];
  const v = rem === null ? null : fit(id, rem);
  if (v !== panelWidths.value[id]) panelWidths.value = { ...panelWidths.value, [id]: v };
  if (save) persist(p.key, v === null ? '' : String(v));
}

/** Back to the default of the style sheet. */
export function resetWidth(id: PanelId): void {
  setWidth(id, null);
}

/** A step wider or narrower (the arrow keys of the edge), never past the limits. */
export function stepWidth(id: PanelId, grow: boolean, step: number): void {
  setWidth(id, widthNow(id) + (grow ? step : -step));
}

/** Starts a drag of the edge. A panel at a side follows the pointer; the chat, standing in the middle, keeps its middle and
    grows by both of its edges. */
export function dragPanel(id: PanelId, e: PointerEvent): void {
  const grip = e.currentTarget as HTMLElement;
  const el = document.getElementById(PANELS[id].el);
  if (!el) return;
  const p = PANELS[id];
  const box = el.getBoundingClientRect();
  const middle = (box.left + box.right) / 2;
  e.preventDefault();
  grip.setPointerCapture(e.pointerId);
  grip.classList.add('drag');
  const move = (ev: PointerEvent): void => {
    const px = p.dock === 'center' ? 2 * Math.abs(ev.clientX - middle) : p.dock === 'left' ? ev.clientX - box.left : box.right - ev.clientX;
    setWidth(id, px / rootPx(), false);
  };
  const up = (): void => {
    grip.classList.remove('drag');
    grip.removeEventListener('pointermove', move);
    grip.removeEventListener('pointerup', up);
    grip.removeEventListener('pointercancel', up);
    const v = panelWidths.value[id];
    persist(p.key, v === null ? '' : String(v));
  };
  grip.addEventListener('pointermove', move);
  grip.addEventListener('pointerup', up);
  grip.addEventListener('pointercancel', up);
}

/** Puts the widths on #win: each panel's own, cut to what the window leaves now, and the sheet's default where the operator
    has chosen none. Every width is worked out before any is written, so the panels do not read each other's fresh figures. */
export function installPanelWidths(): void {
  const win = document.getElementById('win');
  if (!win) return;
  effect(() => {
    void scaleTick.value;
    // the columns are worked out once, from the choices and the sheet's widths, so the panels never read each other's fresh figures
    const c = columns();
    const told = IDS.map((id) => [PANELS[id].css, shown(id, c)] as const);
    for (const [css, rem] of told) {
      if (rem === null) win.style.removeProperty(css);
      else win.style.setProperty(css, `${rem}rem`);
    }
  });
}
