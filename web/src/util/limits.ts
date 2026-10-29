/* The arithmetic behind the subscription bar and card (PLN-84): what is left, how a window's use compares with an even spend
   across it, which windows the bar shows, in which order the providers stand. Pure: the words are put to it in ui/Limits.tsx. */
import type { AgentUsage, LimitWindow } from '../types';

/** The providers' tabs, in the order herdr-gpui lists them; others follow in the order the server sent. */
export const PROVIDER_ORDER = ['codex', 'claude', 'kimi', 'deepseek'];

/** What the providers the planner reads itself are called (the server's names, for a provider it has no reading of yet). */
export const PROVIDER_NAMES: Record<string, string> = { codex: 'Codex', claude: 'Claude', kimi: 'Kimi Code', deepseek: 'DeepSeek' };

/** The programs that carry only stats, never a subscription (they appear in the panel, not in the bar or the tabs). */
const STATS_ONLY = ['omp', 'pi'];

export const usedPct = (w: LimitWindow): number => Math.max(0, Math.min(100, Math.round(Number(w.percent || 0) * 100)));
export const leftPct = (w: LimitWindow): number => 100 - usedPct(w);

export interface Countdown { d: number; h: number; m: number }

/** `47m`, `2h 33m`, `4d 11h`: the coarsest two units that still say when (rounded up to the minute, as herdr-gpui does). */
export function countdown(ms: number): Countdown {
  const minutes = Math.max(0, Math.ceil(ms / 60000));
  return { d: Math.floor(minutes / 1440), h: Math.floor(minutes / 60) % 24, m: minutes % 60 };
}

export type Standing = 'ran_out' | 'reserve' | 'deficit' | 'pace';
export interface PaceLine {
  standing: Standing;
  /** Points in reserve or in deficit. */
  n: number;
  /** When it runs out at this pace (ms from the reading), else it lasts until the reset. */
  runsOutMs: number | null;
}

/** `11% in reserve · Lasts until reset`: null when the pace cannot be judged. */
export function paceLine(w: LimitWindow): PaceLine | null {
  if (w.reserve === undefined || w.reserve === null || !Number.isFinite(w.reserve)) return null;
  if (usedPct(w) >= 100) return { standing: 'ran_out', n: 0, runsOutMs: null };
  const margin = w.reserve;
  const runsOutMs = w.runs_out_ms ?? null;
  if (margin >= 0.5) return { standing: 'reserve', n: Math.round(margin), runsOutMs };
  if (margin <= -0.5) return { standing: 'deficit', n: Math.round(-margin), runsOutMs };
  return { standing: 'pace', n: 0, runsOutMs };
}

/** Where the tick of the bar stands (share of the bar, from the left): the share an even spend would still have left. */
export function expectedLeft(w: LimitWindow): number | null {
  if (w.reserve === undefined || w.reserve === null || !Number.isFinite(w.reserve)) return null;
  const expectedUsed = Number(w.percent) * 100 + w.reserve;
  return Math.max(0, Math.min(100, 100 - expectedUsed));
}

/** The windows a provider's bar shows: the two tightest (as herdr-gpui does), in the order the provider lists them. */
export function tightest(windows: LimitWindow[], n = 2): LimitWindow[] {
  const ranked = windows.map((w, i) => ({ w, i })).sort((a, b) => b.w.percent - a.w.percent || a.i - b.i).slice(0, n);
  return ranked.sort((a, b) => a.i - b.i).map((x) => x.w);
}

/** Whether a provider has anything to put in the bar. */
export const hasNumbers = (a: AgentUsage): boolean => (a.limits?.length ?? 0) > 0 || !!a.balance;

export const level = (windows: LimitWindow[]): 'ok' | 'warn' | 'crit' =>
  windows.reduce<'ok' | 'warn' | 'crit'>((lv, w) => (w.severity === 'crit' || lv === 'crit' ? 'crit' : w.severity === 'warn' || lv === 'warn' ? 'warn' : 'ok'), 'ok');

/** The providers the bar shows: those with numbers, the one picked in the card first, then the ones closest to a limit (herdr-gpui's order). */
export function barAgents(agents: AgentUsage[], picked: string): AgentUsage[] {
  const rank = (a: AgentUsage): number => Math.max(0, ...(a.limits ?? []).map((w) => w.percent));
  const shown = agents.filter((a) => hasNumbers(a) && !STATS_ONLY.includes(a.id)).map((a, i) => ({ a, i }));
  shown.sort((x, y) => rank(y.a) - rank(x.a) || x.i - y.i);
  const out = shown.map((x) => x.a);
  const at = out.findIndex((a) => a.id === picked);
  if (at > 0) out.unshift(...out.splice(at, 1));
  return out;
}

/** The card's tabs: the providers with numbers, and those the planner found a sign-in for but got no numbers from (they say why). */
export function tabAgents(agents: AgentUsage[]): AgentUsage[] {
  const rank = (a: AgentUsage): number => { const i = PROVIDER_ORDER.indexOf(a.id); return i < 0 ? PROVIDER_ORDER.length : i; };
  return agents
    .filter((a) => !STATS_ONLY.includes(a.id) && (hasNumbers(a) || !!a.source || !!a.why))
    .map((a, i) => ({ a, i }))
    .sort((x, y) => rank(x.a) - rank(y.a) || x.i - y.i)
    .map((x) => x.a);
}

/** How long ago, as a count and a unit, for «Updated … ago». */
export function ago(ms: number): { n: number; unit: 'now' | 'min' | 'h' | 'd' } {
  const min = Math.floor(Math.max(0, ms) / 60000);
  if (min < 1) return { n: 0, unit: 'now' };
  if (min < 60) return { n: min, unit: 'min' };
  if (min < 1440) return { n: Math.floor(min / 60), unit: 'h' };
  return { n: Math.floor(min / 1440), unit: 'd' };
}

/** A single top-bar reading, independent of the selected provider. Expired windows no longer consume an allowance. */
export function limitSummary(agents: AgentUsage[], now = Date.now()): { percent: number | null; level: 'ok' | 'warn' | 'crit'; unavailable: boolean } {
  const subscriptions = tabAgents(agents);
  const windows = subscriptions.flatMap((a) => a.limits ?? []).filter((w) => !w.resets_at_ms || w.resets_at_ms > now);
  const percent = windows.length ? Math.max(...windows.map(usedPct)) : null;
  const unavailable = subscriptions.some((a) => a.stale || !!a.why || !hasNumbers(a));
  const depleted = subscriptions.some((a) => a.balance && a.balance.remaining <= 0);
  const severity = level(windows);
  return { percent, unavailable, level: depleted || severity === 'crit' || (percent ?? 0) >= 95 ? 'crit' : unavailable || severity === 'warn' || (percent ?? 0) >= 80 ? 'warn' : 'ok' };
}
