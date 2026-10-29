import type { JSX } from 'preact';
import { signal } from '@preact/signals';
import { panels, snap } from '../store';
import { t, locale } from '../i18n';
import { post, refreshNow } from '../api';
import { dayTimeOf } from '../util/dates';
import { money } from '../util/format';
import { ago, barAgents, countdown, expectedLeft, hasNumbers, leftPct, level, paceLine, PROVIDER_NAMES, PROVIDER_ORDER, tabAgents, tightest, usedPct } from '../util/limits';
import type { AgentUsage, LimitWindow, UsageSection } from '../types';
import { Gl } from './Gl';
import { cardAnchor, closeCard } from './hovercard';
import './limits.css';

const agents = (): AgentUsage[] => snap.value?.limits?.agents ?? [];

/** The provider whose tab is open in the card; it also leads the bar, as the one picked in herdr-gpui's panel does. */
export const picked = signal('');
const refreshing = signal(false);

function untilText(ms: number): string {
  const left = Number(ms) - Date.now();
  if (left < 3600000) return t('limits.inMin', { n: Math.max(1, Math.round(left / 60000)) });
  if (left < 86400000) return t('limits.inHours', { n: Math.round(left / 3600000) });
  return t('limits.inDays', { n: Math.round(left / 86400000) });
}

/** `47m`, `2h 33m`, `4d 11h`. */
export function countdownText(ms: number): string {
  const c = countdown(ms);
  const u = (k: string): string => t(`limits.u.${k}`);
  if (c.d > 0) return `${c.d}${u('d')} ${c.h}${u('h')}`;
  if (c.h > 0) return `${c.h}${u('h')} ${c.m}${u('m')}`;
  return `${c.m}${u('m')}`;
}

/** The title of a window: the kind in words, or the service's own name for it. */
export function winTitle(w: LimitWindow): string {
  return w.kind && w.kind !== 'named' ? t(`limits.kind.${w.kind}`) : w.label;
}

/** A line the provider says in English (a section's title, a fact), in the page's language when the dictionary knows it. */
const said = (text: string): string => {
  const key = `limits.s.${text}`;
  const out = t(key);
  return out === key ? text : out;
};

/** Why the planner has no (or no fresh) reading, in words: the code of the server's `why`. */
export function whyText(code: string): string {
  if (code.startsWith('status:')) return t('limits.why.status', { n: code.slice(7) });
  const key = `limits.why.${code}`;
  const out = t(key);
  return out === key ? code : out;
}

const cur = (n: number, c = 'USD'): string => (c === 'USD' ? `$${Number(n).toFixed(2)}` : `${Number(n).toFixed(2)} ${c}`);

/** `35% used 31m`: the time to the reset names a session or weekly window; a window the service names itself is named. */
function usedText(w: LimitWindow): string {
  let at: string;
  if (w.kind === 'named' || !w.kind) at = w.label;
  else if (w.resets_at_ms) at = w.resets_at_ms <= Date.now() ? t('limits.resetsNow') : countdownText(w.resets_at_ms - Date.now());
  else at = t(`limits.chip${w.kind[0].toUpperCase()}${w.kind.slice(1)}`);
  return t('limits.chipUsed', { n: usedPct(w), at });
}

export function worstLimit(): { agent: AgentUsage; window: LimitWindow } | null {
  const now = Date.now();
  let worst: { agent: AgentUsage; window: LimitWindow } | null = null;
  for (const agent of agents()) {
    for (const w of agent.limits ?? []) {
      // A window whose reset has passed describes an allowance that is no longer being spent.
      if (w.resets_at_ms && w.resets_at_ms <= now) continue;
      if (!worst || w.percent > worst.window.percent) worst = { agent, window: w };
    }
  }
  return worst;
}

export function ProviderIcon(props: { id: string }): JSX.Element {
  return <Gl n={PROVIDER_ORDER.includes(props.id) ? `p-${props.id}` : 'limit'} cls="lp-ico" />;
}

/** What a provider says in the bar, in two lengths: the two tightest windows, then the tightest alone (the third form is its percentage). */
function Segment(props: { a: AgentUsage }): JSX.Element {
  const a = props.a;
  const ws = tightest(a.limits ?? []);
  const one = tightest(a.limits ?? [], 1)[0];
  const lv = level(a.limits ?? []);
  const words = ws.length ? ws.map(usedText).join(' · ') : a.balance ? cur(a.balance.remaining, a.balance.currency || 'USD') : '';
  return (
    <span class={`lp lp-${lv}${a.stale ? ' stale' : ''}`} data-p={a.id} title={`${a.name}: ${words}`}>
      <ProviderIcon id={a.id} />
      {one && <span class="lp-bar"><i style={`--w:${usedPct(one)}%`} /></span>}
      {ws.length ? (
        <>
          <span class="lp-t lp-t1">{usedText(ws[0])}</span>
          {ws[1] && <span class="lp-t lp-t2"> · {usedText(ws[1])}</span>}
          <span class="lp-s">{usedPct(one)}%</span>
        </>
      ) : (
        <span class="lp-t lp-s lp-bal">{words}</span>
      )}
    </span>
  );
}

