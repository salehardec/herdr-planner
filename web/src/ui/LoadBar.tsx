import type { JSX } from 'preact';
import { signal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { live, loadInfo, loadRecv, panels, snap } from '../store';
import { t, tn, locale } from '../i18n';
import { goHome } from '../nav';
import { cardAnchor, closeCard } from './hovercard';
import { rootPx } from '../scale';
import { gb, levelOf, meters, pct, rowFit, staleLoad, worstMeter, type LoadLevel, type Meter, type RowStep } from '../util/load';
import './loadbar.css';

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

/** The least the search is squeezed to (rem) when the subscription row needs its room. */
const SEARCH_LEAST_REM = 12;

/** The monitoring group (the bars, then the subscription chip) sits right of the menu and the crumbs, which come first: the form of the bars
 *  (data-tier) and of the chip (data-form) is the one that fits in what they leave. */
function useTier(box: { current: HTMLElement | null }): void {
  useEffect(() => {
    const el = box.current;
    const mon = el?.parentElement;
    const row = mon?.parentElement;
    const crumbs = row?.querySelector('.crumbs') as HTMLElement | null;
    const menu = row?.querySelector('.menu') as HTMLElement | null;
    const chip = mon?.querySelector('.limchip') as HTMLElement | null;
    if (!el || !mon || !row || !crumbs || !menu) return;
    let mo: MutationObserver | null = null;
    const measure = (): void => {
      // the gate's chip comes after the subscription chip, once the server has said this is the owner's machine
      const gate = mon.querySelector('.gatechip') as HTMLElement | null;
      if (gate) mo?.observe(gate, { childList: true, characterData: true, subtree: true });
      // a phone-sized window puts the group on the second row, beside the search: always the narrowest forms
      const top = row.parentElement as HTMLElement | null;
      const search = top?.querySelector('.top-search') as HTMLElement | null;
      // the search gives room back to the row only while the row needs it: measured at its natural width each time
      top?.style.removeProperty('--search-w');
      if (document.documentElement.classList.contains('bp900')) {
        el.dataset.tier = 'sum';
        if (chip) chip.dataset.form = 'worst';
        if (gate) gate.dataset.form = 'icon';
        return;
      }
      const px = rootPx();
      const gap = parseFloat(getComputedStyle(crumbs).columnGap) || 0;
      const kids = [...crumbs.children] as HTMLElement[];
      // what the crumbs would take if nothing were cut: a button's own text, not the width it was squeezed to
      const natural = kids.reduce((a, c) => a + (c.tagName === 'BUTTON' ? c.scrollWidth : c.offsetWidth), 0) + gap * Math.max(0, kids.length - 1);
      const rowGap = parseFloat(getComputedStyle(row).columnGap) || 0;
      const monGap = parseFloat(getComputedStyle(mon).columnGap) || 0;
      // every form the row can take, measured as it is drawn (the widest first): all the words, the tightest window, the first k providers with
      // their words, the shares, the one closest to its limit
      const segs = chip ? [...chip.querySelectorAll<HTMLElement>('.lp')] : [];
      const widthOf = (form: string, words: number): number => {
        if (!chip) return 0;
        chip.dataset.form = form;
        segs.forEach((sg, i) => { sg.dataset.w = i < words ? '1' : '0'; });
        return chip.offsetWidth / px;
      };
      const n = segs.length;
      const steps: RowStep[] = [{ form: 'full', words: n, w: widthOf('full', n) }, { form: 'one', words: n, w: widthOf('one', n) }];
      for (let k = n - 1; k >= 1; k--) steps.push({ form: 'mix', words: k, w: widthOf('mix', k) });
      steps.push({ form: 'pct', words: 0, w: widthOf('pct', 0) });
      const worstW = widthOf('worst', 0);
      let gateFull = 0;
      let gateIcon = 0;
      if (gate) {
        gate.dataset.form = 'full'; gateFull = gate.offsetWidth;
        gate.dataset.form = 'icon'; gateIcon = gate.offsetWidth;
      }
      const room = (row.clientWidth - menu.offsetWidth - natural - 2 * rowGap) / px;
      // the search may shrink toward its least width, and the row and the bars have the room that frees (half on each side of the middle)
      const searchWas = search ? search.offsetWidth : 0;
      const maxExtra = search ? Math.max(0, (searchWas - SEARCH_LEAST_REM * px) / 2) / px : 0;
      // the gate keeps its words while the subscription row keeps at least as much of its own; else it is only an icon and a dot
      const decide = (extra: number): { fit: ReturnType<typeof rowFit>; gateForm: 'full' | 'icon' } => {
        let fit = rowFit(room + extra - (gate ? (gateFull + monGap) / px : 0), steps, worstW, monGap / px);
        let gateForm: 'full' | 'icon' = 'full';
        if (gate) {
          const small = rowFit(room + extra - (gateIcon + monGap) / px, steps, worstW, monGap / px);
          if (small.idx < fit.idx || fit.idx >= steps.length) { gateForm = 'icon'; fit = small; }
        }
        return { fit, gateForm };
      };
      let chosen = decide(0);
      let extraUsed = 0;
      for (const share of [0.25, 0.5, 0.75, 1]) {
        const d = decide(maxExtra * share);
        if (d.fit.idx < chosen.fit.idx) { chosen = d; extraUsed = maxExtra * share; }
      }
      const { fit, gateForm } = chosen;
      if (search && extraUsed > 0) top?.style.setProperty('--search-w', `${Math.round(searchWas - 2 * extraUsed * px)}px`);
      el.dataset.tier = fit.tier;
      if (chip) {
        widthOf(fit.chip, fit.words);
        // what the choice was made of (for the checks and for whoever wonders why the row has the form it has)
        chip.dataset.fit = JSON.stringify({ room: +room.toFixed(1), extra: +(extraUsed).toFixed(1), gate: gateForm, steps: steps.map((st) => `${st.form}${st.words}:${st.w.toFixed(1)}`), worst: +worstW.toFixed(1) });
      }
      if (gate) gate.dataset.form = gateForm;
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    ro.observe(document.getElementById('top') ?? row);
    mo = new MutationObserver(measure);
    mo.observe(mon, { childList: true });
    mo.observe(crumbs, { childList: true, characterData: true, subtree: true });
    if (chip) mo.observe(chip, { childList: true, characterData: true, subtree: true });
    return () => { ro.disconnect(); mo?.disconnect(); };
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
      <button type="button" class="ld" id="load-chip" aria-label={summaryText(ms, st)} {...cardAnchor('load')}>
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

/** The card under the bars: exact numbers (cores, gigabytes, video memory of every card, the disk when it is reported), when they were read,
 *  and the way to the machine's Focus. */
export function LoadCard(): JSX.Element | null {
  const l = loadInfo.value;
  const st = state();
  if (st === 'na') return null;
  const host = hostName();
  const loc = locale();
  const on = l && st === 'on';
  const ms = on ? meters(l) : [];
  const by = (k: Meter['key']): Meter | null => ms.find((x) => x.key === k) ?? null;
  const disk = on && l.disk && l.disk.total > 0 ? l.disk : null;
  const diskMeter: Meter | null = disk ? { key: 'cpu', percent: pct(disk.percent), level: levelOf(pct(disk.percent)) } : null;
  return (
    <div class="ld-pop" id="load-pop" data-state={st}>
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
      {disk && <Row k="disk" m={diskMeter} detail={t('load.memOf', { used: gb(disk.used, loc), total: gb(disk.total, loc) })} />}
      <div class="ld-ft">
        <span>{l && l.at > 0 ? t('load.updated', { at: clock(l.at), n: Math.max(1, Math.round((l.every_ms || 3000) / 1000)) }) : t('load.noTime')}</span>
        <button type="button" class="ld-go" id="load-more" onClick={() => { closeCard(); panels.focus.value = true; void goHome(); }}>{t('load.more')}</button>
      </div>
    </div>
  );
}
