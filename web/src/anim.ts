/* Motion with GSAP, switched off in simple mode and for «reduce motion» (duration 0). */
import { gsap } from 'gsap';
import { simple } from './store';

const d = (s: number): number => (simple.value ? 0 : s);

/** The todo drawer: the cards drop in one after another from the top with a little overshoot. */
export function drawerItems(root: Element | null): void {
  if (!root) return;
  const items = root.querySelectorAll('.titem');
  gsap.killTweensOf(items);
  gsap.fromTo(items, { y: -18, opacity: 0, scale: 0.97 }, { y: 0, opacity: 1, scale: 1, duration: d(0.38), ease: 'back.out(1.5)', stagger: d(0.05), clearProps: 'transform,opacity' });
}

/** Rows of a result list appear softly. */
export function rows(root: Element | null, selector: string): void {
  if (!root) return;
  const els = root.querySelectorAll(selector);
  gsap.fromTo(els, { y: 8, opacity: 0 }, { y: 0, opacity: 1, duration: d(0.2), ease: 'power2.out', stagger: d(0.012), clearProps: 'transform,opacity' });
}

/** A number that counts up to its value (the kanban). */
export function count(el: HTMLElement | null, to: number): void {
  if (!el) return;
  const from = Number(el.dataset.v ?? to);
  el.dataset.v = String(to);
  if (from === to || simple.value) { el.textContent = String(to); return; }
  const o = { v: from };
  gsap.to(o, { v: to, duration: 0.5, ease: 'power1.out', onUpdate: () => { el.textContent = String(Math.round(o.v)); } });
}

/** The warp vignette: fades with the flight; the world drives it, GSAP only closes it softly. */
export function fade(el: HTMLElement | null, to: number, seconds = 0.35): void {
  if (!el) return;
  gsap.to(el, { opacity: to, duration: d(seconds), ease: 'power1.inOut', overwrite: true });
}
