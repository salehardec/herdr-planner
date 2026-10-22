import type { JSX } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';
import { card, cardSelf, anchorOf, closeCard } from './hovercard';
import { LoadCard } from './LoadBar';
import { LimitsCard } from './Limits';
import { lang, t } from '../i18n';
import { loadInfo, snap } from '../store';
import './hovercard.css';

/** Keeps the card under its element, aligned to its left edge (to its right edge when it would not fit), inside the window. */
function place(el: HTMLElement, anchor: HTMLElement): void {
  const gap = 6;
  const pad = 8;
  const a = anchor.getBoundingClientRect();
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const w = el.offsetWidth;
  let left = a.left;
  if (left + w > vw - pad) left = a.right - w;
  left = Math.max(pad, Math.min(left, vw - w - pad));
  const top = a.bottom + gap;
  el.style.left = `${Math.round(left)}px`;
  el.style.top = `${Math.round(top)}px`;
  el.style.maxHeight = `${Math.max(120, Math.floor(vh - top - pad))}px`;
}

export function HoverCard(): JSX.Element | null {
  const c = card.value;
  const ref = useRef<HTMLDivElement>(null);
  // what the card shows changes with the readings, the language and the window: it is placed again after each
  void lang.value; void loadInfo.value; void snap.value;
  useLayoutEffect(() => {
    const el = ref.current;
    const anchor = c ? anchorOf(c.id) : null;
    if (!el || !anchor) return;
    if (anchor.offsetWidth === 0) { closeCard(); return; }
    const again = (): void => place(el, anchor);
    again();
    window.addEventListener('resize', again);
    return () => window.removeEventListener('resize', again);
  });
  if (!c) return null;
  return (
    <div class="panel topcard" id="top-card" ref={ref} role="dialog" aria-label={c.id === 'load' ? t('load.short') : t('limits.title')} data-card={c.id} data-pinned={c.pinned ? 'true' : undefined} {...cardSelf}>
      {c.id === 'load' ? <LoadCard /> : <LimitsCard />}
    </div>
  );
}
