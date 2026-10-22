/* Which turns of the Coordinator's chat are unread: the pure part (the state in the browser is in chat-unread.ts).
   A turn's answer is the Coordinator's latest words of it, and they change while it works («Сейчас посмотрю…», then the real
   answer): what was read is the words themselves, so a read turn whose answer has changed is news again. */
import type { TurnData } from './types';

/** Every turn up to `base` is read; `seen` are the ones above it that were read, each with the signature of the words that were read. */
export interface ReadState { base: number; seen: Record<number, number> }

/** A short fingerprint of an answer; 0 for no words. */
export function sigOf(a: string): number {
  let h = 0;
  for (let i = 0; i < a.length; i++) h = (Math.imul(h, 31) + a.charCodeAt(i)) | 0;
  return h === 0 && a ? 1 : h;
}

/** Does the turn bring something to read: words of the Coordinator's, or a notice of the board (which is shown by itself). */
const isNews = (x: TurnData): boolean => !!x.a || x.from === 'board';

/** The turns above `base` whose words (as they are now) have not been read. A turn that is not loaded is taken for answered. */
export function unreadOf(rs: ReadState | null, turns: TurnData[], total: number): number[] {
  if (!rs) return [];
  const have = new Map(turns.map((x) => [x.n, x]));
  const out: number[] = [];
  for (let n = rs.base + 1; n < total; n++) {
    const x = have.get(n);
    if (!x) { if (!(n in rs.seen)) out.push(n); continue; }
    if (isNews(x) && rs.seen[n] !== sigOf(x.a)) out.push(n);
  }
  return out;
}

/** Read turns that are not the last one fold into `base`: the last may still be working, its words may change. */
function fold(rs: ReadState, total: number): ReadState {
  const seen = { ...rs.seen };
  let base = rs.base;
  while (base + 1 < total - 1 && (base + 1) in seen) { base++; delete seen[base]; }
  return { base, seen };
}

/** The state after these turns were read (turn, signature of the words that were in view). */
export function withRead(rs: ReadState, marks: [number, number][], total: number): ReadState {
  const seen = { ...rs.seen };
  for (const [n, sig] of marks) if (n > rs.base) seen[n] = sig;
  return fold({ base: rs.base, seen }, total);
}

/**
 * The first look at a chat (nothing stored): what is there now counts as read, so a page that has never been told about the chat
 * does not open with a hundred lights. A transcript that got much shorter than the stored place is a new conversation: the place goes back to its end.
 */
export function settled(rs: ReadState | null, turns: TurnData[], total: number): ReadState {
  const last = total - 1;
  const tail = turns.find((x) => x.n === last);
  if (!rs) {
    // The last turn may still be working: it is read as it stands, and what it says next is news.
    return tail ? { base: last - 1, seen: { [last]: sigOf(tail.a) } } : { base: last, seen: {} };
  }
  // (One turn fewer is a transcript caught half-written, not a new conversation.)
  if (rs.base > last + 1) {
    const seen: Record<number, number> = {};
    for (const [n, sig] of Object.entries(rs.seen)) if (Number(n) <= last) seen[Number(n)] = sig;
    return { base: last, seen };
  }
  // A finished turn without words (and not a notice of the board) will never bring news: it is read, so it does not hold the place back.
  const holes = turns.filter((x) => !isNews(x) && x.n > rs.base && x.n < last && !(x.n in rs.seen)).map((x): [number, number] => [x.n, 0]);
  const next = fold({ base: rs.base, seen: { ...rs.seen, ...Object.fromEntries(holes) } }, total);
  return holes.length || next.base !== rs.base ? next : rs;
}
