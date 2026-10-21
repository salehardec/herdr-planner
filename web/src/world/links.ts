/* `after` links between planets: a curve with a running dash, an arrowhead at
   the task that waits, and a spark travelling from the one it waits for. */

import {
  AdditiveBlending, BufferAttribute, BufferGeometry, Color, ConeGeometry, DynamicDrawUsage, Group, InstancedMesh,
  LineSegments, Matrix4, MeshBasicMaterial, Points, PointsMaterial, QuadraticBezierCurve3, Quaternion, ShaderMaterial,
  Vector3,
} from 'three';
import type { Link } from '../types';
import type { Palette } from './palette';
import { hash } from '../util/format';

const SAMPLES = 28;

const VERT = /* glsl */ `
attribute float aDist;
attribute vec3 aColor;
attribute float aDash;
attribute float aAlpha;
varying float vDist;
varying vec3 vColor;
varying float vDash;
varying float vAlpha;
void main() {
  vDist = aDist; vColor = aColor; vDash = aDash; vAlpha = aAlpha;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}`;
const FRAG = /* glsl */ `
uniform float uTime;
varying float vDist;
varying vec3 vColor;
varying float vDash;
varying float vAlpha;
void main() {
  float d = mod(vDist - uTime * 6.0, 5.2);
  float on = mix(1.0, step(d, 2.6), vDash);
  if (on < 0.5) discard;
  gl_FragColor = vec4(vColor, vAlpha);
}`;

export interface LinkRec {
  link: Link;
  galaxy: string;
  points: Vector3[];
  length: number;
  a: Vector3;
  b: Vector3;
}

const _q = new Quaternion();
const _m = new Matrix4();
const _s = new Vector3();
const _up = new Vector3(0, 1, 0);
const _dir = new Vector3();

export class Links {
  readonly group = new Group();
  private lines: LineSegments;
  private mat: ShaderMaterial;
  private arrows: InstancedMesh;
  private sparks: Points;
  private recs: LinkRec[] = [];
  private moving = true;
  private pal!: Palette;
  private on: (id: string) => boolean = () => true;
  private alphaDirty = true;

  constructor() {
    this.mat = new ShaderMaterial({
      vertexShader: VERT,
      fragmentShader: FRAG,
      transparent: true,
      depthWrite: false,
      uniforms: { uTime: { value: 0 } },
    });
    this.lines = new LineSegments(new BufferGeometry(), this.mat);
    this.lines.frustumCulled = false;
    this.lines.renderOrder = 3;
    this.arrows = new InstancedMesh(new ConeGeometry(0.9, 2.6, 8), new MeshBasicMaterial({ transparent: true, opacity: 0.9 }), 8);
    this.arrows.instanceMatrix.setUsage(DynamicDrawUsage);
    this.arrows.frustumCulled = false;
    this.arrows.count = 0;
    this.sparks = new Points(new BufferGeometry(), new PointsMaterial({
      size: 3.4, sizeAttenuation: false, transparent: true, opacity: 0.95, depthWrite: false, blending: AdditiveBlending, vertexColors: true,
    }));
    this.sparks.frustumCulled = false;
    this.group.add(this.lines, this.arrows, this.sparks);
  }

  setMoving(v: boolean): void { this.moving = v; }
  setPalette(p: Palette): void { this.pal = p; }

  setFilter(on: (id: string) => boolean): void { this.on = on; this.alphaDirty = true; }

