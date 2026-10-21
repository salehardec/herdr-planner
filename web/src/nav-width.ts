/* The width of the navigator. The default follows the window (base.css); the operator drags the panel's right edge (or uses the
   arrow keys on it) and the width is remembered in this browser, in rem so that it keeps its proportion at any scale.
   A double click on the edge goes back to the default. The page reads it as --nav-user on #win. */
import { effect, signal } from '@preact/signals';
import { persist } from './store';
import { effectiveWidth, rootPx, scaleTick } from './scale';

export const NAV_W_KEY = 'herdr-planner.navW';
/** The narrowest and the widest the panel can be, in rem. */
export const NAV_W_MIN = 16;
export const NAV_W_MAX = 48;

const read = (): number | null => {
  try {
    const v = Number(localStorage.getItem(NAV_W_KEY));
    if (v >= NAV_W_MIN && v <= NAV_W_MAX) return v;
  } catch { /* private mode */ }
  return null;
};

/** The width the operator chose, in rem; null: the default. */
export const navWidth = signal<number | null>(read());

/* The chat needs this much between the navigator and the focus card (the same figure the wide chat uses), and the focus card and the gaps
   are what base.css gives them: the navigator may take the rest, never less than what the narrow windows get by default. */
const CHAT_MIN_REM = 30;
const NAV_FLOOR = 21;
const aside = (): number => (effectiveWidth() <= 1100 ? 20.625 : 24.375) + 3;

/** The most the panel may take, in rem, at the size of the window now (the interface's own size included).
    effectiveWidth() is in design pixels (16 to the rem): it is divided to rem here, the other figures are rem. */
export const navMax = (): number => Math.max(NAV_FLOOR, Math.min(NAV_W_MAX, effectiveWidth() / 16 - aside() - CHAT_MIN_REM));

export function setNavWidth(rem: number | null, save = true): void {
  const v = rem === null ? null : Math.round(Math.max(NAV_W_MIN, Math.min(navMax(), rem)) * 4) / 4;
  navWidth.value = v;
  if (save) persist(NAV_W_KEY, v === null ? '' : String(v));
}

/** The width now, in rem (what the panel measures, chosen or default). */
export function navWidthNow(): number {
  const el = document.getElementById('nav');
  return el ? el.getBoundingClientRect().width / rootPx() : navWidth.value ?? 0;
}

/** Starts a drag of the edge. `x0` is the left edge of the panel in px. */
export function dragNav(e: PointerEvent): void {
  const grip = e.currentTarget as HTMLElement;
  const left = document.getElementById('nav')?.getBoundingClientRect().left ?? 0;
  e.preventDefault();
  grip.setPointerCapture(e.pointerId);
  grip.classList.add('drag');
  const move = (ev: PointerEvent): void => setNavWidth((ev.clientX - left) / rootPx(), false);
  const up = (): void => {
    grip.classList.remove('drag');
    grip.removeEventListener('pointermove', move);
    grip.removeEventListener('pointerup', up);
    grip.removeEventListener('pointercancel', up);
    if (navWidth.value !== null) persist(NAV_W_KEY, String(navWidth.value));
  };
  grip.addEventListener('pointermove', move);
  grip.addEventListener('pointerup', up);
  grip.addEventListener('pointercancel', up);
}

export function installNavWidth(): void {
  const win = document.getElementById('win');
  if (!win) return;
  effect(() => {
    void scaleTick.value;
    // a width kept from a bigger window or a smaller scale does not squeeze the chat here: it is cut to what fits now (it stays as saved)
    if (navWidth.value === null) win.style.removeProperty('--nav-user');
    else win.style.setProperty('--nav-user', `${Math.min(navWidth.value, navMax())}rem`);
  });
}
