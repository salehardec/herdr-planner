/* Which turns of the Coordinator's chat are unread: the pure part (the state in the browser is in chat-unread.ts). */
import type { TurnData } from './types';

/** Every turn up to `base` is read; `seen` are the read ones above it (kept short: it folds into `base`). */
export interface ReadState { base: number; seen: number[] }

/** The turns above `base` that are not in `seen` and that have an answer. A turn that is not loaded is taken for answered. */
export function unreadOf(rs: ReadState | null, turns: TurnData[], total: number): number[] {
  if (!rs) return [];
  const have = new Map(turns.map((x) => [x.n, x]));
  const seen = new Set(rs.seen);
  const out: number[] = [];
  for (let n = rs.base + 1; n < total; n++) {
    if (seen.has(n)) continue;
    const x = have.get(n);
    if (x && !x.a) continue;
    out.push(n);
  }
  return out;
}

/** The state after these turns were read: they fold into `base` when they touch it. */
export function withRead(rs: ReadState, ns: number[]): ReadState {
  const seen = new Set(rs.seen);
  for (const n of ns) if (n > rs.base) seen.add(n);
  let base = rs.base;
  while (seen.has(base + 1)) { base++; seen.delete(base); }
  return { base, seen: [...seen].sort((a, b) => a - b) };
}

/**
 * The first look at a chat (nothing stored): what is there now counts as read, so a page that has never been told about the chat
 * does not open with a hundred lights. A transcript that got much shorter than the stored place is a new conversation: the place goes back to its end.
 */
export function settled(rs: ReadState | null, turns: TurnData[], total: number): ReadState {
  const last = total - 1;
  if (!rs) {
    // The last turn may still be working (no answer yet): its answer is news.
    const tail = turns.find((x) => x.n === last);
    return { base: tail && !tail.a ? last - 1 : last, seen: [] };
  }
  // (One turn fewer is a transcript caught half-written, not a new conversation.)
  if (rs.base > last + 1) return { base: last, seen: rs.seen.filter((n) => n <= last) };
  // A turn that ended without words of the Coordinator's will never bring news: it is read, so it does not hold the place back.
  const holes = turns.filter((x) => !x.a && x.n > rs.base && x.n < last && !rs.seen.includes(x.n)).map((x) => x.n);
  return holes.length ? withRead(rs, holes) : rs;
}
