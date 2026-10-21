/* The two ways to move: the manual ship (WASD + mouse) and the orbit rig that
   the autopilot and the camera around a selected thing use. Pure maths. */

import { Euler, Quaternion, Vector3 } from 'three';
import { QUASAR_R, R_BOUND } from './layout';

export const PITCH_MAX = 1.45;
export const CRUISE_MIN = 12;
export const CRUISE_MAX = 420;
export const CRUISE_DEFAULT = 95;
/** Inertia of the ship: the time to reach the wanted speed, seconds. */
export const INERTIA = 0.1;

export interface ShipState {
  pos: Vector3;
  vel: Vector3;
  yaw: number;
  pitch: number;
  roll: number;
  cruise: number;
}

export interface ShipInput {
  forward: number;
  strafe: number;
  lift: number;
  boost: boolean;
  /** Mouse movement since the last step, in pixels. */
  mx: number;
  my: number;
}

export interface ShipOptions {
  sensitivity: number;
  invertY: boolean;
  /** Radius of the boundary; the ship is turned back inside it. */
  bound?: number;
}

export function newShip(): ShipState {
  return { pos: new Vector3(0, 300, 600), vel: new Vector3(), yaw: 0, pitch: -0.4, roll: 0, cruise: CRUISE_DEFAULT };
}

const _fwd = new Vector3();
const _right = new Vector3();
const _want = new Vector3();
const _up = new Vector3(0, 1, 0);

export function forwardOf(yaw: number, pitch: number, out = new Vector3()): Vector3 {
  const cp = Math.cos(pitch);
  return out.set(-Math.sin(yaw) * cp, Math.sin(pitch), -Math.cos(yaw) * cp);
}

export function rightOf(yaw: number, out = new Vector3()): Vector3 {
  return out.set(Math.cos(yaw), 0, -Math.sin(yaw));
}

export function shipQuaternion(s: Pick<ShipState, 'yaw' | 'pitch' | 'roll'>, out = new Quaternion()): Quaternion {
  return out.setFromEuler(new Euler(s.pitch, s.yaw, s.roll, 'YXZ'));
}

export interface StepResult {
  /** The ship is in the soft zone at the edge of the Galaxy. */
  edge: boolean;
  speed: number;
}

/** One step of the manual flight model. Response to the keys is immediate; inertia only smooths it. */
export function stepShip(s: ShipState, input: ShipInput, dt: number, opts: ShipOptions): StepResult {
  dt = Math.min(dt, 0.1);
  const k = 0.0022 * opts.sensitivity;
  s.yaw -= input.mx * k;
  s.pitch -= input.my * k * (opts.invertY ? -1 : 1);
  s.pitch = Math.max(-PITCH_MAX, Math.min(PITCH_MAX, s.pitch));

  forwardOf(s.yaw, s.pitch, _fwd);
  rightOf(s.yaw, _right);
  _want.set(0, 0, 0)
    .addScaledVector(_fwd, input.forward)
    .addScaledVector(_right, input.strafe)
    .addScaledVector(_up, input.lift);
  if (_want.lengthSq() > 1) _want.normalize();
  _want.multiplyScalar(s.cruise * (input.boost ? 2.4 : 1));

  // First-order smoothing toward the wanted velocity (time constant INERTIA).
  const a = 1 - Math.exp(-dt / INERTIA);
  s.vel.lerp(_want, a);
  s.pos.addScaledVector(s.vel, dt);

  // Auto-level: the roll follows the sideways movement and returns to the horizon.
  const bank = -input.strafe * 0.28 - Math.max(-0.4, Math.min(0.4, input.mx * 0.004)) * 0.6;
  s.roll += (bank - s.roll) * (1 - Math.exp(-dt / 0.18));

  const bound = opts.bound ?? R_BOUND;
  let edge = false;
  const r = s.pos.length();
  const soft = bound * 0.9;
  if (r > soft) {
    edge = true;
    // The further out, the harder the push back toward the centre.
    const over = Math.min(1, (r - soft) / (bound - soft));
    s.vel.addScaledVector(s.pos, (-over * over * 900 * dt) / Math.max(1, r));
  }
  if (r > bound) {
    s.pos.multiplyScalar(bound / r);
    const out = s.vel.dot(s.pos) / Math.max(1, s.pos.length());
    if (out > 0) s.vel.addScaledVector(s.pos, -out / Math.max(1, s.pos.length()));
  }
  // Not through the Quasar.
  const q = QUASAR_R + 5;
  const rr = s.pos.length();
  if (rr < q) {
    if (rr < 0.001) s.pos.set(0, 0, q);
    else s.pos.multiplyScalar(q / rr);
  }
  return { edge, speed: s.vel.length() };
}

