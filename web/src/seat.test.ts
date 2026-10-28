import { describe, expect, it } from 'vitest';
import { seatLabel, seatName, seatSpec, seatTip } from './seat';
import type { Seat } from './types';

const seat = (over: Partial<Seat>): Seat => ({ role: 'reviewer', program: 'pi', model: 'kimi-coding/k3.1-pro', took_over: false, ...over });

describe('who is on a task', () => {
  it('names the reviewer by program and the end of its model', () => {
    expect(seatName(seat({}))).toBe('pi/k3.1-pro');
    expect(seatLabel(seat({}))).toBe('ревью · pi/k3.1-pro');
  });

  it('keeps the whole agent for the tooltip', () => {
    expect(seatSpec(seat({}))).toBe('pi/kimi-coding/k3.1-pro');
    expect(seatTip(seat({}))).toContain('Ревьюер: pi/kimi-coding/k3.1-pro');
  });

  it('says a reviewer that took over from the first one is the fallback', () => {
    const s = seat({ program: 'codex', model: '', took_over: true });
    expect(seatLabel(s)).toBe('ревью · codex');
    expect(seatTip(s)).toContain('передано запасному');
  });

  it('shows the working agent and says honestly when nobody is named', () => {
    expect(seatLabel(seat({ role: 'worker', program: 'omp', model: 'deepseek/deepseek-v4-flash' }))).toBe('omp/deepseek-v4-flash');
    expect(seatLabel(seat({ program: '', model: '' }))).toBe('ревью · не назван');
  });
});
