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

/** The forms of the subscription row, widest first: every provider with the two tightest windows (`full`), with the tightest one
 *  (`one`), the first `words` providers with their words and the rest as a share (`mix`), every provider as a share with a small gauge
 *  (`pct`), only the provider closest to its limit and only its share (`worst`), or nothing. */
export type RowForm = 'full' | 'one' | 'mix' | 'pct' | 'worst' | 'none';

export interface RowStep { form: Exclude<RowForm, 'worst' | 'none'>; words: number; w: number }

/** The top bar's monitoring group (the bars, then the subscription row) in the room (rem) the menu and the crumbs leave. The row has
 *  the first claim on it: the widest of the `steps` (given widest first, each with its width) that still leaves the bars at least their
 *  summary; else the one provider closest to its limit while there is room at all (`worst`, of width `worstW`); else nothing. `gap` is
 *  the space between the two, and `idx` the place of the step chosen (the steps' count for `worst`, one more for none). */
export function rowFit(room: number, steps: RowStep[], worstW: number, gap: number): { chip: RowForm; words: number; tier: ReturnType<typeof tierFor>; idx: number } {
  const left = (x: number): number => room - x - gap;
  for (let i = 0; i < steps.length; i++) {
    if (left(steps[i].w) >= 5.6) return { chip: steps[i].form, words: steps[i].words, tier: tierFor(left(steps[i].w)), idx: i };
  }
  if (room >= worstW) return { chip: 'worst', words: 0, tier: tierFor(left(worstW)), idx: steps.length };
  return { chip: 'none', words: 0, tier: 'none', idx: steps.length + 1 };
}
