import { describe, expect, it } from 'vitest';
import { sparkPath, sparkStats, sparkTop } from './Spark';

const p = (at: number, v: number | null): { at: number; v: number | null } => ({ at, v });

describe('the sparkline', () => {
  it('draws the window left to right and the values bottom to top', () => {
    const d = sparkPath([p(0, 0), p(1000, 100)], 0, 1000);
    expect(d).toBe('M0.00 96.00 L100.00 4.00');
  });

  it('keeps an unknown value as a hole, not a line through it', () => {
    const d = sparkPath([p(0, 10), p(500, null), p(1000, 20)], 0, 1000);
    expect(d.match(/M/g)?.length).toBe(2);
    expect(d).not.toContain('L0.00 96.00 L100.00');
  });

  it('gives a full line to a load over 100 % and keeps the hundred below the top', () => {
    const d = sparkPath([p(0, 0), p(1000, 200)], 0, 1000, 200);
    expect(d).toBe('M0.00 96.00 L100.00 4.00');
    expect(sparkPath([p(0, 100), p(1000, 0)], 0, 1000, 200)).toBe('M0.00 50.00 L100.00 96.00');
    expect(sparkTop([p(0, 40), p(10, 130)])).toBe(130);
    expect(sparkTop([p(0, 40)])).toBe(100);
  });

  it('counts only the samples of the window for the peak beside the line', () => {
    const s = sparkStats([p(0, 90), p(5000, 20), p(6000, null), p(7000, 40)], 5000);
    expect(s.shown.map((x) => x.at)).toEqual([5000, 6000, 7000]);
    expect(s.peak).toBe(40);
  });
});
