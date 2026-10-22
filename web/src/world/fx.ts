/* What a planet's state looks like, as numbers. One function per frame and planet, no allocation:
   the world reads the result into the instance buffers. Each state has its own kind of signal, so
   colour is never the only difference (it helps with colour blindness and in a crowded sky):
     work   — the agent or the reviewer is running: a slow breath of the planet and a ripple that
              leaves it (period ≈ 2.2 s), and the program's marker circles on its ring;
     ask    — it waits for the human: a sharp double flash with a ring around the planet (≈ 1.6 s);
     fail   — a steady cage of two rings, one short alarm flash every ≈ 7 s;
     sleep  — queued: dim and still, no ring;
     done   — dimmest, no ring.
   Without motion (simple mode, prefers-reduced-motion) the same shapes stay, frozen at a bright point. */
import type { Status } from '../types';

export type FxKind = 'work' | 'ask' | 'fail' | 'sleep' | 'done';

export const WORK_PERIOD = 2.2;
export const ASK_PERIOD = 1.6;
export const ALARM_PERIOD = 7;

export const kindOf = (s: Status): FxKind => {
  switch (s) {
    case 'working': case 'review': return 'work';
    case 'waiting': case 'ready': case 'plan': return 'ask';
    case 'failed': return 'fail';
    case 'queued': return 'sleep';
    default: return 'done';
  }
};

export interface Fx {
  kind: FxKind;
  /** Brightness added to the planet (0…1). */
  glow: number;
  /** Size factor of the planet. */
  breathe: number;
  /** Radius (in planet radii) and strength of the ripple; 0 strength: no ripple. */
  auraR: number;
  aura: number;
  /** Strength of the steady ring around the planet (0: none, 1: full). */
  halo: number;
  /** Opacity of the planet itself. */
  dim: number;
  /** Does it change with time (false when motion is off, and for the states that never move). */
  moving: boolean;
}

const bump = (u: number, c: number, w: number): number => Math.max(0, 1 - Math.abs(u - c) / w);
const frac = (x: number): number => x - Math.floor(x);

/** `time` in seconds, `phase` in 0…2π (each planet its own, so a sky of them does not blink as one). */
export function stateFx(status: Status, time: number, phase: number, moving: boolean, out: Fx): Fx {
  const kind = kindOf(status);
  out.kind = kind;
  out.glow = 0; out.breathe = 1; out.auraR = 1; out.aura = 0; out.halo = 0; out.dim = 1; out.moving = false;
  const turn = phase / (Math.PI * 2);
  switch (kind) {
    case 'work': {
      if (moving) {
        const u = frac(time / WORK_PERIOD + turn);
        const breath = 0.5 - 0.5 * Math.cos(u * Math.PI * 2);
        out.glow = 0.1 + 0.5 * breath;
        out.breathe = 1 + 0.06 * breath;
        out.auraR = 1.2 + 0.9 * u;
        out.aura = (1 - u) * Math.min(1, u * 8) * 0.85;
        out.moving = true;
      } else {
        out.glow = 0.45; out.auraR = 1.5; out.aura = 0.7;
      }
      break;
    }
    case 'ask': {
      if (moving) {
        const u = frac(time / ASK_PERIOD + turn);
        const flash = Math.max(bump(u, 0.1, 0.09), bump(u, 0.3, 0.09));
        out.glow = 0.16 + 0.8 * flash;
        out.halo = 0.55 + 0.45 * flash;
        out.moving = true;
      } else {
        out.glow = 0.5; out.halo = 1;
      }
      break;
    }
    case 'fail': {
      if (moving) {
        const u = frac(time / ALARM_PERIOD + turn);
        const flash = u < 0.07 ? 1 - u / 0.07 : 0;
        out.glow = 0.28 + 0.72 * flash;
        out.halo = 0.8 + 0.2 * flash;
        out.moving = true;
      } else {
        out.glow = 0.4; out.halo = 1;
      }
      break;
    }
    case 'sleep': out.dim = 0.7; break;
    default: out.dim = 0.5; break;
  }
  return out;
}

export const newFx = (): Fx => ({ kind: 'done', glow: 0, breathe: 1, auraR: 1, aura: 0, halo: 0, dim: 1, moving: false });
