/* The memory lint on the panel: which entries are marked, how many, how long drafts have lain. The marks come from the planner
   (src/memory_lint.rs); the page only counts and words them. Nothing here removes anything. */
import type { MemEntry, MemLint } from './memory-api';

const DAY = 86_400_000;

/** The entries the lint marks. */
export const marked = (entries: readonly MemEntry[]): MemEntry[] => entries.filter((e) => (e.lint?.length ?? 0) > 0);

/** The entries that have a mark of this kind. */
export const withKind = (entries: readonly MemEntry[], kind: MemLint['kind']): MemEntry[] => entries.filter((e) => e.lint?.some((l) => l.kind === kind));

/** Whole days from `ms` to `now` (0 for a time in the future or unknown). */
export const daysSince = (ms: number, now = Date.now()): number => (ms > 0 && now > ms ? Math.floor((now - ms) / DAY) : 0);

/** `2026-10-22` → `22.10.2026`. */
export const dmy = (iso: string): string => iso.replace(/^(\d{4})-(\d{2})-(\d{2})$/, '$3.$2.$1');

/** The drafts that have lain unsent: how many and since when (`since` 0 = none). */
export const stuckOf = (entries: readonly MemEntry[]): { n: number; ids: string[] } => {
  const s = entries.filter((e) => e.stuck);
  return { n: s.length, ids: s.map((e) => e.id) };
};
