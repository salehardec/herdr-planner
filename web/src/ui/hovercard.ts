/* One details card under an element of the top bar (the bars, the subscription chip): it opens on hover (with a short delay), on keyboard
   focus and on a tap or a click that is not a mouse's, and closes on Esc, on a click or focus elsewhere, and a moment after the pointer
   leaves both the element and the card (the pause lets the pointer cross the gap between them without the card flickering). */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';

export type CardId = 'load' | 'limits';

/** The card that is open; `pinned` is a card opened by a tap or by the keyboard: it stays until it is closed on purpose. */
export const card = signal<{ id: CardId; pinned: boolean } | null>(null);

const OPEN_MS = 90;
const CLOSE_MS = 240;
let openTimer = 0;
let closeTimer = 0;

const clear = (): void => { window.clearTimeout(openTimer); window.clearTimeout(closeTimer); };

export function closeCard(): void {
  clear();
  if (card.value) card.value = null;
}

function show(id: CardId, delay: number): void {
  window.clearTimeout(closeTimer);
  if (card.value?.id === id) return;
  window.clearTimeout(openTimer);
  const go = (): void => { card.value = { id, pinned: false }; };
  if (delay > 0) openTimer = window.setTimeout(go, delay); else go();
}

/** The pointer left the element or the card: the card goes after a pause, unless something is bringing it back (or it was pinned). */
function later(): void {
  window.clearTimeout(openTimer);
  window.clearTimeout(closeTimer);
  if (!card.value || card.value.pinned) return;
  closeTimer = window.setTimeout(() => { if (card.value && !card.value.pinned) card.value = null; }, CLOSE_MS);
}

/** A tap or the keyboard's Enter opens the card and keeps it; the next one closes it. A mouse click only keeps what hover has opened. */
function pick(id: CardId, e: MouseEvent): void {
  clear();
  if ((e as PointerEvent).pointerType === 'mouse') { if (card.value?.id !== id) card.value = { id, pinned: false }; return; }
  card.value = card.value?.id === id && card.value.pinned ? null : { id, pinned: true };
}

/** What an element of the top bar needs to own a card. */
export const cardAnchor = (id: CardId): Pick<JSX.HTMLAttributes<HTMLButtonElement>, 'aria-haspopup' | 'aria-expanded' | 'aria-controls' | 'onMouseEnter' | 'onMouseLeave' | 'onFocus' | 'onClick'> => ({
  'aria-haspopup': 'dialog',
  'aria-expanded': card.value?.id === id,
  'aria-controls': 'top-card',
  onMouseEnter: () => show(id, OPEN_MS),
  onMouseLeave: later,
  onFocus: () => show(id, 0),
  onClick: (e: MouseEvent) => pick(id, e),
});

/** The card's own pointer events: being over it keeps it. */
export const cardSelf = {
  onMouseEnter: () => window.clearTimeout(closeTimer),
  onMouseLeave: later,
};

const ANCHORS: Record<CardId, string> = { load: 'load-chip', limits: 'hud-limits' };
export const anchorOf = (id: CardId): HTMLElement | null => document.getElementById(ANCHORS[id]);

/** A press outside and focus moving elsewhere (the search, a panel) close the card (Esc does, in keys.ts, before it closes anything else). */
export function installCard(): void {
  const inside = (n: EventTarget | null): boolean => {
    const el = n as Node | null;
    const c = card.value;
    if (!c || !el) return false;
    return !!document.getElementById('top-card')?.contains(el) || !!anchorOf(c.id)?.contains(el);
  };
  document.addEventListener('pointerdown', (e) => { if (card.value && !inside(e.target)) closeCard(); }, true);
  document.addEventListener('focusin', (e) => {
    const c = card.value;
    if (!c || inside(e.target)) return;
    // another element of the bar takes over by its own focus handler
    if (!(e.target as HTMLElement | null)?.closest?.('#load-chip,#hud-limits')) closeCard();
  });
}
