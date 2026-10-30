import type { JSX } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';
import { card, cardSelf, cardKeys, anchorOf, closeCard } from './hovercard';
import { LoadCard } from './LoadBar';
import { LimitsCard } from './Limits';
import { lang, t } from '../i18n';
import { loadInfo, snap, panels } from '../store';
import './hovercard.css';

/** Aligns the card to its button, inside the viewport; docked chat can open it above when there is more room. */
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
  const below = vh - a.bottom - gap - pad;
  const above = a.top - gap - pad;
  const height = Math.min(el.scrollHeight, 320);
  const over = below < height && above > below;
  const available = over ? above : below;
  el.style.maxHeight = `${Math.max(0, Math.floor(available))}px`;
  const top = over ? a.top - gap - el.offsetHeight : a.bottom + gap;
  el.style.left = `${Math.round(left)}px`;
  el.style.top = `${Math.round(top)}px`;
}

export function HoverCard(): JSX.Element | null {
  const c = card.value;
  const ref = useRef<HTMLDivElement>(null);
  // what the card shows changes with the readings, the language and the window: it is placed again after each
  void lang.value; void loadInfo.value; void snap.value; void panels.chat.value;
  useLayoutEffect(() => {
    const el = ref.current;
    const anchor = c ? anchorOf(c.id) : null;
    if (!el || !anchor) return;
    if (anchor.offsetWidth === 0 || anchor.closest('[aria-hidden="true"]')) { closeCard(); return; }
    const chat = anchor.closest<HTMLElement>('.chat');
    let frame = 0;
    const again = (): void => {
      cancelAnimationFrame(frame);
      place(el, anchor);
      // The docked chat slides into view. A card opened during that slide follows its button.
      if (chat?.getAnimations().some((a) => a.playState === 'running')) frame = requestAnimationFrame(again);
    };
    again();
    const ro = new ResizeObserver(again);
    ro.observe(anchor);
    if (chat) ro.observe(chat);
    window.addEventListener('resize', again);
    chat?.addEventListener('transitionrun', again);
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      window.removeEventListener('resize', again);
      chat?.removeEventListener('transitionrun', again);
    };
  });
  if (!c) return null;
  return (
    <div class="panel topcard" id="top-card" ref={ref} role="dialog" aria-label={c.id === 'load' ? t('load.short') : t('limits.title')} data-card={c.id} data-pinned={c.pinned ? 'true' : undefined} {...cardSelf} {...cardKeys}>
      {c.id === 'load' ? <LoadCard /> : <LimitsCard />}
    </div>
  );
}
