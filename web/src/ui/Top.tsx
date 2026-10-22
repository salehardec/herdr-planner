import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { attention, level, mode, panels, selected, snap, universe, taskOf } from '../store';
import { Search } from './Search';
import { goHome, navigateTo } from '../nav';
import { fromQueue } from '../nav';
import { setMode } from '../keys';
import { t } from '../i18n';
import { LimitsChip } from './Limits';
import { Menu } from './Menu';
import { StationChip } from './StationChip';
import { LoadBar, LoadPop } from './LoadBar';
import './top.css';

function Crumbs(): JSX.Element {
  const sel = selected.value;
  const host = snap.value?.host ?? '';
  const items: { label: string; go: () => void; last?: boolean }[] = [];
  items.push({ label: host, go: () => { void goHome(); } });
  let uni = universe.value;
  if (sel?.kind === 'planet') uni = taskOf(sel.id)?.repo ?? uni;
  if (sel?.kind === 'moon') { const task = sel.id.split('#')[0]; uni = task === 'quasar' ? null : taskOf(task)?.repo ?? uni; }
  if (sel?.kind === 'quasar') items.push({ label: t('quasar.label'), go: () => { void navigateTo(sel); } });
  else if (uni && (level.value === 'nebula' || sel?.kind === 'universe' || sel?.kind === 'planet' || sel?.kind === 'moon')) {
    items.push({ label: uni, go: () => { void navigateTo({ kind: 'universe', id: uni as string }); } });
  }
  if (sel?.kind === 'planet') {
    const tk = taskOf(sel.id);
    items.push({ label: tk?.tag ?? sel.id, go: () => {} });
  }
  if (sel?.kind === 'moon') {
    const [tid, sid] = sel.id.split('#');
    const tk = tid === 'quasar' ? null : taskOf(tid);
    if (tk) items.push({ label: tk.tag, go: () => { void navigateTo({ kind: 'planet', id: tk.id }); } });
    const sa = (tk ? tk.subagents : snap.value?.coordinator.subagents ?? []).find((x) => x.id === sid);
    items.push({ label: sa?.role ?? sid, go: () => {} });
  }
  items[items.length - 1].last = true;
  return (
    <nav id="crumbs" class="crumbs" aria-label={t('crumbs.aria')}>
      {items.map((it, i) => (
        <>
          {i > 0 && <span class="sep">›</span>}
          <button type="button" class={it.last ? 'last' : ''} onClick={it.go} data-crumb={i}>{it.label}</button>
        </>
      ))}
    </nav>
  );
}

/** The search sits in the middle of the top; it is as wide as the free middle allows, so it must know how wide the buttons at the right are. */
function useRightWidth(top: { current: HTMLElement | null }, right: { current: HTMLElement | null }): void {
  useEffect(() => {
    const t = top.current;
    const r = right.current;
    if (!t || !r || typeof ResizeObserver === 'undefined') return;
    const measure = (): void => {
      const kids = [...r.children] as HTMLElement[];
      const w = kids.reduce((a, c) => a + c.offsetWidth, 0) + 8 * Math.max(0, kids.length - 1);
      t.style.setProperty('--right-w', `${Math.ceil(w)}px`);
    };
    measure();
    const ro = new ResizeObserver(measure);
    [...r.children].forEach((c) => ro.observe(c));
    const mo = new MutationObserver(() => { ro.disconnect(); [...r.children].forEach((c) => ro.observe(c)); measure(); });
    mo.observe(r, { childList: true });
    return () => { ro.disconnect(); mo.disconnect(); };
  }, []);
}

export function Top(): JSX.Element {
  const topRef = useRef<HTMLDivElement>(null);
  const rightRef = useRef<HTMLDivElement>(null);
  useRightWidth(topRef, rightRef);
  const waits = attention.value;
  const m = mode.value;
  return (
    <div class="top" id="top" ref={topRef}>
      <div class="topl" id="topl">
        <Crumbs />
        <LoadBar />
      </div>
      <Search />
      <div class="right" id="right" ref={rightRef}>
        <StationChip />
        <button
          class={`btn waitbtn ${waits ? 'hot' : ''}`} id="wait-btn" type="button" title="B" aria-pressed={panels.wait.value}
          onClick={() => { panels.wait.value = !panels.wait.value; if (panels.wait.value) fromQueue.value = false; }}
        >{t('wait.button', { n: waits })}</button>
        <LimitsChip />
        <div id="mode" class="modeswitch" role="group" aria-label={t('mode.aria')}>
          <button type="button" data-mode="auto" class={m === 'auto' ? 'on' : ''} aria-pressed={m === 'auto'} title="P" onClick={() => setMode('auto')}>{t('mode.auto')}</button>
          <button type="button" data-mode="manual" class={m === 'manual' ? 'on' : ''} aria-pressed={m === 'manual'} title="P" onClick={() => setMode('manual', true)}>{t('mode.manual')}</button>
        </div>
        <Menu />
      </div>
      <LoadPop />
    </div>
  );
}
