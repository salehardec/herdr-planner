import { describe, expect, it } from 'vitest';
import { PerspectiveCamera, Vector3 } from 'three';
import { frameUniverse, freeRect, UNIVERSE_MIN_DIST, UNIVERSE_PITCH, type FrameView, type Rect } from './framing';
import { rigPosition } from './flight';
import type { PlanetLayout, UniverseLayout } from './layout';

const planet = (i: number, orbit: number): PlanetLayout => ({
  id: `p/${i}`, tag: `P-${i}`, title: '', status: 'working', app: '', repo: 'p', n: i, orbit, angle0: i * 2.1, radius: 9 + orbit * 6.5,
  size: 1.6, belt: false, created: 0,
});

function universe(live: number, at = { x: 300, y: 4, z: -200 }): UniverseLayout {
  const planets = Array.from({ length: live }, (_, i) => planet(i, Math.floor(i / 3)));
  const outer = planets.reduce((m, p) => Math.max(m, p.radius), 0);
  return {
    name: 'p', key: 'P', group: 'g', groupIndex: 0, quiet: false, empty: false, active: live, total: live,
    center: at, radius: Math.max(20, outer + 9), planets, index: 0,
  };
}

const view: FrameView = { w: 1500, h: 950, fov: 45, k: 1 };

/** Where the planets of the universe are on the screen, seen through a real camera of the page's kind. */
function screenOf(u: UniverseLayout, rig: ReturnType<typeof frameUniverse>, v: FrameView): { x: number; y: number }[] {
  const cam = new PerspectiveCamera(v.fov, v.w / v.h, 0.5, 9000);
  cam.position.copy(rigPosition(rig));
  cam.lookAt(rig.focus);
  cam.updateMatrixWorld(true);
  const out: { x: number; y: number }[] = [];
  for (const p of u.planets) for (let i = 0; i < 24; i++) {
    const a = p.angle0 + (i / 24) * Math.PI * 2;
    const q = new Vector3(u.center.x + Math.cos(a) * p.radius, u.center.y, u.center.z + Math.sin(a) * p.radius).project(cam);
    out.push({ x: (q.x * 0.5 + 0.5) * v.w, y: (-q.y * 0.5 + 0.5) * v.h });
  }
  return out;
}

const inside = (pts: { x: number; y: number }[], r: Rect): boolean => pts.every((p) => p.x >= r.l && p.x <= r.r && p.y >= r.t && p.y <= r.b);

describe('framing a universe', () => {
  it('puts every place of every planet inside the window, and the nebula in the middle', () => {
    for (const live of [1, 3, 10, 24, 50]) {
      const u = universe(live);
      const rig = frameUniverse(u, view);
      const pts = screenOf(u, rig, view);
      expect(inside(pts, { l: 0, t: 0, r: view.w, b: view.h })).toBe(true);
      const mid = (a: number[]): number => (Math.min(...a) + Math.max(...a)) / 2;
      expect(Math.abs(mid(pts.map((p) => p.x)) - view.w / 2)).toBeLessThan(view.w * 0.05);
    }
  });

  it('looks a little from above, from the same side every time', () => {
    const u = universe(10);
    const a = frameUniverse(u, view);
    const b = frameUniverse(u, view);
    expect(a.pitch).toBeCloseTo(UNIVERSE_PITCH, 6);
    expect(a.yaw).toBe(b.yaw);
    expect(a.dist).toBe(b.dist);
    expect(a.focus.distanceTo(b.focus)).toBe(0);
  });

  it('comes closer to a small project and keeps away from a big one', () => {
    const small = frameUniverse(universe(2), view);
    const mid = frameUniverse(universe(10), view);
    const big = frameUniverse(universe(40), view);
    expect(small.dist).toBeGreaterThanOrEqual(UNIVERSE_MIN_DIST);
    expect(small.dist).toBeLessThan(mid.dist);
    expect(mid.dist).toBeLessThan(big.dist);
  });

  it('frames the nebula in the free area, not under the panels', () => {
    const u = universe(12);
    const free = freeRect(view.w, view.h, { l: 400, t: 52, r: 400, b: 0 });
    const rig = frameUniverse(u, view, free);
    const pts = screenOf(u, rig, view);
    expect(inside(pts, free)).toBe(true);
    const mid = (a: number[]): number => (Math.min(...a) + Math.max(...a)) / 2;
    expect(Math.abs(mid(pts.map((p) => p.x)) - (free.l + free.r) / 2)).toBeLessThan(40);
    // a chat under it: the picture goes up
    const chat = freeRect(view.w, view.h, { l: 400, t: 52, r: 400, b: 420 });
    const high = screenOf(u, frameUniverse(u, view, chat), view);
    expect(inside(high, chat)).toBe(true);
  });

  it('leaves room for the labels, which grow with the interface', () => {
    const u = universe(12);
    const k = 1.5;
    const rig = frameUniverse(u, { ...view, k });
    const pts = screenOf(u, rig, view);
    const left = Math.min(...pts.map((p) => p.x));
    const top = Math.min(...pts.map((p) => p.y));
    expect(left).toBeGreaterThanOrEqual(52 * k - 1);
    expect(top).toBeGreaterThan(40 * k);
  });

  it('works for a universe with nothing in it and for a narrow window', () => {
    const empty = universe(0);
    expect(frameUniverse(empty, view).dist).toBeGreaterThanOrEqual(UNIVERSE_MIN_DIST);
    const phone: FrameView = { w: 400, h: 800, fov: 45, k: 1 };
    const u = universe(8);
    expect(inside(screenOf(u, frameUniverse(u, phone), phone), { l: 0, t: 0, r: 400, b: 800 })).toBe(true);
  });
});

describe('the free area', () => {
  it('keeps at least 40 % of the window when the panels ask for more', () => {
    const r = freeRect(1000, 800, { l: 700, t: 0, r: 700, b: 900 });
    expect(r.r - r.l).toBeCloseTo(400, 6);
    expect(r.b - r.t).toBeCloseTo(320, 6);
  });
});
