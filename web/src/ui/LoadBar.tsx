import type { JSX } from 'preact';
import { signal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { live, loadInfo, loadRecv, snap } from '../store';
import { t, tn, locale } from '../i18n';
import { goHome } from '../nav';
import { rootPx } from '../scale';
import { tierFor } from '../util/load';
import { gb, levelOf, meters, pct, staleLoad, worstMeter, type LoadLevel, type Meter } from '../util/load';
import './loadbar.css';

/** The bars and their tooltip are two pieces of the top bar: the tooltip stands outside the bars' box (which clips by its width). */
const shown = signal(false);
/** What time it is for the bars: they go grey when a reading is too old. */
const now = signal(Date.now());

type State = 'wait' | 'off' | 'on' | 'na';

function state(): State {
  const l = loadInfo.value;
  if (!l || l.at === 0) return live.value ? 'wait' : 'off';
  if (l.unsupported) return 'na';
  if (!live.value || !l.ok || staleLoad(l.at, l.sent, now.value - loadRecv.value, l.every_ms)) return 'off';
  return 'on';
}

const hostName = (): string => snap.value?.host ?? '';
const keyName = (k: Meter['key']): string => t(`load.${k}`);

function clock(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
}

function summaryText(ms: Meter[], st: State): string {
  const host = hostName();
  if (st === 'wait') return t('load.waiting', { host });
  if (st === 'off') return t('load.offline', { host });
  return t('load.aria', { host, list: ms.map((m) => `${keyName(m.key)} ${m.percent} %, ${t(`load.level.${m.level}`)}`).join('; ') });
}

/** The bars sit beside the crumbs, which come first: the form of the bars is the one that fits in what the crumbs leave (data-tier). */
function useTier(box: { current: HTMLElement | null }): void {
  useEffect(() => {
    const el = box.current;
    const row = el?.parentElement;
    const crumbs = row?.querySelector('.crumbs') as HTMLElement | null;
    if (!el || !row || !crumbs) return;
    const measure = (): void => {
      // a phone-sized window puts the bars on the second row, beside the search: always the narrowest form
      if (document.documentElement.classList.contains('bp900')) { el.dataset.tier = 'sum'; return; }
      const gap = parseFloat(getComputedStyle(crumbs).columnGap) || 0;
      const kids = [...crumbs.children] as HTMLElement[];
      // what the crumbs would take if nothing were cut: a button's own text, not the width it was squeezed to
      const natural = kids.reduce((a, c) => a + (c.tagName === 'BUTTON' ? c.scrollWidth : c.offsetWidth), 0) + gap * Math.max(0, kids.length - 1);
      const rowGap = parseFloat(getComputedStyle(row).columnGap) || 0;
      el.dataset.tier = tierFor((row.clientWidth - natural - rowGap) / rootPx());
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    ro.observe(document.getElementById('top') ?? row);
    const mo = new MutationObserver(measure);
    mo.observe(crumbs, { childList: true, characterData: true, subtree: true });
    return () => { ro.disconnect(); mo.disconnect(); };
  }, []);
}

export function LoadBar(): JSX.Element | null {
  const box = useRef<HTMLDivElement>(null);
  useTier(box);
  useEffect(() => {
    const id = window.setInterval(() => { now.value = Date.now(); }, 2000);
    return () => clearInterval(id);
  }, []);
  const l = loadInfo.value;
  const st = state();
  // Without a reading the bars are still there, grey, with the same CPU / MEM places: the bar does not jump when the first one comes.
  const ms: Meter[] = l && st === 'on' ? meters(l) : [];
  const shape: Meter['key'][] = l && l.gpus.length ? ['cpu', 'mem', 'gpu'] : ['cpu', 'mem'];
  const worst = ms.length ? worstMeter(ms) : null;
  const level: LoadLevel | 'off' = worst ? worst.level : 'off';
  return (
    <div class="loadbar" id="loadbar" ref={box} data-tier="sum" data-state={st} data-level={level}>
      <button
        type="button" class="ld" id="load-chip" aria-label={summaryText(ms, st)} aria-describedby="load-pop"
        onMouseEnter={() => { shown.value = true; }} onMouseLeave={() => { shown.value = false; }}
        onFocus={() => { shown.value = true; }} onBlur={() => { shown.value = false; }}
        onClick={(e) => { (e.currentTarget as HTMLElement).blur(); shown.value = false; void goHome(); }}
      >
        <span class="ld-host">{hostName()}</span>
        {shape.map((k) => {
          const m = ms.find((x) => x.key === k);
          return (
            <span class="ld-m" data-k={k} data-level={m ? m.level : 'off'} key={k}>
              <span class="ld-k">{keyName(k)}</span>
              <span class="ld-bar"><i style={`--w:${m ? m.percent : 0}%`} /></span>
              <b class="ld-v">{m ? `${m.percent}%` : '—'}</b>
            </span>
          );
        })}
        <span class="ld-sum" data-level={level}>
          <i class="ld-dot" aria-hidden="true" />
          <span class="ld-k">{worst ? keyName(worst.key) : t('load.short')}</span>
          <b class="ld-v">{worst ? `${worst.percent}%` : '—'}</b>
        </span>
      </button>
    </div>
  );
}

function Row(props: { k: string; m: Meter | null; detail: string; sub?: string }): JSX.Element {
  const m = props.m;
  return (
    <div class="ld-r" data-k={props.k} data-level={m ? m.level : 'off'}>
      <span class="ld-rk">{props.k === 'gpu' ? t('load.gpu') : keyName(props.k as Meter['key'])}</span>
      <span class="ld-bar wide"><i style={`--w:${m ? m.percent : 0}%`} /></span>
      <b class="ld-v">{m ? `${m.percent} %` : '—'}</b>
      <span class="ld-rd">{props.detail}{m ? ` · ${t(`load.level.${m.level}`)}` : ''}</span>
      {props.sub && <span class="ld-rs">{props.sub}</span>}
    </div>
  );
}

/** The tooltip: exact numbers (cores, gigabytes, video memory, every card) and when they were read. */
export function LoadPop(): JSX.Element | null {
  const l = loadInfo.value;
  const st = state();
  if (st === 'na') return null;
  const host = hostName();
  const loc = locale();
  const on = l && st === 'on';
  const ms = on ? meters(l) : [];
  const by = (k: Meter['key']): Meter | null => ms.find((x) => x.key === k) ?? null;
  return (
    <div class={`ld-pop panel${shown.value ? ' open' : ''}`} id="load-pop" role="tooltip" data-state={st}>
      <div class="ld-ph"><b>{host}</b><span>{t('load.own')}</span></div>
      {st !== 'on' && (
        <div class="ld-off">{st === 'wait' ? t('load.waiting', { host }) : t('load.offline', { host })}{l && !l.ok && l.error ? <span class="ld-err">{l.error}</span> : null}</div>
      )}
      <Row k="cpu" m={by('cpu')} detail={on ? tn('load.cores', l.cpu.cores) : t('load.noData')} />
      <Row k="mem" m={by('mem')} detail={on ? t('load.memOf', { used: gb(l.mem.used, loc), total: gb(l.mem.total, loc) }) : t('load.noData')} />
      {on && l.gpus.length === 0 && <div class="ld-none" data-k="gpu">{t('load.noGpu')}</div>}
      {on && l.gpus.map((g, i) => {
        const own: Meter = { key: 'gpu', percent: pct(g.util), level: levelOf(pct(g.util)) };
        return <Row key={i} k="gpu" m={own} detail={t('load.vram', { used: gb(g.mem_used, loc), total: gb(g.mem_total, loc) })} sub={g.name} />;
      })}
      {on && l.gpus.length > 1 && <div class="ld-note">{t('load.gpuMean', { n: by('gpu')?.percent ?? 0 })}</div>}
      <div class="ld-ft">
        {l && l.at > 0 ? t('load.updated', { at: clock(l.at), n: Math.max(1, Math.round((l.every_ms || 3000) / 1000)) }) : t('load.noTime')}
        <span class="ld-go">{t('load.click')}</span>
      </div>
    </div>
  );
}