/** Cruise speed steps with the wheel: ×1.18 per notch, within limits. */
export function wheelCruise(cruise: number, deltaY: number): number {
  const steps = Math.max(-4, Math.min(4, -Math.sign(deltaY) * Math.max(1, Math.round(Math.abs(deltaY) / 100))));
  return Math.max(CRUISE_MIN, Math.min(CRUISE_MAX, cruise * Math.pow(1.18, steps)));
}

/* ───────── orbit rig (autopilot and selected things) ───────── */

export interface RigState {
  focus: Vector3;
  yaw: number;
  pitch: number;
  dist: number;
}

export function newRig(): RigState {
  return { focus: new Vector3(), yaw: 0.35, pitch: 0.62, dist: 900 };
}

export function rigPosition(r: RigState, out = new Vector3()): Vector3 {
  const cp = Math.cos(r.pitch);
  return out.set(
    r.focus.x + Math.sin(r.yaw) * cp * r.dist,
    r.focus.y + Math.sin(r.pitch) * r.dist,
    r.focus.z + Math.cos(r.yaw) * cp * r.dist,
  );
}

/** The rig that looks at `focus` from where the camera now is. */
export function rigFromCamera(pos: Vector3, focus: Vector3): RigState {
  const off = pos.clone().sub(focus);
  const dist = Math.max(1, off.length());
  return { focus: focus.clone(), yaw: Math.atan2(off.x, off.z), pitch: Math.asin(Math.max(-1, Math.min(1, off.y / dist))), dist };
}

/** The ship state that looks at `look` from `pos`. */
export function shipFromCamera(pos: Vector3, look: Vector3, cruise = CRUISE_DEFAULT): ShipState {
  const d = look.clone().sub(pos);
  const len = Math.max(0.0001, d.length());
  const pitch = Math.asin(Math.max(-1, Math.min(1, d.y / len)));
  const yaw = Math.atan2(-d.x, -d.z);
  return { pos: pos.clone(), vel: new Vector3(), yaw, pitch, roll: 0, cruise };
}

export const clampPitch = (p: number, lo = 0.08, hi = 1.3): number => Math.max(lo, Math.min(hi, p));
export const ease = (t: number): number => 1 - Math.pow(1 - t, 3);

/** A warp flight takes longer for longer distances, within limits (seconds). */
export function flightDuration(distance: number, far = false): number {
  if (far) return 2.2;
  return Math.max(1.1, Math.min(2.6, 0.9 + distance / 700));
}

/** Where the crosshair points: the entity with the smallest angle to the ship's heading, within `cone` radians. */
export function aimedAt<T extends { pos: Vector3; radius: number }>(
  origin: Vector3, dir: Vector3, things: T[], cone = 0.06,
): T | null {
  let best: T | null = null;
  let bestScore = Infinity;
  const v = new Vector3();
  for (const t of things) {
    v.copy(t.pos).sub(origin);
    const d = v.length();
    if (d < 0.5) continue;
    const cos = v.dot(dir) / d;
    if (cos <= 0) continue;
    const ang = Math.acos(Math.min(1, cos));
    const size = Math.atan2(t.radius, d);
    if (ang - size > cone) continue;
    const score = ang + d * 1e-5;
    if (score < bestScore) { bestScore = score; best = t; }
  }
  return best;
}
