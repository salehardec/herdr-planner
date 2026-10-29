import { describe, expect, it } from 'vitest';
import { gb, levelOf, meters, pct, rowFit, staleLoad, tierFor, worse, worstMeter } from './load';

describe('load levels', () => {
  it('changes colour at 70 and 90', () => {
    expect([0, 69.4, 70, 89, 90, 100].map(levelOf)).toEqual(['ok', 'ok', 'high', 'high', 'crit', 'crit']);
    expect(worse('ok', 'crit')).toBe('crit');
    expect(worse('crit', 'high')).toBe('crit');
  });
  it('keeps a share between 0 and 100', () => {
    expect([pct(-3), pct(41.6), pct(250), pct(NaN)]).toEqual([0, 42, 100, 0]);
  });
  it('tells gigabytes with the language\'s separator', () => {
    expect(gb(1.5 * 1073741824, 'en-GB')).toBe('1.5');
    expect(gb(1.5 * 1073741824, 'ru-RU')).toBe('1,5');
  });
  it('has a GPU meter only when there is a card, as the mean of the cards', () => {
    const base = { cpu: { percent: 20 }, mem: { percent: 75 } };
    expect(meters({ ...base, gpus: [] }).map((m) => m.key)).toEqual(['cpu', 'mem']);
    const m = meters({ ...base, gpus: [{ util: 10 }, { util: 100 }] });
    expect(m.map((x) => [x.key, x.percent, x.level])).toEqual([['cpu', 20, 'ok'], ['mem', 75, 'high'], ['gpu', 55, 'ok']]);
    expect(worstMeter(m).key).toBe('mem');
  });
  it('takes the form that fits the room the crumbs leave', () => {
    expect([32, 24, 14, 8, 4, 3].map(tierFor)).toEqual(['host', 'full', 'stack', 'sum', 'dot', 'none']);
  });
  it('gives the subscription row the first claim on the room, then shrinks it step by step', () => {
    const steps = [
      { form: 'full' as const, words: 4, w: 13 },
      { form: 'one' as const, words: 4, w: 9 },
      { form: 'mix' as const, words: 3, w: 8 },
      { form: 'mix' as const, words: 1, w: 6.5 },
      { form: 'pct' as const, words: 0, w: 5 },
    ];
    expect(rowFit(40, steps, 3.6, 0.5)).toEqual({ chip: 'full', words: 4, tier: 'full', idx: 0 });
    expect(rowFit(50, steps, 3.6, 0.5)).toEqual({ chip: 'full', words: 4, tier: 'host', idx: 0 });
    // the words give way first to the tightest window of each provider, while the bars keep at least their summary…
    expect(rowFit(19.1, steps, 3.6, 0.5)).toEqual({ chip: 'full', words: 4, tier: 'sum', idx: 0 });
    expect(rowFit(19.0, steps, 3.6, 0.5)).toEqual({ chip: 'one', words: 4, tier: 'sum', idx: 1 });
    // …then to fewer providers with their words, the rest as shares…
    expect(rowFit(14.2, steps, 3.6, 0.5)).toEqual({ chip: 'mix', words: 3, tier: 'sum', idx: 2 });
    expect(rowFit(12.7, steps, 3.6, 0.5)).toEqual({ chip: 'mix', words: 1, tier: 'sum', idx: 3 });
    // …then to shares only…
    expect(rowFit(11.2, steps, 3.6, 0.5)).toEqual({ chip: 'pct', words: 0, tier: 'sum', idx: 4 });
    // …then to the one provider closest to its limit, whatever is left for the bars, …
    expect(rowFit(10, steps, 3.6, 0.5)).toEqual({ chip: 'worst', words: 0, tier: 'sum', idx: 5 });
    expect(rowFit(4, steps, 3.6, 0.5)).toEqual({ chip: 'worst', words: 0, tier: 'none', idx: 5 });
    // …and at last to nothing
    expect(rowFit(3, steps, 3.6, 0.5)).toEqual({ chip: 'none', words: 0, tier: 'none', idx: 6 });
  });
  it('knows an old reading by the machine\'s own clock and by how long the stream was quiet', () => {
    expect(staleLoad(0, 5000, 0, 3000)).toBe(true);
    expect(staleLoad(1000, 4000, 100, 3000)).toBe(false);
    // the machine sent it 30 s after it was read: old, whatever the browser's clock says
    expect(staleLoad(1000, 31000, 100, 3000)).toBe(true);
    // fresh by the machine, but nothing has come for 20 s
    expect(staleLoad(1000, 2000, 20000, 3000)).toBe(true);
  });
});
