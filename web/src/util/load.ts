/* The bars of the machine's load: where the colour changes, and how a number is told. */

/** From this share on a load is «high» (yellow), and from the next one «critical» (red). */
export const LOAD_HIGH = 70;
export const LOAD_CRIT = 90;

export type LoadLevel = 'ok' | 'high' | 'crit';

export const levelOf = (percent: number): LoadLevel => (percent >= LOAD_CRIT ? 'crit' : percent >= LOAD_HIGH ? 'high' : 'ok');

const RANK: Record<LoadLevel, number> = { ok: 0, high: 1, crit: 2 };
export const worse = (a: LoadLevel, b: LoadLevel): LoadLevel => (RANK[b] > RANK[a] ? b : a);

/** A share shown as a whole number between 0 and 100. */
export const pct = (v: number): number => Math.max(0, Math.min(100, Math.round(Number.isFinite(v) ? v : 0)));

/** Bytes as gigabytes with one decimal, in the language's own separator («14,2» / «14.2»). */
export const gb = (bytes: number, locale: string): string =>
  (Math.max(0, bytes) / 1073741824).toLocaleString(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** The reading is too old to be the machine's present state. Its age is the machine's own (`sent` − `at`, one clock), and
 *  a stream that went quiet is told by the time the last reading reached the browser (`recvAgo`, the browser's one clock):
 *  the two clocks are never compared with each other. */
export const staleLoad = (at: number, sent: number, recvAgo: number, everyMs: number): boolean => {
  const limit = Math.max(3 * (everyMs || 3000), 10000);
  return at <= 0 || (sent > 0 && sent - at > limit) || recvAgo > limit;
};

export interface Meter { key: 'cpu' | 'mem' | 'gpu'; percent: number; level: LoadLevel }

/** The meters of a reading in the order CPU, MEM, GPU; GPU only when the machine has a card (its share is the mean of the cards). */
export function meters(l: { cpu: { percent: number }; mem: { percent: number }; gpus: { util: number }[] }): Meter[] {
  const m = (key: Meter['key'], v: number): Meter => ({ key, percent: pct(v), level: levelOf(pct(v)) });
  const out = [m('cpu', l.cpu.percent), m('mem', l.mem.percent)];
  if (l.gpus.length) out.push(m('gpu', l.gpus.reduce((a, g) => a + g.util, 0) / l.gpus.length));
  return out;
}

/** The meter that is worst (the first of equals): what the narrowest form of the bar shows. */
export const worstMeter = (ms: Meter[]): Meter => ms.reduce((a, b) => (RANK[b.level] > RANK[a.level] || (b.level === a.level && b.percent > a.percent) ? b : a));

/** The form of the bars for the room (in rem) left beside the crumbs: with the machine's name, three bars, three small bars under their
 *  numbers, the worst one only (named, or just the number when the room is very small), or nothing. */
export const tierFor = (rem: number): 'host' | 'full' | 'stack' | 'sum' | 'dot' | 'none' =>
  rem >= 31 ? 'host' : rem >= 22 ? 'full' : rem >= 12.5 ? 'stack' : rem >= 5.6 ? 'sum' : rem >= 3.8 ? 'dot' : 'none';

/** The top bar's monitoring group (the bars, then the subscription chip) in the room (rem) the menu and the crumbs leave: the chip keeps its
 *  words (the agent and the window) while the bars still get the full form, else it shrinks to its percentage, and with no room at all
 *  it steps aside. `gap` is the space between the two. */
export function monFit(room: number, chipFull: number, chipPct: number, gap: number): { chip: 'full' | 'pct' | 'none'; tier: ReturnType<typeof tierFor> } {
  if (room - chipFull - gap >= 12.5) return { chip: 'full', tier: tierFor(room - chipFull - gap) };
  if (room >= chipPct) return { chip: 'pct', tier: tierFor(room - chipPct - gap) };
  return { chip: 'none', tier: 'none' };
}
