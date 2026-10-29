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

/** Set while the focus is handed back to an element on purpose: that focus must not open its card again. */
let muted = false;

/** Closes the card; `back` hands the focus back to the element it belongs to when it was inside the card. */
export function closeCard(back = false): void {
  clear();
  const c = card.value;
  if (!c) return;
  const inCard = !!document.getElementById('top-card')?.contains(document.activeElement);
  card.value = null;
  if (back && inCard) {
    muted = true;
    anchorOf(c.id)?.focus();
    muted = false;
  }
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
  // the focus is on the card's link (the keyboard): the card stays
  if (document.getElementById('top-card')?.contains(document.activeElement)) return;
  closeTimer = window.setTimeout(() => { if (card.value && !card.value.pinned) card.value = null; }, CLOSE_MS);
}

/** A tap or the keyboard's Enter opens the card and keeps it; the next one closes it. A mouse click only keeps what hover has opened. */
function pick(id: CardId, e: MouseEvent): void {
  clear();
  if ((e as PointerEvent).pointerType === 'mouse') { if (card.value?.id !== id) card.value = { id, pinned: false }; return; }
  card.value = card.value?.id === id && card.value.pinned ? null : { id, pinned: true };
}

/** What an element of the top bar needs to own a card. */
export const cardAnchor = (id: CardId): Pick<JSX.HTMLAttributes<HTMLButtonElement>, 'aria-haspopup' | 'aria-expanded' | 'aria-controls' | 'onMouseEnter' | 'onMouseLeave' | 'onFocus' | 'onClick' | 'onKeyDown'> => ({
  'aria-haspopup': 'dialog',
  'aria-expanded': card.value?.id === id,
  'aria-controls': 'top-card',
  onMouseEnter: () => show(id, OPEN_MS),
  onMouseLeave: later,
  onFocus: () => { if (!muted) show(id, 0); },
  onClick: (e: MouseEvent) => pick(id, e),
  onKeyDown: (e: KeyboardEvent) => {
    // Tab from the element goes into its open card (its link), which the DOM order would skip
    if (e.key !== 'Tab' || e.shiftKey || card.value?.id !== id) return;
    const link = document.querySelector<HTMLElement>('#top-card button, #top-card a');
    if (!link) return;
    e.preventDefault();
    link.focus();
  },
});

/** Tab and Shift+Tab move through the card's own controls; past its last (or before its first) they leave the card: back to its
 *  element, with the card closed (the next Tab goes on from there). A card with a single link (the machine's) leaves at once. */
export const cardKeys = {
  onKeyDown: (e: KeyboardEvent) => {
    if (e.key !== 'Tab') return;
    const all = [...document.querySelectorAll<HTMLElement>('#top-card button:not([disabled]), #top-card a[href]')].filter((el) => el.tabIndex >= 0);
    const at = all.indexOf(document.activeElement as HTMLElement);
    const leaving = all.length === 0 || (e.shiftKey ? at <= 0 : at === all.length - 1);
    if (!leaving) return;
    e.preventDefault();
    closeCard(true);
  },
};

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
