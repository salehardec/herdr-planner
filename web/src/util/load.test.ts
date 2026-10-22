import { describe, expect, it } from 'vitest';
import { gb, levelOf, meters, pct, staleLoad, tierFor, worse, worstMeter } from './load';

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
    expect([32, 24, 14, 8, 3].map(tierFor)).toEqual(['host', 'full', 'stack', 'sum', 'none']);
  });
  it('knows an old reading', () => {
    expect(staleLoad(0, 3000, 5000)).toBe(true);
    expect(staleLoad(1000, 3000, 9000)).toBe(false);
    expect(staleLoad(1000, 3000, 12000)).toBe(true);
  });
});