/** The bar: each provider with its icon, a small gauge and what is used and when it resets, as in herdr-gpui's status bar
 *  (data-form: `full` the two tightest windows, `one` the tightest, `pct` only its share; LoadBar.tsx measures them). */
export function LimitsChip(): JSX.Element {
  const shown = barAgents(agents(), picked.value);
  if (!shown.length) {
    return (
      <button class="btn limchip" id="hud-limits" type="button" aria-label={t('limits.noneTip')} {...cardAnchor('limits')}>
        <Gl n="limit" /><span class="lim-ag">{t('limits.chip')} <span class="faint">{t('limits.noData')}</span></span><b class="lim-pc nd">—</b>
      </button>
    );
  }
  const all = shown.flatMap((a) => a.limits ?? []);
  const lv = level(all);
  const tip = shown
    .map((a) => `${a.name}: ${tightest(a.limits ?? []).map(usedText).join(' · ') || (a.balance ? cur(a.balance.remaining, a.balance.currency || 'USD') : '')}${a.stale ? ` (${t('limits.stale')})` : ''}`)
    .join('; ');
  return (
    <button class={`btn limchip lim-row${lv === 'crit' ? ' lim-crit' : lv === 'warn' ? ' lim-warn' : ''}`} id="hud-limits" type="button" aria-label={tip} {...cardAnchor('limits')}>
      {shown.map((a) => <Segment key={a.id} a={a} />)}
    </button>
  );
}

/* ───────── the panel and the light page: every window of every subscription ───────── */

function Window(props: { w: LimitWindow }): JSX.Element {
  const w = props.w;
  const used = usedPct(w);
  const remaining = 100 - used;
  const sev = w.severity === 'crit' ? 'crit' : w.severity === 'warn' ? 'warn' : 'ok';
  const reset = !w.resets_at_ms ? t('limits.noReset') : w.resets_at_ms <= Date.now() ? t('limits.resetPassed') : t('limits.reset', { at: dayTimeOf(w.resets_at_ms), left: untilText(w.resets_at_ms) });
  const pace = paceText(w);
  return (
    <div class={`lim-win ${sev}`}>
      <div class="lim-t"><span class="lim-label">{winTitle(w)}</span><b>{t('limits.left', { n: remaining })}</b></div>
      <div class="lim-bar" role="progressbar" aria-label={winTitle(w)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={used} title={t('limits.used', { n: used })}><i style={`--w:${used}%`} /></div>
      <div class="lim-meta"><span>{reset}</span>{pace && <span class="lim-pace"> · {pace}</span>}</div>
    </div>
  );
}

/** `62% in reserve · Lasts until reset`: whether the allowance lasts to the reset at the pace so far. Empty when that cannot be judged. */
export function paceText(w: LimitWindow): string {
  const p = paceLine(w);
  if (!p) return '';
  if (p.standing === 'ran_out') return t('limits.ranOut');
  const standing = p.standing === 'reserve' ? t('limits.reserve', { n: p.n }) : p.standing === 'deficit' ? t('limits.deficit', { n: p.n }) : t('limits.onPace');
  const outlook = p.runsOutMs === null ? t('limits.lasts') : t('limits.runsOut', { at: countdownText(p.runsOutMs) });
  return `${standing} · ${outlook}`;
}

export function LimitAgent(props: { a: AgentUsage }): JSX.Element {
  const a = props.a;
  const windows = a.limits ?? [];
  const lv = level(windows);
  const stale = a.stale === true;
  const staleMin = Math.max(1, Math.round((snap.value?.limits?.stale_after_secs ?? 1800) / 60));
  return (
    <div class={`lim-agent ${lv}${stale ? ' stale' : ''}`} data-agent={a.id}>
      <div class="lim-head"><b>{a.name}</b>{a.tier && <span class="lim-tier">{a.tier}</span>}{a.account && <span class="lim-acct">{a.account}</span>}<span class="grow" />{stale && <span class="lim-stale" title={t('limits.staleTip', { n: staleMin })}>{t('limits.staleShort')}</span>}</div>
      {a.status && (windows.length || a.balance) ? <div class="lim-status">{a.status}</div> : null}
      {windows.map((w, i) => <Window key={i} w={w} />)}
      {(a.extra ?? []).map((w, i) => <Window key={`x${i}`} w={w} />)}
      {a.balance && (
        <div class="lim-bal">
          <span>{t('limits.balance')}</span><b>{money(a.balance.remaining)} {a.balance.currency || 'USD'}</b>
          {a.balance.estimated && <span class="lim-est">{t('limits.estimate')}</span>}
          {Number(a.balance.funded) > 0 && <span>{t('limits.of', { n: money(a.balance.funded as number), c: a.balance.currency || 'USD' })}</span>}
        </div>
      )}
      {!windows.length && !a.balance && <div class="lim-none-body">{a.note || t('limits.noData')}</div>}
      {a.why && (windows.length > 0 || !!a.balance) && <div class="lim-why">{t('limits.whyStale', { why: whyText(a.why) })}</div>}
      <div class="lim-foot">{a.updated_at_ms ? t('limits.updated', { at: dayTimeOf(a.updated_at_ms) }) : t('limits.updatedUnknown')}{a.source === 'omarchy' ? ` · ${t('limits.sourceOmarchy')}` : ''}</div>
    </div>
  );
}

function Unconnected(): JSX.Element | null {
  const ids = snap.value?.limits?.unconnected ?? [];
  if (!ids.length) return null;
  const name = (id: string): string => PROVIDER_NAMES[id] ?? id;
  return (
    <div class="lim-unc" id="limits-unconnected" title={ids.map((id) => `${name(id)}: ${t(`limits.hint.${id}`)}`).join('\n')}>
      {t('limits.unconnected', { list: ids.map(name).join(', ') })}
    </div>
  );
}

export function LimitsPop(): JSX.Element | null {
  if (!panels.limits.value) return null;
  const list = agents();
  return (
    <div class="panel pop limits" role="dialog" aria-label={t('limits.title')}>
      <div class="lim-pop-head"><b>{t('limits.title')}</b><span>{t('limits.sub')}</span><span class="grow" /><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.limits.value = false; }}><Gl n="close" /></button></div>
      <div class="lim-list">{list.length ? list.map((a) => <LimitAgent key={a.id} a={a} />) : <div class="none">{t('limits.omarchyNone')}</div>}</div>
      <Unconnected />
    </div>
  );
}

