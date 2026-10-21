import type { JSX } from 'preact';
import { panels, snap } from '../store';
import { t, locale } from '../i18n';
import { dayTimeOf } from '../util/dates';
import { money } from '../util/format';
import type { AgentUsage, LimitWindow } from '../types';
import { Gl } from './Gl';
import './limits.css';

const agents = (): AgentUsage[] => snap.value?.limits?.agents ?? [];

function untilText(ms: number): string {
  const left = Number(ms) - Date.now();
  if (left <= 0) return t('limits.resetPassed');
  if (left < 3600000) return t('limits.inMin', { n: Math.max(1, Math.round(left / 60000)) });
  if (left < 86400000) return t('limits.inHours', { n: Math.round(left / 3600000) });
  return t('limits.inDays', { n: Math.round(left / 86400000) });
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

export function LimitsChip(): JSX.Element {
  const worst = worstLimit();
  const open = panels.limits.value;
  if (!worst) {
    return (
      <button class="btn limchip" id="hud-limits" type="button" aria-pressed={open} title={t('limits.noneTip')} onClick={() => { panels.limits.value = !open; }}>
        <Gl n="limit" />{t('limits.chip')} <span class="faint">{t('limits.noData')}</span>
      </button>
    );
  }
  const remaining = Math.max(0, 100 - Math.round(worst.window.percent * 100));
  const level = worst.window.severity === 'crit' ? ' lim-crit' : worst.window.severity === 'warn' ? ' lim-warn' : '';
  const stale = worst.agent.stale ? ` · ${t('limits.stale')}` : '';
  return (
    <button
      class={`btn limchip${level}`} id="hud-limits" type="button" aria-pressed={open} onClick={() => { panels.limits.value = !open; }}
      title={`${worst.agent.name} · ${worst.window.label}: ${t('limits.left', { n: remaining })}${stale}`}
    >
      <Gl n="limit" /><b>{remaining}%</b> <span class="lim-ag">{worst.agent.name} · {worst.window.label}</span>
    </button>
  );
}

function Window(props: { w: LimitWindow }): JSX.Element {
  const w = props.w;
  const used = Math.max(0, Math.min(100, Math.round(Number(w.percent || 0) * 100)));
  const remaining = 100 - used;
  const sev = w.severity === 'crit' ? 'crit' : w.severity === 'warn' ? 'warn' : 'ok';
  const reset = w.resets_at_ms ? t('limits.reset', { at: dayTimeOf(w.resets_at_ms), left: untilText(w.resets_at_ms) }) : t('limits.noReset');
  return (
    <div class={`lim-win ${sev}`}>
      <div class="lim-t"><span class="lim-label">{w.label}</span><b>{t('limits.left', { n: remaining })}</b></div>
      <div class="lim-bar" role="progressbar" aria-label={w.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={used} title={t('limits.used', { n: used })}><i style={`--w:${used}%`} /></div>
      <div class="lim-meta"><span>{reset}</span></div>
    </div>
  );
}

function Agent(props: { a: AgentUsage }): JSX.Element {
  const a = props.a;
  const windows = a.limits ?? [];
  const level = windows.reduce<'ok' | 'warn' | 'crit'>((lv, w) => (w.severity === 'crit' || lv === 'crit' ? 'crit' : w.severity === 'warn' || lv === 'warn' ? 'warn' : 'ok'), 'ok');
  const stale = a.stale === true;
  const staleMin = Math.max(1, Math.round((snap.value?.limits?.stale_after_secs ?? 1800) / 60));
  return (
    <div class={`lim-agent ${level}${stale ? ' stale' : ''}`} data-agent={a.id}>
      <div class="lim-head"><b>{a.name}</b>{a.tier && <span class="lim-tier">{a.tier}</span>}<span class="grow" />{stale && <span class="lim-stale" title={t('limits.staleTip', { n: staleMin })}>{t('limits.staleShort')}</span>}</div>
      {a.status && (windows.length || a.balance) ? <div class="lim-status">{a.status}</div> : null}
      {windows.map((w, i) => <Window key={i} w={w} />)}
      {a.balance && (
        <div class="lim-bal">
          <span>{t('limits.balance')}</span><b>{money(a.balance.remaining)} {a.balance.currency || 'USD'}</b>
          {a.balance.estimated && <span class="lim-est">{t('limits.estimate')}</span>}
          {Number(a.balance.funded) > 0 && <span>{t('limits.of', { n: money(a.balance.funded as number), c: a.balance.currency || 'USD' })}</span>}
        </div>
      )}
      {!windows.length && !a.balance && <div class="lim-none-body">{a.note || t('limits.noData')}</div>}
      <div class="lim-foot">{a.updated_at_ms ? t('limits.updated', { at: dayTimeOf(a.updated_at_ms) }) : t('limits.updatedUnknown')}</div>
    </div>
  );
}

export function LimitsPop(): JSX.Element | null {
  if (!panels.limits.value) return null;
  const list = agents();
  return (
    <div class="panel pop limits" role="dialog" aria-label={t('limits.title')}>
      <div class="lim-pop-head"><b>{t('limits.title')}</b><span>{t('limits.sub')}</span><span class="grow" /><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.limits.value = false; }}><Gl n="close" /></button></div>
      <div class="lim-list">{list.length ? list.map((a) => <Agent key={a.id} a={a} />) : <div class="none">{t('limits.omarchyNone')}</div>}</div>
    </div>
  );
}

void locale;
