/* The size of the interface. Every size in the style sheets is in rem, and the root font follows the window
   (see base.css), so a 2K or 4K screen gets a bigger interface by itself. «Масштаб интерфейса» in the ship's
   computer, and Ctrl+= / Ctrl+− / Ctrl+0, replace the automatic size with a fixed percentage of 16 px. */
import { signal } from '@preact/signals';
import { persist } from './store';
import { toast } from './toast';
import { t } from './i18n';

export const SCALE_KEY = 'herdr-planner.uiScale';
/** The percentages the keys and the setting step through (100 % = 16 px). */
export const SCALE_STEPS = [80, 90, 100, 110, 125, 150, 175, 200, 250];

const read = (): 'auto' | number => {
  try {
    const v = Number(localStorage.getItem(SCALE_KEY));
    if (v >= 50 && v <= 300) return v;
  } catch { /* private mode */ }
  return 'auto';
};

/** `'auto'`: by the size of the window; otherwise a percentage. */
export const uiScale = signal<'auto' | number>(read());
/** What the root font is now, in px (follows the window and the setting). */
export const rootPx = (): number => parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
/** How much bigger than the design (16 px) the interface is now. */
let k = 1;
export const scaleK = (): number => k;
/** Percent now, whichever way it came. */
export const scalePercent = (): number => Math.round((rootPx() / 16) * 100);
/** Bumped when the size changes: views that show it re-render. */
export const scaleTick = signal(0);

/* The widths at which the layout changes (they were media queries in px). They are measured in rem, not px:
   the width of the window as the interface sees it is innerWidth / (rootPx / 16), so a raised scale on a small
   window gets the narrow layouts a smaller window would. The classes bp440 … bp1700 sit on <html> (base.css and
   the panels' sheets use them). */
export const BREAKPOINTS = [440, 900, 1000, 1100, 1700];
/** The window's width in design pixels (16 px to the rem). */
export const effectiveWidth = (): number => innerWidth / (rootPx() / 16);

function breakpoints(): void {
  const w = effectiveWidth();
  for (const b of BREAKPOINTS) document.documentElement.classList.toggle(`bp${b}`, w <= b);
}

function apply(): void {
  const root = document.documentElement;
  if (uiScale.value === 'auto') root.style.removeProperty('--ui-root');
  else root.style.setProperty('--ui-root', `${(16 * uiScale.value) / 100}px`);
  k = rootPx() / 16;
  breakpoints();
  scaleTick.value++;
}

export function setScale(v: 'auto' | number): void {
  uiScale.value = v;
  persist(SCALE_KEY, v === 'auto' ? 'auto' : String(v));
  apply();
}

/** One step bigger (+1) or smaller (−1) than the size now. */
export function stepScale(dir: 1 | -1): void {
  const now = scalePercent();
  const next = dir > 0 ? SCALE_STEPS.find((s) => s > now + 1) : [...SCALE_STEPS].reverse().find((s) => s < now - 1);
  if (next === undefined) { toast(t('scale.limit', { n: now })); return; }
  setScale(next);
  toast(t('scale.now', { n: next }));
}

export function resetScale(): void {
  setScale('auto');
  toast(t('scale.auto', { n: scalePercent() }));
}

export function installScale(): void {
  apply();
  addEventListener('resize', apply);
}