/* ───────── the card under the bar: one provider at a time, its tabs at the bottom ───────── */

/** A window of the card: what is left, when it resets, and a bar with a tick where an even spend would stand. */
function CardWindow(props: { w: LimitWindow }): JSX.Element {
  const w = props.w;
  const left = leftPct(w);
  const tick = expectedLeft(w);
  const sev = w.severity === 'crit' ? 'crit' : w.severity === 'warn' ? 'warn' : 'ok';
  const reset = !w.resets_at_ms ? '' : w.resets_at_ms <= Date.now() ? t('limits.resetsNow') : t('limits.resetsIn', { at: countdownText(w.resets_at_ms - Date.now()) });
  const pace = paceText(w);
  return (
    <div class={`lc-win ${sev}`} data-kind={w.kind || 'named'}>
      <div class="lc-wt"><b>{t('limits.winLeft', { title: winTitle(w), n: left })}</b><span>{reset}</span></div>
      <div class="lc-bar" role="progressbar" aria-label={winTitle(w)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={left}>
        <i style={`--w:${left}%`} />
        {tick !== null && <u class="lc-tick" style={`--x:${tick}%`} />}
      </div>
      {pace && <div class="lc-pace">{pace}</div>}
    </div>
  );
}

function CardSection(props: { s: UsageSection }): JSX.Element {
  const s = props.s;
  if (s.kind === 'shares') {
    return (
      <div class="lc-sec">
        <b>{said(s.title)}</b>
        {s.shares.map(([name, pct]) => (
          <div class="lc-share" key={name}><span>{name}</span><span class="lc-sbar"><i style={`--w:${Math.max(0, Math.min(100, Math.round(pct)))}%`} /></span><span class="lc-sv">{Math.round(pct)}%</span></div>
        ))}
      </div>
    );
  }
  return (
    <div class="lc-sec">
      <b>{said(s.title)}</b>
      {s.facts.map(([k, v]) => <div class="lc-fact" key={k}><span>{said(k)}</span><span>{said(v)}</span></div>)}
    </div>
  );
}

function updatedText(ms: number | undefined): string {
  if (!ms) return t('limits.updatedUnknown');
  const a = ago(Date.now() - ms);
  return a.unit === 'now' ? t('limits.agoNow') : a.unit === 'min' ? t('limits.agoMin', { n: a.n }) : a.unit === 'h' ? t('limits.agoH', { n: a.n }) : t('limits.agoD', { n: a.n });
}

async function refreshLimits(): Promise<void> {
  if (refreshing.value) return;
  refreshing.value = true;
  try {
    await post('/api/refresh?limits=1');
    await refreshNow();
  } catch { /* the card keeps what it had */ }
  // the providers answer a moment later, and the server says so through the snapshot; the button is free again meanwhile
  window.setTimeout(() => { refreshing.value = false; }, 1500);
}

function ProviderCard(props: { a: AgentUsage }): JSX.Element {
  const a = props.a;
  const windows = a.limits ?? [];
  const hasData = hasNumbers(a);
  return (
    <div class="lc-body" data-agent={a.id} role="tabpanel">
      <div class="lc-head"><b class="lc-name">{a.name}</b><span class="lc-acct">{a.account}</span></div>
      <div class="lc-sub"><span class="lc-upd">{updatedText(a.updated_at_ms)}</span><span class="lc-tier">{a.tier}</span></div>
      {a.stale && <div class="lc-warn">{t('limits.stale')}</div>}
      {windows.map((w, i) => <CardWindow key={i} w={w} />)}
      {(a.extra ?? []).map((w, i) => <CardWindow key={`x${i}`} w={w} />)}
      {a.balance && (
        <div class="lc-bal"><span>{t('limits.balance')}</span><b>{cur(a.balance.remaining, a.balance.currency || 'USD')}</b></div>
      )}
      {(a.sections ?? []).map((s, i) => <CardSection key={i} s={s} />)}
      {!hasData && <div class="lim-none-body lc-none">{a.why ? t('limits.whyNoData', { why: whyText(a.why) }) : a.note || t('limits.noData')}</div>}
      {hasData && a.why && <div class="lim-why">{t('limits.whyStale', { why: whyText(a.why) })}</div>}
      <div class="lc-actions">
        <button type="button" class="lc-act" id="limits-refresh" disabled={refreshing.value} onClick={() => void refreshLimits()}><Gl n="refresh" cls={refreshing.value ? 'spin' : ''} />{refreshing.value ? t('limits.refreshing') : t('limits.refresh')}</button>
        {a.dashboard && <a class="lc-act" href={a.dashboard} target="_blank" rel="noopener noreferrer"><Gl n="dash" />{t('limits.dashboard')}</a>}
        {a.status_page && <a class="lc-act" href={a.status_page} target="_blank" rel="noopener noreferrer"><Gl n="pulse" />{t('limits.statusPage')}</a>}
      </div>
    </div>
  );
}

/** The card under the chip: the provider picked (the first one at first), its windows with how long they last, and the tabs. */
export function LimitsCard(): JSX.Element {
  const tabs = tabAgents(agents());
  const cur0 = tabs.find((a) => a.id === picked.value) ?? tabs[0];
  const pick = (id: string): void => { picked.value = id; };
  const move = (e: KeyboardEvent, at: number): void => {
    const to = e.key === 'ArrowRight' ? at + 1 : e.key === 'ArrowLeft' ? at - 1 : -1;
    if (to < 0 || to >= tabs.length) return;
    e.preventDefault();
    pick(tabs[to].id);
    queueMicrotask(() => document.querySelector<HTMLElement>(`#limits-card [data-tab="${tabs[to].id}"]`)?.focus());
  };
  return (
    <div class="lim-card" id="limits-card">
      {cur0 ? <ProviderCard a={cur0} /> : <div class="none">{t('limits.omarchyNone')}</div>}
      {tabs.length > 0 && (
        <div class="lc-tabs" role="tablist" aria-label={t('limits.providers')}>
          {tabs.map((a, i) => (
            <button key={a.id} type="button" role="tab" class="lc-tab" data-tab={a.id} aria-selected={a.id === cur0?.id} tabIndex={a.id === cur0?.id ? 0 : -1} onClick={() => pick(a.id)} onKeyDown={(e) => move(e, i)}>
              <ProviderIcon id={a.id} /><span>{a.name}</span>
            </button>
          ))}
        </div>
      )}
      <Unconnected />
      <div class="lim-more"><button type="button" class="ld-go" id="limits-more" onClick={() => { closeCard(); panels.limits.value = true; }}>{t('limits.title')}</button></div>
    </div>
  );
}

void locale;