  /** `resolve(ref)` gives a world position for an id, or for a far reference (`vm-a:ELA-3`). */
  build(links: Link[], galaxy: string): void {
    this.recs = links.map((link) => ({ link, galaxy, points: Array.from({ length: SAMPLES + 1 }, () => new Vector3()), length: 0, a: new Vector3(), b: new Vector3() }));
    const n = this.recs.length;
    const segs = n * SAMPLES;
    const pos = new Float32Array(segs * 6);
    const dist = new Float32Array(segs * 2);
    const col = new Float32Array(segs * 6);
    const dash = new Float32Array(segs * 2);
    const alpha = new Float32Array(segs * 2).fill(0.5);
    const g = new BufferGeometry();
    g.setAttribute('position', new BufferAttribute(pos, 3).setUsage(DynamicDrawUsage));
    g.setAttribute('aDist', new BufferAttribute(dist, 1).setUsage(DynamicDrawUsage));
    g.setAttribute('aColor', new BufferAttribute(col, 3));
    g.setAttribute('aDash', new BufferAttribute(dash, 1));
    g.setAttribute('aAlpha', new BufferAttribute(alpha, 1).setUsage(DynamicDrawUsage));
    this.lines.geometry.dispose();
    this.lines.geometry = g;
    this.arrows.dispose();
    this.group.remove(this.arrows);
    this.arrows = new InstancedMesh(new ConeGeometry(0.9, 2.6, 8), new MeshBasicMaterial({ transparent: true, opacity: 0.9 }), Math.max(8, n));
    this.arrows.instanceMatrix.setUsage(DynamicDrawUsage);
    this.arrows.frustumCulled = false;
    this.arrows.count = 0;
    this.group.add(this.arrows);
    const sg = new BufferGeometry();
    sg.setAttribute('position', new BufferAttribute(new Float32Array(n * 3), 3).setUsage(DynamicDrawUsage));
    sg.setAttribute('color', new BufferAttribute(new Float32Array(n * 3), 3));
    this.sparks.geometry.dispose();
    this.sparks.geometry = sg;
    this.alphaDirty = true;
    this.paint();
  }

  private colorOf(rec: LinkRec): Color {
    const k = rec.link.kind;
    return k === 'waiting' ? this.pal.accent : this.pal.muted;
  }

  private paint(): void {
    if (!this.pal) return;
    const col = this.lines.geometry.getAttribute('aColor') as BufferAttribute | undefined;
    const dash = this.lines.geometry.getAttribute('aDash') as BufferAttribute | undefined;
    if (!col || !dash) return;
    const sc = this.sparks.geometry.getAttribute('color') as BufferAttribute;
    this.recs.forEach((rec, i) => {
      const c = this.colorOf(rec);
      const isDash = rec.link.kind !== 'done' ? 1 : 0;
      for (let s = 0; s < SAMPLES; s++) {
        for (let e = 0; e < 2; e++) {
          const v = (i * SAMPLES + s) * 2 + e;
          col.setXYZ(v, c.r, c.g, c.b);
          dash.setX(v, isDash);
        }
      }
      sc.setXYZ(i, c.r, c.g, c.b);
      this.arrows.setColorAt(i, c);
    });
    col.needsUpdate = true;
    dash.needsUpdate = true;
    sc.needsUpdate = true;
    if (this.arrows.instanceColor) this.arrows.instanceColor.needsUpdate = true;
  }

  get records(): LinkRec[] { return this.recs; }

