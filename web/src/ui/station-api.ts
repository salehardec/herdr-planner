/* The Station (the crew and board settings), the ship's computer, and the calls they make.
   How other screens open them: openStation(), openShip(). The calls mirror src/station.rs, src/ship.rs. */
import { signal } from '@preact/signals';
import { api, ApiError, post } from '../api';
import { t } from '../i18n';

/** The Station screen; `wizard` is the first-run «Экипаж корабля». */
export const stationDlg = signal<{ wizard?: boolean } | null>(null);
/** The ship's computer («Бортовой компьютер»). */
export const shipDlg = signal(false);

export function openStation(wizard = false): void { stationDlg.value = { wizard }; }
export function openShip(): void { shipDlg.value = true; }

/** The first-run wizard is seen once (set by «Готово», «Пропустить» and closing it). */
export const CREW_SEEN_KEY = 'herdr-planner.crew-seen';
export const crewSeen = (): boolean => { try { return localStorage.getItem(CREW_SEEN_KEY) === '1'; } catch { return true; } };
export function markCrewSeen(): void { try { localStorage.setItem(CREW_SEEN_KEY, '1'); } catch { /* private mode */ } }

/** Closes the Station screen; the wizard counts as seen however it is closed. */
export function closeStation(): void {
  if (stationDlg.value?.wizard) markCrewSeen();
  stationDlg.value = null;
}

/* ───────── types ───────── */

export interface StationSetting {
  name: string;
  kind: string;
  value: string;
  from: string;
  place: string;
  own: boolean;
  note: string;
}

/** Whether a program can be given work now: `missing` (not installed), `signed_out`, `limited` (out of allowance), `ok`, `unknown`. */
export type AvailKind = 'missing' | 'signed_out' | 'limited' | 'ok' | 'unknown';

export interface ProgramAvail {
  state: AvailKind;
  /** When the allowance comes back (ms) when Omarchy's windows say it; 0 otherwise. */
  until_ms: number;
  /** The board's own words for when («17:00»); empty when it does not say. */
  until: string;
  reason: string;
  source: '' | 'board' | 'omarchy' | 'doctor';
}

export interface ProgramWindow { label: string; percent: number; resets_at_ms: number; severity: string }

export interface ProgramState {
  signed_in: boolean | null;
  account: string;
  unknown_why: string;
  avail: ProgramAvail;
  windows: ProgramWindow[];
  limit_why: string;
  stale: boolean;
}

export interface ProviderInfo { name: string; source: 'list' | 'usage' }
export interface RoleUse { role: string; field: 'agent' | 'fallback'; value: string }
export interface TaskUse { id: string; title: string; status: string }

export interface StationProgram {
  name: string;
  installed: boolean;
  version: string;
  signed_in: boolean | null;
  line: string;
  state: ProgramState;
  providers: ProviderInfo[];
  roles: RoleUse[];
  tasks: TaskUse[];
  models: { count: number; loading: boolean; listed: boolean; suggested: number };
}

/** What a preset says for one role; an empty field is «not set». */
export interface PresetRole { agent: string; effort: string; fallback: string }
export interface PresetItem { id: string; name: string; roles: Record<string, PresetRole>; diff: number }
export interface PresetActive { id: string; name: string; state: 'matches' | 'differs'; diff: number }
export interface PresetsData { list: PresetItem[]; active: PresetActive | null }

export interface PresetStep {
  role: string;
  field: 'agent' | 'effort' | 'fallback';
  /** The board's setting (`worker.agent`), or `light` (the planner's own preset). */
  setting: string;
  was: string;
  was_from: string;
  /** `null`: the board's `unset`. */
  now: string | null;
  error: string;
  command: string;
}

export interface PresetPreview {
  ok: true;
  preset: { id: string; name: string };
  repo: string;
  steps: PresetStep[];
  valid: boolean;
  /** Programs whose models are still being read: their steps are not checked yet. */
  loading: string[];
  light_skipped: boolean;
  affects: { running: Affect[]; waiting: Affect[] };
}

export interface StationData {
  ok: true;
  level: 'you' | 'repo';
  repo: string;
  repos: string[];
  version: string;
  settings: StationSetting[];
  programs: StationProgram[];
  light: string;
  presets: PresetsData;
}

export interface ModelsData {
  ok: true;
  program: string;
  names: string[];
  effort: string[];
  note: string;
  loading: boolean;
  /** The program lists its models itself (else `suggested` are the ones it was used with). */
  listed: boolean;
  suggested: string[];
  providers: ProviderInfo[];
}

export interface Affect { id: string; title: string; status: string; tag?: string; app?: string }

export interface Preview {
  ok: true;
  name: string;
  repo: string;
  was: string;
  was_from: string;
  now: string | null;
  valid: boolean;
  error: string;
  affects: Affect[];
  note: string;
}

export interface SetResult { ok: true; name: string; repo: string; was: string; now: string | null; out: string }

