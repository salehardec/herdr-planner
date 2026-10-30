/* «В память» in bulk: which proposed lines the human ticked, the text they edited, and the one preview → «да» → write for
   all of them. The selection and the edits live here, not in a card, so a row of «Ждёт тебя» and a card of the universe's
   panel show the same ticks. */
import { signal } from '@preact/signals';
import { prepareAnswer } from '../nav';
import { resolveRequest } from '../request-queue';
import { requests, snap } from '../store';
import type { MemorySuggestion } from '../types';
import { t } from '../i18n';
import { toast } from '../toast';
import { dismissSuggestion, keepSuggestion, loadMemory, previewSuggestion, suggestionToast, type Accept, type MemPreview } from './memory-api';

type Line = { kind: string; text: string };

/** The text of the lines the human changed, by task; what is not here is as the model proposed it. */
export const memEdits = signal<Record<string, Line[]>>({});
/** The ticked lines: `task#index`. */
export const memPick = signal<ReadonlySet<string>>(new Set());

export const linesOf = (s: MemorySuggestion): Line[] => memEdits.value[s.task] ?? s.lines;
export const setLines = (task: string, lines: Line[]): void => { memEdits.value = { ...memEdits.value, [task]: lines }; };
export const pickKey = (task: string, i: number): string => `${task}#${i}`;

/** The lines that can be kept: the empty ones are not. */
const live = (s: MemorySuggestion): number[] => linesOf(s).flatMap((l, i) => (l.text.trim() ? [i] : []));

export function cardPick(s: MemorySuggestion): 'all' | 'some' | 'none' {
  const idx = live(s);
  const n = idx.filter((i) => memPick.value.has(pickKey(s.task, i))).length;
  return n === 0 ? 'none' : n === idx.length ? 'all' : 'some';
}

const withPick = (f: (next: Set<string>) => void): void => { const next = new Set(memPick.value); f(next); memPick.value = next; };

export const toggleLine = (s: MemorySuggestion, i: number): void => withPick((n) => { const k = pickKey(s.task, i); if (!n.delete(k)) n.add(k); });
export function toggleCard(s: MemorySuggestion): void {
  const on = cardPick(s) !== 'all';
  withPick((n) => { for (const i of live(s)) { if (on) n.add(pickKey(s.task, i)); else n.delete(pickKey(s.task, i)); } });
}
export const pickAll = (list: MemorySuggestion[]): void => withPick((n) => { for (const s of list) for (const i of live(s)) n.add(pickKey(s.task, i)); });
export const clearPick = (list?: MemorySuggestion[]): void => withPick((n) => {
  if (!list) { n.clear(); return; }
  for (const s of list) for (const k of [...n]) if (k.startsWith(`${s.task}#`)) n.delete(k);
});

/** The cards with a ticked line, each with the ticked lines (edited text, empty ones left out). */
export function picked(list: MemorySuggestion[]): { s: MemorySuggestion; lines: Line[] }[] {
  const out: { s: MemorySuggestion; lines: Line[] }[] = [];
  for (const s of list) {
    const all = linesOf(s);
    const lines = live(s).filter((i) => memPick.value.has(pickKey(s.task, i))).map((i) => all[i]);
    if (lines.length) out.push({ s, lines });
  }
  return out;
}

/** What the preview of the picked cards says, waiting for the human's «да». */
export interface Batch {
  scope: string;
  items: { s: MemorySuggestion; lines: Line[]; p: MemPreview }[];
  busy: boolean;
  err: string;
}
export const memBatch = signal<Batch | null>(null);

export async function batchAsk(scope: string, list: MemorySuggestion[]): Promise<void> {
  const chosen = picked(list);
  if (!chosen.length || memBatch.value?.busy) return;
  memBatch.value = { scope, items: [], busy: true, err: '' };
  try {
    const items: Batch['items'] = [];
    for (const c of chosen) items.push({ ...c, p: await previewSuggestion(c.s.task, c.lines) });
    memBatch.value = { scope, items, busy: false, err: '' };
  } catch (e) {
    memBatch.value = null;
    toast((e as Error)?.message || t('mem.failed'), { icon: 'failed' });
  }
}

