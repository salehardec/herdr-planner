/* The Station (a galaxy's crew and board settings), the ship's computer, and the calls they make.
   How other screens open them: openStation(label), openShip(). The calls mirror src/station.rs, src/ship.rs. */
import { signal } from '@preact/signals';
import { api, ApiError, post } from '../api';
import { t } from '../i18n';

/** The Station screen: `galaxy` is `''` for this machine's own station; `wizard` is the first-run «Экипаж корабля». */
export const stationDlg = signal<{ galaxy: string; wizard?: boolean } | null>(null);
/** The ship's computer («Бортовой компьютер»). */
export const shipDlg = signal(false);
/** The arrival notice: the ship has landed in a far galaxy (`label`). */
export const arrival = signal<{ label: string } | null>(null);

export function openStation(galaxy: string, wizard = false): void { stationDlg.value = { galaxy, wizard }; }
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

/** The arrival notice of one galaxy: «не показывать для этой галактики». */
export const arriveKey = (label: string): string => `herdr-planner.arrive.${label}`;
export function arrivalHidden(label: string): boolean {
  try { return localStorage.getItem(arriveKey(label)) === 'hidden'; } catch { return false; }
}
export function hideArrival(label: string): void {
  try { localStorage.setItem(arriveKey(label), 'hidden'); } catch { /* private mode */ }
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

export interface StationProgram {
  name: string;
  installed: boolean;
  version: string;
  signed_in: boolean | null;
  line: string;
}

export interface StationData {
  ok: true;
  galaxy: string;
  level: 'you' | 'repo';
  repo: string;
  repos: string[];
  version: string;
  settings: StationSetting[];
  programs: StationProgram[];
  light: string;
}

export interface ModelsData {
  ok: true;
  program: string;
  names: string[];
  effort: string[];
  note: string;
  loading: boolean;
}

export interface Affect { id: string; title: string; status: string }

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

/** A failed call: the words for the page (the board's own when it refused) and the machine's reason when it did not answer. */
export interface CallError { message: string; status: number; why?: { line: string; detail: string[] } }

export function callError(e: unknown): CallError {
  const err = e as { message?: string; status?: number; data?: { error?: string; why?: { line: string; detail: string[] } } };
  return {
    message: err.data?.error || err.message || 'error',
    status: err.status ?? 0,
    why: err.data?.why,
  };
}

/* ───────── the calls ───────── */

const q = (galaxy: string, repo = ''): string => `galaxy=${encodeURIComponent(galaxy)}${repo ? `&repo=${encodeURIComponent(repo)}` : ''}`;

export const stationGet = (galaxy: string, repo = ''): Promise<StationData> =>
  api<StationData>(`/api/station?${q(galaxy, repo)}`).then((d) => { lightCache.set(galaxy, { value: d.light, at: Date.now() }); return d; });

export const stationModels = (galaxy: string, program: string): Promise<ModelsData> =>
  api<ModelsData>(`/api/station/models?galaxy=${encodeURIComponent(galaxy)}&program=${encodeURIComponent(program)}`);

export interface ChangeBody { galaxy: string; name: string; value?: string; unset?: boolean; repo?: string }

export const stationPreview = (b: ChangeBody): Promise<Preview> => post<Preview>('/api/station/preview', b);

export const stationSet = (b: ChangeBody): Promise<SetResult> => post<SetResult>('/api/station/set', { ...b, confirm: true });

/** The «Лёгкие задачи» preset of a galaxy (`''` clears it). */
export const stationLight = (galaxy: string, agent: string): Promise<{ ok: true; light: string }> =>
  post('/api/station/light', { galaxy, agent, confirm: true });

const lightCache = new Map<string, { value: string; at: number }>();

/**
 * The «Лёгкие задачи» preset of a galaxy, for the todo → task choice («Лёгкая»). Reads the last station answer when
 * it is recent, otherwise asks the station. `''` when the galaxy has no preset or cannot be asked.
 */
export async function lightPreset(galaxy: string): Promise<string> {
  const hit = lightCache.get(galaxy);
  if (hit && Date.now() - hit.at < 60000) return hit.value;
  try {
    return (await stationGet(galaxy)).light;
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

export const galaxyRemove = (label: string): Promise<{ ok: true; label: string }> => post('/api/galaxy/remove', { label, confirm: true });

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
export const PROGRAM_ORDER = ['claude', 'codex', 'omp', 'pi', 'opencode', 'agy', 'copilot', 'cursor-agent', 'droid', 'grok', 'devin', 'gemini'];

/** The program of an agent value («codex/gpt-5» → «codex»). */
export const programOf = (agent: string): string => (agent.includes('/') ? agent.slice(0, agent.indexOf('/')) : agent);
/** The model of an agent value («codex/gpt-5» → «gpt-5»); the whole value when it has no program. */
export const modelOf = (agent: string): string => (agent.includes('/') ? agent.slice(agent.indexOf('/') + 1) : '');

export const isBool = (v: string): boolean => v === 'true' || v === 'false';
