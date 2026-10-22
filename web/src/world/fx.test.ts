import { describe, expect, it } from 'vitest';
import type { Status } from '../types';
import { ALARM_PERIOD, ASK_PERIOD, WORK_PERIOD, kindOf, newFx, stateFx } from './fx';

const at = (s: Status, t: number, moving = true) => ({ ...stateFx(s, t, 1.3, moving, newFx()) });
const sample = (s: Status, period: number, moving = true) => Array.from({ length: 40 }, (_, i) => at(s, (i / 40) * period, moving));
const spread = (xs: number[]) => Math.max(...xs) - Math.min(...xs);

describe('planet states', () => {
  it('a working planet breathes and sends a ripple; a reviewer too', () => {
    for (const s of ['working', 'review'] as const) {
      const f = sample(s, WORK_PERIOD);
      expect(spread(f.map((x) => x.glow))).toBeGreaterThan(0.3);
      expect(spread(f.map((x) => x.auraR))).toBeGreaterThan(0.5);
      expect(Math.max(...f.map((x) => x.aura))).toBeGreaterThan(0.5);
      expect(f.every((x) => x.moving)).toBe(true);
    }
  });

  it('breathing takes about two seconds and is gentle', () => {
    expect(WORK_PERIOD).toBeGreaterThanOrEqual(1.5);
    expect(WORK_PERIOD).toBeLessThanOrEqual(2.5);
    const f = sample('working', WORK_PERIOD);
    expect(spread(f.map((x) => x.breathe))).toBeLessThan(0.1);
  });

  it('a queued planet does not move: dim, no glow, no ring', () => {
    const f = sample('queued', 10);
    expect(spread(f.map((x) => x.glow))).toBe(0);
    expect(f.every((x) => !x.moving && x.aura === 0 && x.halo === 0 && x.dim < 1)).toBe(true);
  });

  it('a done planet is dimmer than a queued one', () => {
    expect(at('done', 0).dim).toBeLessThan(at('queued', 0).dim);
    expect(at('done', 0).glow).toBe(0);
  });

  it('what waits for the human flashes with a ring, differently from work', () => {
    for (const s of ['waiting', 'ready', 'plan'] as const) {
      const f = sample(s, ASK_PERIOD);
      expect(spread(f.map((x) => x.glow))).toBeGreaterThan(0.5);
      expect(f.every((x) => x.halo > 0 && x.aura === 0)).toBe(true);
    }
    expect(kindOf('waiting')).not.toBe(kindOf('working'));
  });

  it('a failed planet has a steady ring and one rare flash', () => {
    const f = sample('failed', ALARM_PERIOD * 2);
    expect(f.every((x) => x.halo >= 0.8)).toBe(true);
    const bright = f.filter((x) => x.glow > 0.5).length;
    expect(bright).toBeGreaterThan(0);
    expect(bright).toBeLessThan(f.length / 4);
  });

  it('without motion every state stays still, and the signals stay', () => {
    const all: Status[] = ['working', 'review', 'waiting', 'ready', 'plan', 'failed', 'queued', 'done'];
    for (const s of all) {
      const a = at(s, 0.3, false);
      const b = at(s, 1.7, false);
      expect(a).toEqual(b);
      expect(a.moving).toBe(false);
    }
    expect(at('working', 0, false).aura).toBeGreaterThan(0.5);
    expect(at('working', 0, false).glow).toBeGreaterThan(0.3);
    expect(at('waiting', 0, false).halo).toBe(1);
    expect(at('failed', 0, false).halo).toBe(1);
  });

  it('planets are out of step with each other', () => {
    const a = stateFx('working', 0.5, 0, true, newFx()).glow;
    const b = stateFx('working', 0.5, 1.5, true, newFx()).glow;
    expect(Math.abs(a - b)).toBeGreaterThan(0.05);
  });
});
