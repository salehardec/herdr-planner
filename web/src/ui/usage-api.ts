/* What the shipyards did (PLN-46): the load of every yard over the last hours and the day, and what the containers of
   every task ate. The calls mirror src/runners_hist.rs and src/runners_api.rs. */
import { signal } from '@preact/signals';
import { api } from '../api';
import { t, tn } from '../i18n';

/** One sample of a yard's load, in percent of the machine; `null` — not known then (a hole in the line). */
export interface HistPoint { at: number; cpu: number | null; mem: number | null; disk: number | null }

/** What the containers of one task ate on one yard. */
export interface UseRow {
  tag: string;
  /** The board task the label names; empty when this board does not know it. */
  task: string;
  /** CPU time in core-seconds. */
  cpu_sec: number;
  /** The highest sum of the memory of the task's running containers, in bytes. */
  mem_peak: number;
  first_ms: number;
  last_ms: number;
}

export interface YardHistory { points: HistPoint[]; tasks: UseRow[] }

export interface HistData {
  ok: boolean;
  at: number;
  runners: Record<string, YardHistory>;
  fine_step: number;
  coarse_step: number;
  fine_keep_sec: number;
  keep_sec: number;
}

export const hist = signal<HistData | null>(null);
export const histError = signal('');
let inflight: Promise<void> | null = null;

/** Reads the history once; a call while another is on the way joins it. */
export function loadHistory(): Promise<void> {
  if (inflight) return inflight;
  inflight = api<HistData>('/api/runners/history')
    .then((d) => { hist.value = d; histError.value = ''; })
    .catch((e: Error) => { histError.value = e.message; })
    .finally(() => { inflight = null; });
  return inflight;
}

/** The windows the Focus offers: the last hours (minute rows) and the day (ten-minute rows). */
export const HOURS_MS = 3 * 3600 * 1000;
export const DAY_MS = 24 * 3600 * 1000;

export interface TaskUse extends UseRow { runner: string }

/** One row per task of a yard: an old key of the same task (`AL9-4` for `ALP-4`) is the same task. */
function mergeRows(rows: TaskUse[]): TaskUse[] {
  const out = new Map<string, TaskUse>();
  for (const r of rows) {
    const key = `${r.runner}\u0000${r.task || r.tag.toUpperCase()}`;
    const cur = out.get(key);
    if (!cur) {
      out.set(key, r);
      continue;
    }
    cur.cpu_sec += r.cpu_sec;
    cur.mem_peak = Math.max(cur.mem_peak, r.mem_peak);
    cur.first_ms = Math.min(cur.first_ms, r.first_ms);
    cur.last_ms = Math.max(cur.last_ms, r.last_ms);
  }
  return [...out.values()].sort((a, b) => b.cpu_sec - a.cpu_sec);
}


/** A yard's rows, one per task. */
export function yardUsage(rows: UseRow[], runner: string): TaskUse[] {
  return mergeRows(rows.map((r) => ({ ...r, runner })));
}

/** The rows of one board task over every yard: by the task the label names, else by the tag itself. */
export function taskUsage(data: HistData | null, task: { id: string; tag: string }): TaskUse[] {
  if (!data) return [];
  const tag = task.tag.toUpperCase();
  const rows: TaskUse[] = [];
  for (const [runner, h] of Object.entries(data.runners)) {
    for (const u of h.tasks) if (u.task === task.id || (!u.task && u.tag.toUpperCase() === tag)) rows.push({ ...u, runner });
  }
  return mergeRows(rows);
}

/** What those rows add up to: CPU time; the peak memory of the worst yard at one moment, not a sum over time. */
export function totalUsage(rows: TaskUse[]): { cpu_sec: number; mem_peak: number; first_ms: number; last_ms: number } | null {
  if (!rows.length) return null;
  let cpu = 0;
  let peak = 0;
  let first = 0;
  let last = 0;
  for (const r of rows) {
    cpu += r.cpu_sec;
    peak = Math.max(peak, r.mem_peak);
    first = first ? Math.min(first, r.first_ms) : r.first_ms;
    last = Math.max(last, r.last_ms);
  }
  return { cpu_sec: cpu, mem_peak: peak, first_ms: first, last_ms: last };
}

/** CPU time as the page says it: seconds, minutes, then hours with a tenth. */
export function cpuTime(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0.5) return '—';
  if (sec < 60) return tn('runner.usage.sec', Math.round(sec), { n: Math.round(sec) });
  if (sec < 3600) return tn('runner.usage.min', Math.round(sec / 60), { n: Math.round(sec / 60) });
  const h = sec / 3600;
  return t('runner.usage.hour', { h: h >= 10 ? String(Math.round(h)) : h.toFixed(1) });
}

/** A memory size as the page says it: GiB with a tenth, else MiB. */
export function memSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '—';
  if (bytes >= 1024 ** 3) return t('runner.usage.gib', { n: (bytes / 1024 ** 3).toFixed(1) });
  return t('runner.usage.mib', { n: Math.round(bytes / 1024 ** 2) });
}
