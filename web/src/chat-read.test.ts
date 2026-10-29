import { describe, expect, it } from 'vitest';
import { readToEnd, settled, sigOf, unreadOf, withRead } from './chat-read';
import type { TurnData } from './types';

const turn = (n: number, a = `answer ${n}`, from: TurnData['from'] = 'human'): TurnData => ({ n, at: 0, from, q: `q${n}`, a });
const read = (n: number, a = `answer ${n}`): [number, number] => [n, sigOf(a)];

describe('unreadOf', () => {
  it('has nothing before the first look', () => {
    expect(unreadOf(null, [turn(0)], 1)).toEqual([]);
  });
  it('lists only loaded answered turns, not history that was reloaded in a partial page', () => {
    expect(unreadOf({ base: 4, seen: {} }, [turn(7), turn(8), turn(9)], 10)).toEqual([7, 8, 9]);
  });
  it('skips the turns read as they are and a turn that is still working', () => {
    expect(unreadOf({ base: 4, seen: { 6: sigOf('answer 6') } }, [turn(5), turn(6), turn(7, '')], 8)).toEqual([5]);
  });
  it('lights a turn again when its words changed after they were read (an interim line, then the answer)', () => {
    const rs = { base: 4, seen: { 5: sigOf('Сейчас посмотрю доску.') } };
    expect(unreadOf(rs, [turn(5, 'Сейчас посмотрю доску.')], 6)).toEqual([]);
    expect(unreadOf(rs, [turn(5, 'alpha/4 на ревью.')], 6)).toEqual([5]);
  });
  it('does not count a service notice of the board', () => {
    expect(unreadOf({ base: 4, seen: {} }, [turn(5, '', 'board')], 6)).toEqual([]);
    expect(unreadOf({ base: 4, seen: { 5: 0 } }, [turn(5, '', 'board')], 6)).toEqual([]);
  });
});

describe('withRead', () => {
  it('keeps the last turn apart: its words may still change', () => {
    expect(withRead({ base: 4, seen: {} }, [read(5)], 6, [turn(5)])).toEqual({ base: 4, seen: { 5: sigOf('answer 5') } });
  });
  it('folds the read turns into the place once they are not the last', () => {
    expect(withRead({ base: 4, seen: { 5: sigOf('answer 5') } }, [read(6)], 8, [turn(5), turn(6)])).toEqual({ base: 6, seen: {} });
    expect(withRead({ base: 4, seen: {} }, [read(7)], 9, [turn(7)])).toEqual({ base: 4, seen: { 7: sigOf('answer 7') } });
  });
  it('ignores turns below the place', () => {
    expect(withRead({ base: 4, seen: {} }, [read(2)], 9, [])).toEqual({ base: 4, seen: {} });
  });
});

describe('readToEnd', () => {
  it('clears a scrolled-up queue at the bottom, keeping the latest answer watched for updates', () => {
    const turns = [turn(7), turn(8), turn(9)];
    const rs = readToEnd(turns, 10);
    expect(rs).toEqual({ base: 8, seen: { 9: sigOf('answer 9') } });
    expect(unreadOf(rs, turns, 10)).toEqual([]);
    expect(unreadOf(rs, [...turns, turn(10)], 11)).toEqual([10]);
    expect(unreadOf(rs, [...turns.slice(0, 2), turn(9, 'updated answer')], 10)).toEqual([9]);
  });
});

describe('settled', () => {
  it('takes what is there at the first look for read, the last turn as it stands', () => {
    expect(settled(null, [turn(8), turn(9)], 10)).toEqual({ base: 8, seen: { 9: sigOf('answer 9') } });
  });
  it('a last turn that is still working: its answer is news', () => {
    const rs = settled(null, [turn(8), turn(9, '')], 10);
    expect(rs).toEqual({ base: 8, seen: { 9: 0 } });
    expect(unreadOf(rs, [turn(9, 'done')], 10)).toEqual([9]);
  });
  it('keeps a stored place, also for a transcript caught half-written', () => {
    const rs = { base: 9, seen: {} };
    expect(settled(rs, [], 10)).toBe(rs);
    expect(settled(rs, [], 9)).toBe(rs);
  });
  it('does not revive a read tail after reconnecting or loading the same history again', () => {
    const turns = [turn(7), turn(8), turn(9)];
    const rs = readToEnd(turns, 10);
    expect(settled(rs, turns, 10)).toBe(rs);
    expect(unreadOf(rs, turns, 10)).toEqual([]);
  });
  it('folds a finished non-message turn, including a notice of the board, so it does not hold the place back', () => {
    expect(settled({ base: 8, seen: { 10: sigOf('answer 10') } }, [turn(9, ''), turn(10), turn(11)], 12)).toEqual({ base: 10, seen: {} });
    const rs = { base: 8, seen: {} };
    expect(settled(rs, [turn(9, '')], 10)).toBe(rs);
    expect(settled(rs, [turn(9, '', 'board'), turn(10)], 11).base).toBe(9);
  });
  it('does not fold a turn whose words changed after they were read, when a later turn appears', () => {
    const rs = { base: 4, seen: { 5: sigOf('Сейчас посмотрю доску.') } };
    const turns = [turn(5, 'beta/2 готова.'), turn(6, '', 'board')];
    const next = settled(rs, turns, 7);
    expect(next.base).toBe(4);
    expect(unreadOf(next, turns, 7)).toEqual([5]);
    expect(withRead(next, [read(6, '')], 8, [...turns, turn(7)]).base).toBe(4);
  });
  it('does not fold a read turn that is not loaded (a reloaded page holds the last few): it is checked once loaded', () => {
    const rs = { base: 4, seen: { 5: sigOf('Сейчас посмотрю доску.') } };
    const tail = [turn(6, '', 'board'), turn(7, '', 'board'), turn(8, '', 'board')];
    const reloaded = settled(rs, tail, 9);
    expect(reloaded.base).toBe(4);
    expect(reloaded.seen[5]).toBe(sigOf('Сейчас посмотрю доску.'));
    const loaded = settled(reloaded, [turn(5, 'beta/2 готова.'), ...tail], 9);
    expect(unreadOf(loaded, [turn(5, 'beta/2 готова.'), ...tail], 9)).toEqual([5]);
  });
  it('folds the turn that is no longer the last', () => {
    expect(settled({ base: 8, seen: { 9: sigOf('answer 9') } }, [turn(9), turn(10)], 11)).toEqual({ base: 9, seen: {} });
  });
  it('puts the place back at the end of a much shorter conversation', () => {
    expect(settled({ base: 40, seen: { 3: 1, 41: 1 } }, [], 5)).toEqual({ base: 4, seen: { 3: 1 } });
  });
});