export const batchCancel = (): void => { memBatch.value = null; };

/** One «да» for every card of the preview; a card that fails stays, with the reason, the others are written. */
export async function batchKeep(): Promise<void> {
  const b = memBatch.value;
  if (!b || b.busy) return;
  memBatch.value = { ...b, busy: true, err: '' };
  const completions = b.items.map((it) => ({ row: requests.value.find((r) => r.key === `memory:${it.s.task}`), done: prepareAnswer(it.s.task, 'memory') }));
  const successful: typeof completions = [];
  const failed: Batch['items'] = [];
  const why: string[] = [];
  let lines = 0;
  const repos = new Set<string>();
  for (const [index, it] of b.items.entries()) {
    const a: Accept = it.p.risk === 'block' ? { accept_public: true } : it.p.risk === 'confirm' ? { accept_risk: true } : {};
    try {
      const result = await keepSuggestion(it.s.task, it.lines, it.p.token, a);
      suggestionToast(it.s.task, it.s.repo, t('mem.suggestKept', { repo: it.s.repo }), result.undo);
      successful.push(completions[index]);
      lines += it.lines.length;
      repos.add(it.s.repo);
    } catch (e) { failed.push(it); why.push(`${it.s.tag}: ${(e as Error)?.message || t('mem.failed')}`); }
  }
  for (const c of successful) if (c.row) resolveRequest(c.row);
  for (const c of successful) await c.done();
  for (const r of repos) void loadMemory(r);
  const done = b.items.length - failed.length;
  if (done) toast(t('mem.batchKept', { n: lines, c: done }));
  memBatch.value = failed.length ? { ...b, items: failed, busy: false, err: why.join('\n') } : null;
}

/** «Не нужно» for every card with a ticked line: the whole card goes, as it does from its own button. */
export async function batchDismiss(list: MemorySuggestion[]): Promise<void> {
  const chosen = picked(list);
  if (!chosen.length) return;
  const completions = chosen.map((c) => ({ row: requests.value.find((r) => r.key === `memory:${c.s.task}`), done: prepareAnswer(c.s.task, 'memory') }));
  const successful: typeof completions = [];
  let done = 0;
  const repos = new Set<string>();
  for (const [index, c] of chosen.entries()) {
    try { const result = await dismissSuggestion(c.s.task); suggestionToast(c.s.task, c.s.repo, t('wait.dismissed'), result.undo); successful.push(completions[index]); done++; repos.add(c.s.repo); } catch (e) { toast(`${c.s.tag}: ${(e as Error)?.message || t('mem.failed')}`, { icon: 'failed' }); }
  }
  for (const c of successful) if (c.row) resolveRequest(c.row);
  for (const c of successful) await c.done();
  for (const r of repos) void loadMemory(r);
  if (done) toast(t('mem.batchDismissed', { n: done }));
}

// What has left the board's list (kept, dropped) takes its ticks, its edits and its preview with it.
snap.subscribe((s) => {
  if (!s) return;
  const tasks = new Set((s.memory_suggestions ?? []).map((m) => m.task));
  const pick = [...memPick.value].filter((k) => tasks.has(k.slice(0, k.lastIndexOf('#'))));
  if (pick.length !== memPick.value.size) memPick.value = new Set(pick);
  const keep = Object.keys(memEdits.value).filter((k) => tasks.has(k));
  if (keep.length !== Object.keys(memEdits.value).length) memEdits.value = Object.fromEntries(keep.map((k) => [k, memEdits.value[k]]));
  const b = memBatch.value;
  if (b && !b.busy) {
    const items = b.items.filter((i) => tasks.has(i.s.task));
    if (items.length !== b.items.length) memBatch.value = items.length ? { ...b, items } : null;
  }
});
