/* The camera's place for a universe: the whole nebula with its planets and their labels in the free part of the screen
   (the part the panels leave), seen a little from above. Pure maths: the same universe and the same free area always give
   the same rig, so a second flight there ends in the same picture. */

import { Vector3 } from 'three';
import type { UniverseLayout } from './layout';
import type { RigState } from './flight';

/** The free part of the screen, in pixels of the canvas. */
export interface Rect { l: number; t: number; r: number; b: number }

export interface FrameView {
  /** Size of the canvas, px. */
  w: number;
  h: number;
  /** Vertical field of view, degrees. */
  fov: number;
  /** How much bigger than the design the interface is (the size of the labels). */
  k: number;
}

/** A little from above: the nebula is a disc, and the planets of one orbit do not hide each other. */
export const UNIVERSE_PITCH = 0.4;
/** The side the camera looks from, turned from «outward from the Quasar» (radians): the Quasar is behind the nebula. */
export const UNIVERSE_YAW = 0.3;
/** The nearest the camera comes: a project of one or two tasks is not shown as a close-up of two huge balls. */
export const UNIVERSE_MIN_DIST = 70;
const MAX_DIST = 4000;
/** The largest radius of the nebula's cloud, as a part of the universe's radius (nebulae.ts draws up to 1.15). */
const CLOUD = 1.08;
const RING_POINTS = 20;
/** What a label takes beside its planet, px at the design size: half the chip's width, the chip's height and its lift. */
const CHIP_HALF_W = 52;
const CHIP_H = 20;
const CHIP_LIFT = 8;
/** The planet's mean radius in the world, for the room it needs on the screen. */
const PLANET_R = 2.3;

interface Pt { o: Vector3; planet: boolean }

/** The free area of a window with nothing open. */
export const wholeView = (v: FrameView): Rect => ({ l: 0, t: 0, r: v.w, b: v.h });

/** Where a universe is looked at from: the rig that puts its centre in the middle of the free area and fits it there. */
export function frameUniverse(u: UniverseLayout, view: FrameView, free?: Rect): RigState {
  const yaw = Math.atan2(u.center.x, u.center.z) + UNIVERSE_YAW;
  const pitch = UNIVERSE_PITCH;
  const centre = new Vector3(u.center.x, u.center.y, u.center.z);
  const area = free ?? wholeView(view);
  const out = new Vector3(Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch));
  const fwd = out.clone().negate();
  const right = new Vector3(-fwd.z, 0, fwd.x).normalize();
  const up = new Vector3().crossVectors(right, fwd);
  const tanV = Math.tan((view.fov * Math.PI) / 360);
  const tanH = tanV * (view.w / Math.max(1, view.h));
  const k = view.k;

  // What must be in the frame: every place a planet can be (the orbits turn, so a whole ring each), and the cloud.
  const pts: Pt[] = [];
  const ring = (r: number, planet: boolean): void => {
    for (let i = 0; i < RING_POINTS; i++) {
      const a = (i / RING_POINTS) * Math.PI * 2;
      pts.push({ o: new Vector3(Math.cos(a) * r, 0, Math.sin(a) * r), planet });
    }
  };
  for (const r of new Set(u.planets.map((p) => Math.round(p.radius * 4) / 4))) ring(r, true);
  ring(u.radius * CLOUD, false);

  const pad = Math.max(10 * k, 0.025 * Math.min(area.r - area.l, area.b - area.t));
  const px = (dist: number): number => (PLANET_R * view.h) / (2 * tanV * dist);
  /** The room the planet's label needs above and beside it, px, at this distance. */
  const room = (dist: number): { side: number; top: number; bottom: number } => {
    const rpx = px(dist);
    return { side: Math.max(CHIP_HALF_W * k, rpx), top: (Math.max(10, rpx + CHIP_LIFT) + CHIP_H) * k, bottom: rpx };
  };

  /** The camera at `dist`, with the centre of the nebula shown in the middle of the area the planets and their labels may take. */
  const at = (dist: number): { focus: Vector3; fits: boolean } => {
    const m = room(dist);
    const box = { l: area.l + pad + m.side, r: area.r - pad - m.side, t: area.t + pad + m.top, b: area.b - pad - m.bottom };
    const cx = (box.l + box.r) / 2;
    const cy = (box.t + box.b) / 2;
    const nx = (cx / view.w) * 2 - 1;
    const ny = 1 - (cy / view.h) * 2;
    const focus = centre.clone().addScaledVector(right, -dist * nx * tanH).addScaledVector(up, -dist * ny * tanV);
    const pos = focus.clone().addScaledVector(out, dist);
    const v = new Vector3();
    let fits = box.r > box.l && box.b > box.t;
    for (const p of pts) {
      if (!fits) break;
      v.copy(centre).add(p.o).sub(pos);
      const z = v.dot(fwd);
      if (z < 1) { fits = false; break; }
      const sx = view.w / 2 + (v.dot(right) / (z * tanH)) * (view.w / 2);
      const sy = view.h / 2 - (v.dot(up) / (z * tanV)) * (view.h / 2);
      const l = p.planet ? box.l : area.l + pad, r = p.planet ? box.r : area.r - pad;
      const t = p.planet ? box.t : area.t + pad, b = p.planet ? box.b : area.b - pad;
      if (sx < l || sx > r || sy < t || sy > b) fits = false;
    }
    return { focus, fits };
  };

  let lo = UNIVERSE_MIN_DIST;
  let found = at(lo);
  if (!found.fits) {
    let hi = lo * 2;
    while (hi < MAX_DIST && !at(hi).fits) { lo = hi; hi *= 2; }
    for (let i = 0; i < 28; i++) {
      const mid = (lo + hi) / 2;
      if (at(mid).fits) hi = mid; else lo = mid;
    }
    lo = Math.min(hi, MAX_DIST);
    found = at(lo);
  }
  return { focus: found.focus, yaw, pitch, dist: lo };
}

/** Insets of the panels → the free area. `insets` are the pixels each side is covered by; the free part keeps at least
    `min` of the width and the height (a window too small for its panels does not lose the picture altogether). */
export function freeRect(w: number, h: number, insets: { l: number; t: number; r: number; b: number }, min = 0.4): Rect {
  const shrink = (a: number, b: number, whole: number): [number, number] => {
    const room = whole * (1 - min);
    const sum = a + b;
    return sum > room && sum > 0 ? [(a * room) / sum, (b * room) / sum] : [a, b];
  };
  const [l, r] = shrink(Math.max(0, insets.l), Math.max(0, insets.r), w);
  const [t, b] = shrink(Math.max(0, insets.t), Math.max(0, insets.b), h);
  return { l, t, r: w - r, b: h - b };
}
