import { describe, expect, it } from 'vitest';
import { dayKey, groupByDate, isToday } from './dates';

const at = (y: number, m: number, d: number, h = 12): number => new Date(y, m - 1, d, h).getTime();

describe('date groups', () => {
  const now = at(2026, 10, 20, 15);
  const items = [
    { id: 'a', created: at(2026, 10, 20, 9) },
    { id: 'b', created: at(2026, 10, 19, 22) },
    { id: 'c', created: at(2026, 10, 14, 8) },
    { id: 'd', created: at(2026, 10, 20, 14) },
    { id: 'e', created: at(2026, 10, 19, 7) },
    { id: 'u', created: 0 },
  ];
  it('puts today first, then dd.mm.yyyy newest on top', () => {
    const g = groupByDate(items, (x) => x.created, now);
    expect(g.map((x) => x.key)).toEqual(['today', '19.10.2026', '14.10.2026', '—']);
    expect(g[0].today).toBe(true);
    expect(g[0].items.map((x) => x.id)).toEqual(['d', 'a']);
    expect(g[1].items.map((x) => x.id)).toEqual(['b', 'e']);
  });
  it('formats days as dd.mm.yyyy', () => {
    expect(dayKey(at(2026, 3, 5))).toBe('05.03.2026');
    expect(isToday(at(2026, 10, 20, 1), now)).toBe(true);
    expect(isToday(at(2026, 10, 19, 23), now)).toBe(false);
  });
});