export interface ShipData {
  ok: true;
  projects_dir: string;
  listen: string;
  port: number;
  user: string;
  auth_required: boolean;
  tls: boolean;
  config_file: string;
  can_edit: boolean;
}

export interface ShipResult { ok: true; changed: { name: string; was: string; now: string }[]; restart: boolean }

/** A failed call: the words for the page (the board's own when it refused). */
export interface CallError { message: string; status: number }

export function callError(e: unknown): CallError {
  const err = e as { message?: string; status?: number; data?: { error?: string } };
  return {
    message: err.data?.error || err.message || 'error',
    status: err.status ?? 0,
  };
}

/* ───────── the calls ───────── */

const q = (repo = ''): string => (repo ? `repo=${encodeURIComponent(repo)}` : '');

export const stationGet = (repo = ''): Promise<StationData> =>
  api<StationData>(`/api/station?${q(repo)}`).then((d) => { lightCache = { value: d.light, at: Date.now() }; return d; });

export const stationModels = (program: string): Promise<ModelsData> =>
  api<ModelsData>(`/api/station/models?program=${encodeURIComponent(program)}`);

export interface ChangeBody { name: string; value?: string; unset?: boolean; repo?: string }

export const stationPreview = (b: ChangeBody): Promise<Preview> => post<Preview>('/api/station/preview', b);

export const stationSet = (b: ChangeBody): Promise<SetResult> => post<SetResult>('/api/station/set', { ...b, confirm: true });

/** The «Лёгкие задачи» preset (`''` clears it). */
export const stationLight = (agent: string): Promise<{ ok: true; light: string }> =>
  post('/api/station/light', { agent, confirm: true });

export interface PresetOp {
  op: 'create' | 'capture' | 'template' | 'update' | 'rename' | 'delete';
  id?: string;
  name?: string;
  roles?: Record<string, PresetRole>;
  kind?: string;
  repo?: string;
}

/** Create, capture, rename, update or delete a preset. The planner keeps them; the board is not asked. */
export const presetsPost = (b: PresetOp): Promise<{ ok: true; id: string }> => post('/api/station/presets', b);

export const presetPreview = (id: string, repo = ''): Promise<PresetPreview> =>
  post<PresetPreview>('/api/station/preset/preview', { id, repo: repo || undefined });

export const presetApply = (id: string, repo = ''): Promise<{ ok: true; preset: string; repo: string; ran: string[] }> =>
  post('/api/station/preset/apply', { id, repo: repo || undefined, confirm: true });

let lightCache: { value: string; at: number } | null = null;

/**
 * The «Лёгкие задачи» preset, for the todo → task choice («Лёгкая»). Reads the last station answer when
 * it is recent, otherwise asks the station. `''` when there is no preset or it cannot be asked.
 */
export async function lightPreset(): Promise<string> {
  if (lightCache && Date.now() - lightCache.at < 60000) return lightCache.value;
  try {
    return (await stationGet()).light;
  } catch {
    return '';
  }
}

export const shipGet = (): Promise<ShipData> => api<ShipData>('/api/ship');

/** A POST whose 401 is an answer (a wrong old password), not a reason to send the page to the login. */
async function postAnswer<T>(path: string, body: unknown): Promise<T> {
  const r = await fetch(path, { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) {
    const e = new ApiError(data.error || `HTTP ${r.status}`);
    e.status = r.status;
    e.data = data;
    throw e;
  }
  return data as T;
}

export const shipPost = (b: Record<string, unknown>): Promise<ShipResult> => postAnswer<ShipResult>('/api/ship', { ...b, confirm: true });

/* ───────── small helpers the screens share ───────── */

/** The roles of the crew, as the settings name them. */
export const ROLE_SETTINGS = [
  'coordinator.agent', 'coordinator.effort', 'plan.agent', 'reviewer.agent', 'reviewer.effort', 'reviewer.fallback',
  'worker.agent', 'worker.effort', 'worker.fallback',
];

/** Where a value comes from, in words. */
export function fromLabel(from: string): string {
  const key: Record<string, string> = { you: 'st.from.you', 'built in': 'st.from.builtin', repo: 'st.from.repo', task: 'st.from.task', file: 'st.from.file' };
  return key[from] ? t(key[from]) : from;
}

/** The programs the crew may name, in the order the page offers them. */
export const PROGRAM_ORDER = [
  'claude', 'codex', 'omp', 'pi', 'opencode', 'agy', 'copilot', 'cursor-agent', 'droid', 'grok', 'devin', 'gemini',
  'kimi', 'qwen', 'kilo', 'hermes', 'letta', 'mastracode', 'qodercli',
];

/** The program of an agent value («codex/gpt-5» → «codex»). */
export const programOf = (agent: string): string => (agent.includes('/') ? agent.slice(0, agent.indexOf('/')) : agent);
/** The model of an agent value («codex/gpt-5» → «gpt-5»); the whole value when it has no program. */
export const modelOf = (agent: string): string => (agent.includes('/') ? agent.slice(agent.indexOf('/') + 1) : '');

export const isBool = (v: string): boolean => v === 'true' || v === 'false';
