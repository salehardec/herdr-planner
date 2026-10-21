import { describe, expect, it } from 'vitest';
import { Vector3 } from 'three';
import { aimedAt, CRUISE_MAX, CRUISE_MIN, flightDuration, forwardOf, newShip, rigFromCamera, rigPosition, shipFromCamera, stepShip, wheelCruise } from './flight';
import { R_BOUND, QUASAR_R } from './layout';

const idle = { forward: 0, strafe: 0, lift: 0, boost: false, mx: 0, my: 0 };
const opts = { sensitivity: 1, invertY: false };

describe('manual flight', () => {
  it('answers a key at once and smooths only a little', () => {
    const s = newShip();
    s.pos.set(0, 0, 400);
    s.yaw = 0; s.pitch = 0;
    stepShip(s, { ...idle, forward: 1 }, 1 / 60, opts);
    expect(s.vel.length()).toBeGreaterThan(0);
    expect(s.pos.z).toBeLessThan(400);
    // After a few frames the speed is the cruise speed.
    for (let i = 0; i < 20; i++) stepShip(s, { ...idle, forward: 1 }, 1 / 60, opts);
    expect(s.vel.length()).toBeGreaterThan(s.cruise * 0.95);
  });

  it('stops soon after the key is released', () => {
    const s = newShip();
    s.yaw = 0; s.pitch = 0;
    for (let i = 0; i < 30; i++) stepShip(s, { ...idle, forward: 1 }, 1 / 60, opts);
    for (let i = 0; i < 40; i++) stepShip(s, idle, 1 / 60, opts);
    expect(s.vel.length()).toBeLessThan(1);
  });

  it('turns with the mouse and keeps the pitch away from the zenith', () => {
    const s = newShip();
    stepShip(s, { ...idle, mx: 100 }, 1 / 60, opts);
    expect(s.yaw).toBeLessThan(0);
    stepShip(s, { ...idle, my: -100000 }, 1 / 60, opts);
    expect(s.pitch).toBeLessThanOrEqual(1.45);
    stepShip(s, { ...idle, my: 100000 }, 1 / 60, opts);
    expect(s.pitch).toBeGreaterThanOrEqual(-1.45);
  });

  it('turns the ship back at the edge of the Galaxy', () => {
    const s = newShip();
    s.pos.set(0, 0, R_BOUND - 10);
    s.yaw = 0; s.pitch = 0; // looking toward -z: inward; turn around
    s.yaw = Math.PI;
    let edge = false;
    for (let i = 0; i < 600; i++) { edge = stepShip(s, { ...idle, forward: 1, boost: true }, 1 / 60, opts).edge || edge; }
    expect(edge).toBe(true);
    expect(s.pos.length()).toBeLessThanOrEqual(R_BOUND + 0.001);
  });

  it('does not fly through the Quasar', () => {
    const s = newShip();
    s.pos.set(0, 0, 60);
    s.yaw = 0; s.pitch = 0;
    for (let i = 0; i < 400; i++) stepShip(s, { ...idle, forward: 1, boost: true }, 1 / 60, opts);
    expect(s.pos.length()).toBeGreaterThanOrEqual(QUASAR_R);
  });

  it('levels the roll back to the horizon', () => {
    const s = newShip();
    for (let i = 0; i < 20; i++) stepShip(s, { ...idle, strafe: 1 }, 1 / 60, opts);
    expect(Math.abs(s.roll)).toBeGreaterThan(0.1);
    for (let i = 0; i < 120; i++) stepShip(s, idle, 1 / 60, opts);
    expect(Math.abs(s.roll)).toBeLessThan(0.01);
  });

  it('changes the cruise speed with the wheel, within limits', () => {
    expect(wheelCruise(100, -100)).toBeGreaterThan(100);
    expect(wheelCruise(100, 100)).toBeLessThan(100);
    expect(wheelCruise(CRUISE_MAX, -1000)).toBe(CRUISE_MAX);
    expect(wheelCruise(CRUISE_MIN, 1000)).toBe(CRUISE_MIN);
  });
});

describe('orbit rig and aiming', () => {
  it('round-trips a camera position', () => {
    const focus = new Vector3(10, 5, -20);
    const pos = new Vector3(60, 40, 80);
    const rig = rigFromCamera(pos, focus);
    expect(rigPosition(rig).distanceTo(pos)).toBeLessThan(1e-6);
    const ship = shipFromCamera(pos, focus);
    expect(forwardOf(ship.yaw, ship.pitch).dot(focus.clone().sub(pos).normalize())).toBeCloseTo(1, 5);
  });

  it('aims at the thing nearest the heading, ignoring what is behind', () => {
    const origin = new Vector3();
    const dir = new Vector3(0, 0, -1);
    const things = [
      { id: 'near', pos: new Vector3(0, 0, -50), radius: 3 },
      { id: 'side', pos: new Vector3(40, 0, -50), radius: 3 },
      { id: 'behind', pos: new Vector3(0, 0, 50), radius: 3 },
    ];
    expect(aimedAt(origin, dir, things)?.id).toBe('near');
    expect(aimedAt(origin, dir, things.slice(1, 2))).toBeNull();
    expect(aimedAt(origin, dir, things.slice(2))).toBeNull();
  });

  it('takes longer to fly far, but not endlessly', () => {
    expect(flightDuration(2000)).toBeGreaterThan(flightDuration(300));
    expect(flightDuration(100000)).toBeLessThanOrEqual(2.6);
  });
});
