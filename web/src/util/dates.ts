/* Dates in the navigator and elsewhere: «today», then dd.mm.yyyy, newest first. */

const pad = (n: number): string => String(n).padStart(2, '0');

export function dayKey(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()}`;
}

export function isToday(ms: number, now = Date.now()): boolean {
  return dayKey(ms) === dayKey(now);
}

/** HH:MM */
export function timeOf(ms: number): string {
  if (!ms) return '';
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** HH:MM:SS: for what is refreshed while it is read (a terminal screen). */
export function clockOf(ms: number): string {
  if (!ms) return '';
  return `${timeOf(ms)}:${pad(new Date(ms).getSeconds())}`;
}

/** HH:MM today, otherwise dd.mm, HH:MM */
export function dayTimeOf(ms: number, now = Date.now()): string {
  if (!ms) return '';
  const d = new Date(ms);
  return isToday(ms, now) ? timeOf(ms) : `${pad(d.getDate())}.${pad(d.getMonth() + 1)}, ${timeOf(ms)}`;
}

export interface DateGroup<T> {
  /** `today` or `dd.mm.yyyy` */
  key: string;
  today: boolean;
  items: T[];
}

/** Groups by the day of `created`: today first, then the other days, newest first. Unknown dates go last. */
export function groupByDate<T>(items: T[], created: (x: T) => number, now = Date.now()): DateGroup<T>[] {
  const map = new Map<string, { ts: number; items: T[] }>();
  for (const it of items) {
    const ms = created(it);
    const key = ms > 0 ? (isToday(ms, now) ? 'today' : dayKey(ms)) : '—';
    const g = map.get(key) ?? { ts: 0, items: [] };
    g.items.push(it);
    // The newest task decides the group's place.
    g.ts = Math.max(g.ts, ms);
    map.set(key, g);
  }
  const out = [...map.entries()].map(([key, g]) => ({ key, today: key === 'today', ts: g.ts, items: g.items }));
  out.sort((a, b) => (a.today ? 1 : 0) !== (b.today ? 1 : 0) ? (a.today ? -1 : 1) : (b.ts || -1) - (a.ts || -1));
  for (const g of out) g.items.sort((x, y) => created(y) - created(x));
  return out.map(({ key, today, items: its }) => ({ key, today, items: its }));
}
