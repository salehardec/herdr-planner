import { describe, expect, it } from 'vitest';
import { gb, levelOf, meters, monFit, pct, staleLoad, tierFor, worse, worstMeter } from './load';

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
  it('gives the subscription chip its words only while the bars keep at least the stack form', () => {
    expect(monFit(40, 13, 3.6, 0.5)).toEqual({ chip: 'full', tier: 'full' });
    expect(monFit(50, 13, 3.6, 0.5)).toEqual({ chip: 'full', tier: 'host' });
    expect(monFit(28, 13, 3.6, 0.5)).toEqual({ chip: 'full', tier: 'stack' });
    expect(monFit(25, 13, 3.6, 0.5)).toEqual({ chip: 'pct', tier: 'stack' });
    expect(monFit(10, 13, 3.6, 0.5)).toEqual({ chip: 'pct', tier: 'sum' });
    expect(monFit(4, 13, 3.6, 0.5)).toEqual({ chip: 'pct', tier: 'none' });
    expect(monFit(3, 13, 3.6, 0.5)).toEqual({ chip: 'none', tier: 'none' });
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
