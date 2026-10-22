import { describe, expect, it } from 'vitest';
import { settled, unreadOf, withRead } from './chat-read';
import type { TurnData } from './types';

const turn = (n: number, a = `answer ${n}`): TurnData => ({ n, at: 0, from: 'human', q: `q${n}`, a });

describe('unreadOf', () => {
  it('has nothing before the first look', () => {
    expect(unreadOf(null, [turn(0)], 1)).toEqual([]);
  });
  it('lists the answered turns above the place, loaded or not', () => {
    expect(unreadOf({ base: 4, seen: [] }, [turn(7), turn(8), turn(9)], 10)).toEqual([5, 6, 7, 8, 9]);
  });
  it('skips the read ones and a turn that is still working', () => {
    expect(unreadOf({ base: 4, seen: [6] }, [turn(5), turn(6), turn(7, '')], 8)).toEqual([5]);
  });
});

describe('withRead', () => {
  it('folds the read turns into the place when they touch it', () => {
    expect(withRead({ base: 4, seen: [] }, [5])).toEqual({ base: 5, seen: [] });
    expect(withRead({ base: 4, seen: [] }, [7])).toEqual({ base: 4, seen: [7] });
    expect(withRead({ base: 4, seen: [7] }, [5, 6])).toEqual({ base: 7, seen: [] });
  });
  it('ignores turns below the place', () => {
    expect(withRead({ base: 4, seen: [] }, [2])).toEqual({ base: 4, seen: [] });
  });
});

describe('settled', () => {
  it('takes what is there at the first look for read', () => {
    expect(settled(null, [turn(8), turn(9)], 10)).toEqual({ base: 9, seen: [] });
  });
  it('leaves a working last turn unread-to-be: its answer is news', () => {
    expect(settled(null, [turn(8), turn(9, '')], 10)).toEqual({ base: 8, seen: [] });
  });
  it('keeps a stored place, also for a transcript caught half-written', () => {
    const rs = { base: 9, seen: [] };
    expect(settled(rs, [], 10)).toBe(rs);
    expect(settled(rs, [], 9)).toBe(rs);
  });
  it('counts a finished turn without an answer as read, so it does not hold the place back', () => {
    expect(settled({ base: 8, seen: [10] }, [turn(9, ''), turn(10), turn(11)], 12)).toEqual({ base: 10, seen: [] });
    // the last one may still be working: its answer is news
    const rs = { base: 8, seen: [] };
    expect(settled(rs, [turn(9, '')], 10)).toBe(rs);
  });
  it('puts the place back at the end of a much shorter conversation', () => {
    expect(settled({ base: 40, seen: [3, 41] }, [], 5)).toEqual({ base: 4, seen: [3] });
  });
});