  /** Updates the curves from live endpoint positions; endpoints that cannot be resolved hide the link. */
  update(time: number, resolve: (ref: string) => Vector3 | null): void {
    this.mat.uniforms.uTime.value = this.moving ? time : 0;
    const geo = this.lines.geometry;
    const pos = geo.getAttribute('position') as BufferAttribute | undefined;
    const dist = geo.getAttribute('aDist') as BufferAttribute | undefined;
    const alpha = geo.getAttribute('aAlpha') as BufferAttribute | undefined;
    if (!pos || !dist || !alpha) return;
    const sp = this.sparks.geometry.getAttribute('position') as BufferAttribute;
    let arrows = 0;
    const curve = new QuadraticBezierCurve3(new Vector3(), new Vector3(), new Vector3());
    this.recs.forEach((rec, i) => {
      const a = resolve(rec.link.from);
      const b = resolve(rec.link.to);
      const ok = !!a && !!b;
      if (ok && a && b) {
        rec.a.copy(a); rec.b.copy(b);
        const len = a.distanceTo(b);
        const mid = a.clone().add(b).multiplyScalar(0.5);
        mid.y += 9 + len * 0.12;
        curve.v0.copy(a); curve.v1.copy(mid); curve.v2.copy(b);
        let acc = 0;
        for (let s = 0; s <= SAMPLES; s++) {
          curve.getPoint(s / SAMPLES, rec.points[s]);
          if (s > 0) acc += rec.points[s].distanceTo(rec.points[s - 1]);
        }
        rec.length = acc;
        let run = 0;
        for (let s = 0; s < SAMPLES; s++) {
          const p0 = rec.points[s], p1 = rec.points[s + 1];
          const v0 = (i * SAMPLES + s) * 2;
          pos.setXYZ(v0, p0.x, p0.y, p0.z);
          pos.setXYZ(v0 + 1, p1.x, p1.y, p1.z);
          dist.setX(v0, run);
          run += p0.distanceTo(p1);
          dist.setX(v0 + 1, run);
        }
        // Arrow at the end, along the last segment.
        const tip = rec.points[SAMPLES];
        _dir.copy(tip).sub(rec.points[SAMPLES - 2]).normalize();
        _q.setFromUnitVectors(_up, _dir);
        _m.compose(tip.clone().addScaledVector(_dir, -1.6), _q, _s.setScalar(1));
        this.arrows.setMatrixAt(arrows++, _m);
        // The spark travels from the predecessor to the waiting task.
        if (rec.link.kind !== 'done' && this.moving) {
          const k = (time * 0.22 + (hash(rec.link.from + rec.link.to) % 100) / 100) % 1;
          const idx = Math.min(SAMPLES, Math.floor(k * SAMPLES));
          sp.setXYZ(i, rec.points[idx].x, rec.points[idx].y, rec.points[idx].z);
        } else {
          sp.setXYZ(i, 0, -1e5, 0);
        }
      } else {
        for (let s = 0; s < SAMPLES; s++) {
          const v0 = (i * SAMPLES + s) * 2;
          pos.setXYZ(v0, 0, -1e5, 0);
          pos.setXYZ(v0 + 1, 0, -1e5, 0);
        }
        sp.setXYZ(i, 0, -1e5, 0);
      }
    });
    if (this.alphaDirty) {
      this.recs.forEach((rec, i) => {
        const dim = !(this.on(rec.link.from) && this.on(rec.link.to));
        const a = dim ? 0.06 : rec.link.kind === 'done' ? 0.2 : 0.55;
        for (let s = 0; s < SAMPLES * 2; s++) alpha.setX(i * SAMPLES * 2 + s, a);
      });
      alpha.needsUpdate = true;
      this.alphaDirty = false;
    }
    pos.needsUpdate = true;
    dist.needsUpdate = true;
    sp.needsUpdate = true;
    this.arrows.count = arrows;
    this.arrows.instanceMatrix.needsUpdate = true;
  }

  /** The link under a screen point (nearest sampled segment within `maxPx`). */
  pick(project: (v: Vector3) => { x: number; y: number; ok: boolean }, px: number, py: number, maxPx: number): LinkRec | null {
    let best: LinkRec | null = null;
    let bestD = maxPx;
    for (const rec of this.recs) {
      if (rec.length === 0) continue;
      let prev: { x: number; y: number; ok: boolean } | null = null;
      for (const p of rec.points) {
        const q = project(p);
        if (prev && prev.ok && q.ok) {
          const d = distToSegment(px, py, prev.x, prev.y, q.x, q.y);
          if (d < bestD) { bestD = d; best = rec; }
        }
        prev = q;
      }
    }
    return best;
  }

  dispose(): void {
    this.lines.geometry.dispose();
    this.mat.dispose();
    this.arrows.dispose();
    this.sparks.geometry.dispose();
  }
}

export function distToSegment(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
  const dx = bx - ax, dy = by - ay;
  const l2 = dx * dx + dy * dy;
  const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / l2));
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}
