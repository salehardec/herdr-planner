/* What the crew screen shows, worked out from the station's answer: who leads each role, which subscriptions are
   unavailable, the fallback chains, the words for «until when». No DOM here, so it is tested on its own. */
import { locale } from '../i18n';
import type { ModelsData, PresetRole, ProgramAvail, StationData, StationProgram } from './station-api';

export const ROLE_IDS = ['coordinator', 'plan', 'reviewer', 'worker', 'watcher', 'light'] as const;
export type RoleId = typeof ROLE_IDS[number];

/** The board's settings of each role (`light` is the planner's own preset). */
export const ROLE_SETTING: Record<RoleId, { agent?: string; effort?: string; fallback?: string }> = {
  coordinator: { agent: 'coordinator.agent', effort: 'coordinator.effort' },
  plan: { agent: 'plan.agent' },
  reviewer: { agent: 'reviewer.agent', effort: 'reviewer.effort', fallback: 'reviewer.fallback' },
  worker: { agent: 'worker.agent', effort: 'worker.effort', fallback: 'worker.fallback' },
  watcher: { agent: 'watcher.agent' },
  light: {},
};

export interface Lead { role: RoleId; agent: string }
export interface Down { program: string; kind: 'limited' | 'signed_out'; until: string; untilMs: number }

/** The agent that leads a role now (empty when nothing is set). `none` (no planner, no Coordinator) is empty too. */
export function leadOf(data: Pick<StationData, 'settings' | 'light'>, role: RoleId): string {
  if (role === 'light') return data.light;
  const name = ROLE_SETTING[role].agent;
  const v = data.settings.find((s) => s.name === name)?.value ?? '';
  return v === 'none' ? '' : v;
}

/** The installed programs that cannot be given work now: out of allowance, or signed out. */
export function downPrograms(programs: StationProgram[]): Down[] {
  const out: Down[] = [];
  for (const p of programs) {
    if (!p.installed) continue;
    const a = p.state.avail;
    if (a.state === 'limited') out.push({ program: p.name, kind: 'limited', until: a.until, untilMs: a.until_ms });
    else if (a.state === 'signed_out') out.push({ program: p.name, kind: 'signed_out', until: '', untilMs: 0 });
  }
  return out;
}

/** The one-line summary: who leads each role, and which subscriptions are down. */
export function summary(data: StationData): { leads: Lead[]; down: Down[] } {
  return {
    leads: ROLE_IDS.map((role) => ({ role, agent: leadOf(data, role) })),
    down: downPrograms(data.programs),
  };
}

/** A fallback value (`codex, claude/claude-opus-5-5` or `none`) as its items. */
export function chainOf(value: string): string[] {
  const v = value.trim();
  if (!v || v === 'none') return [];
  return v.split(',').map((x) => x.trim()).filter(Boolean);
}

/** The items as the board's value: `none` for an empty chain. */
export const chainValue = (items: string[]): string => (items.length ? items.join(', ') : 'none');

/** The program of an agent value or a chain item. */
export const programName = (v: string): string => v.split('/')[0].trim();

/**
 * «until 17:20» / «until Fri 17:20» for a moment in ms: only the time when it is today.
 * `now` is a parameter so that a test can pin it.
 */
export function momentLabel(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const same = new Date(now).toDateString() === d.toDateString();
  const opts: Intl.DateTimeFormatOptions = same ? { hour: '2-digit', minute: '2-digit' } : { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' };
  return new Intl.DateTimeFormat(locale(), opts).format(d);
}

/** When a limited program comes back, in words: the board's own text, else Omarchy's window, else nothing. */
export function untilLabel(a: Pick<ProgramAvail, 'until' | 'until_ms'>, now = Date.now()): string {
  if (a.until) return a.until;
  if (a.until_ms > now) return momentLabel(a.until_ms, now);
  return '';
}

/** The role fields of a preset that are set, in the order the screen reads them. */
export const emptyRole = (): PresetRole => ({ agent: '', effort: '', fallback: '' });

/** The key a preset holds an own role under (`role:<id>`). */
export const presetKey = (id: string): string => `role:${id}`;

/** The keys a preset may hold: the built-in roles and the ids of the own roles. */
export const presetKeys = (own: string[] = []): string[] => [...ROLE_IDS, ...own.map(presetKey)];

export function rolesOrEmpty(roles: Record<string, PresetRole> | undefined, own: string[] = []): Record<string, PresetRole> {
  const out: Record<string, PresetRole> = {};
  for (const r of presetKeys(own)) out[r] = { ...emptyRole(), ...(roles?.[r] ?? {}) };
  return out;
}

/** Whether a preset (as the editor holds it) differs from the saved one. */
export function sameRoles(a: Record<string, PresetRole>, b: Record<string, PresetRole>, own: string[] = []): boolean {
  return presetKeys(own).every((r) => {
    const x = { ...emptyRole(), ...(a[r] ?? {}) };
    const y = { ...emptyRole(), ...(b[r] ?? {}) };
    return x.agent === y.agent && x.effort === y.effort && x.fallback === y.fallback;
  });
}

/** The state of one program for the screen: `ok`, `limited`, `signed_out`, `unknown` or `missing`. */
export const stateOf = (p: StationProgram): ProgramAvail['state'] => p.state.avail.state;

/** A program a fallback chain or a role may still be given work on: installed, not signed out. */
export const offerable = (p: StationProgram): boolean => p.installed && p.state.avail.state !== 'signed_out';

export interface ModelOption { value: string; kind: 'model' | 'default' | 'current' }

/**
 * What the model picker of a program offers: the models the program lists; with no list of its own, the models it was
 * used with, and the program alone (its own default model). `bare` offers the program alone at the top for a program with
 * a list too (a fallback chain may name a program without a model). A value that is set but not offered stays at the top.
 */
export function modelOptions(program: string, m: ModelsData | null, current: string, bare = false): ModelOption[] {
  const out: ModelOption[] = [];
  const mine = current === program || current.startsWith(`${program}/`);
  if (m) {
    const listed = m.names.length > 0;
    if (bare) out.push({ value: program, kind: 'default' });
    for (const n of listed ? m.names : m.suggested) out.push({ value: n, kind: 'model' });
    if (!listed && !bare) out.push({ value: program, kind: 'default' });
  }
  if (mine && current && !out.some((o) => o.value === current)) out.unshift({ value: current, kind: 'current' });
  return out;
}
