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

/** The most the panel may take: the window as the interface sees it, less room for the world and the focus card. */
export const navMax = (): number => Math.max(NAV_W_MIN, Math.min(NAV_W_MAX, effectiveWidth() * 0.6));

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
    if (navWidth.value === null) win.style.removeProperty('--nav-user');
    else win.style.setProperty('--nav-user', `${navWidth.value}rem`);
  });
}
