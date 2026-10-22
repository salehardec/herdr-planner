/* Planets (tasks), their hangar rings (the task's main agent), moons (its
   sub-agents), status halos and beacons. Everything is instanced: a few
   hundred planets are a handful of draw calls. */

import {
  AdditiveBlending, BoxGeometry, BufferAttribute, BufferGeometry, Color, DynamicDrawUsage, Group,
  InstancedBufferAttribute, InstancedMesh, LineBasicMaterial, LineSegments, Matrix4, MeshBasicMaterial,
  Mesh, OctahedronGeometry, Points, PointsMaterial, RingGeometry, Quaternion, ShaderMaterial, SphereGeometry, TetrahedronGeometry,
  TorusGeometry, Vector3, Euler,
} from 'three';
import type { Layout, PlanetLayout, UniverseLayout } from './layout';
import { planetPosition } from './layout';
import type { Palette } from './palette';
import { PROGRAM_SHAPE } from './palette';
import type { Entity, Status, Subagent, Task } from '../types';
import { hash } from '../util/format';

const PLANET_VERT = /* glsl */ `
attribute float aGlow;
attribute float aDim;
uniform float uScale;
uniform float uMinPx;
varying vec3 vCol;
varying float vL;
varying float vRim;
varying float vGlow;
varying float vDim;
void main() {
  vec3 c = vec3(instanceMatrix[3]);
  float s = length(vec3(instanceMatrix[0]));
  vec4 mvc = viewMatrix * vec4(c, 1.0);
  float dist = max(1.0, -mvc.z);
  float px = s * uScale / dist;
  float boost = max(1.0, uMinPx / max(px, 0.0001));
  vec3 wpos = c + position * s * boost;
  vec3 n = normalize(position);
  vec3 toL = normalize(-c + vec3(0.0001));
  float ndl = max(dot(n, toL), 0.0);
  vL = 0.46 + 0.8 * ndl;
  vec3 vn = normalize(mat3(viewMatrix) * n);
  vRim = pow(1.0 - max(vn.z, 0.0), 2.2);
  vCol = instanceColor;
  vGlow = aGlow;
  vDim = aDim;
  gl_Position = projectionMatrix * viewMatrix * vec4(wpos, 1.0);
}`;

const PLANET_FRAG = /* glsl */ `
varying vec3 vCol;
varying float vL;
varying float vRim;
varying float vGlow;
varying float vDim;
void main() {
  vec3 col = vCol * vL + vCol * vRim * 0.55 + vCol * vGlow * 0.65;
  gl_FragColor = vec4(col, vDim);
}`;

const MOON_LIMIT = 12;
const NEAR = 260;

export interface Item {
  key: string;
  entity: Entity;
  pos: Vector3;
  radius: number;
  label: string;
  sub: string;
  status?: Status;
  /** Higher wins a label slot. */
  priority: number;
  universe: string;
}

interface PlanetRec {
  p: PlanetLayout;
  u: UniverseLayout;
  task: Task | undefined;
  idx: number;
  item: Item;
  phase: number;
}

interface MoonRec {
  key: string;
  planetId: string;
  sa: Subagent;
  angle0: number;
  radius: number;
  speed: number;
  born: number;
  leaving: number | null;
  item: Item;
}

const _m = new Matrix4();
const _q = new Quaternion();
const _qi = new Quaternion();
const _s = new Vector3();
const _p = new Vector3();
const _e = new Euler();
const _c = new Color();
const ZERO = new Vector3();

export class Bodies {
  readonly group = new Group();
  private planets!: InstancedMesh;
  private planetMat: ShaderMaterial;
  private halos!: InstancedMesh;
  private rings!: InstancedMesh;
  private marks: Record<string, InstancedMesh> = {};
  private moons!: InstancedMesh;
  private tails!: InstancedMesh;
  private beacons: LineSegments;
  private beaconSparks: Points;
  private recs: PlanetRec[] = [];
  private byId = new Map<string, PlanetRec>();
  private moonRecs = new Map<string, MoonRec>();
  private cap = 0;
  private moonCap = 0;
  private glowAttr!: InstancedBufferAttribute;
  private dimAttr!: InstancedBufferAttribute;
  private pal!: Palette;
  private layout: Layout | null = null;
  private tasks = new Map<string, Task>();
  private filter: Status[] | null = null;
  private colorsDirty = true;
  private beaconRecs: PlanetRec[] = [];
  private simple = false;
  private fxRings: { mesh: import('three').Mesh; t0: number; dur: number }[] = [];
  private coordMoons: Subagent[] = [];
  readonly moonItems: Item[] = [];
  readonly planetItems: Item[] = [];
  private hoverId: string | null = null;
  private fresh: Record<string, number> = {};
  private lastTime = 0;

  constructor() {
    this.planetMat = new ShaderMaterial({
      vertexShader: PLANET_VERT,
      fragmentShader: PLANET_FRAG,
      transparent: true,
      uniforms: { uScale: { value: 800 }, uMinPx: { value: 3.2 } },
    });
    this.beacons = new LineSegments(new BufferGeometry(), new LineBasicMaterial({
      transparent: true, opacity: 0.35, depthWrite: false, blending: AdditiveBlending,
    }));
    this.beacons.frustumCulled = false;
    this.beaconSparks = new Points(new BufferGeometry(), new PointsMaterial({
      size: 3.2, sizeAttenuation: false, transparent: true, opacity: 0.95, depthWrite: false, blending: AdditiveBlending,
    }));
    this.beaconSparks.frustumCulled = false;
    this.group.add(this.beacons, this.beaconSparks);
    this.allocate(64, 32);
  }

  private allocate(cap: number, moonCap: number): void {
    this.cap = cap;
    this.moonCap = moonCap;
    for (const m of [this.planets, this.halos, this.rings, this.moons, this.tails, ...Object.values(this.marks)]) {
      if (m) { this.group.remove(m); m.dispose(); }
    }
    const planetGeo = new SphereGeometry(1, 12, 9);
    this.glowAttr = new InstancedBufferAttribute(new Float32Array(cap), 1).setUsage(DynamicDrawUsage);
    this.dimAttr = new InstancedBufferAttribute(new Float32Array(cap).fill(1), 1).setUsage(DynamicDrawUsage);
    planetGeo.setAttribute('aGlow', this.glowAttr);
    planetGeo.setAttribute('aDim', this.dimAttr);
    this.planets = new InstancedMesh(planetGeo, this.planetMat, cap);
    this.planets.instanceMatrix.setUsage(DynamicDrawUsage);
    this.planets.frustumCulled = false;
    this.planets.renderOrder = 4;
    const basic = (opacity: number): MeshBasicMaterial => new MeshBasicMaterial({ transparent: true, opacity, depthWrite: false });
    this.halos = new InstancedMesh(new TorusGeometry(1, 0.07, 6, 32), basic(0.85), cap);
    this.rings = new InstancedMesh(new TorusGeometry(1, 0.045, 6, 36), basic(0.7), cap);
    const shapes: Record<string, BufferGeometry> = {
      sphere: new SphereGeometry(0.5, 8, 6), box: new BoxGeometry(0.8, 0.8, 0.8),
      octa: new OctahedronGeometry(0.62), tetra: new TetrahedronGeometry(0.68),
    };
    this.marks = {};
    for (const [k, g] of Object.entries(shapes)) {
      this.marks[k] = new InstancedMesh(g, new MeshBasicMaterial({ transparent: true, opacity: 0.95 }), cap);
    }
    this.moons = new InstancedMesh(new SphereGeometry(0.4, 8, 6), new MeshBasicMaterial({ transparent: true, opacity: 0.95 }), moonCap);
    this.tails = new InstancedMesh(new SphereGeometry(0.3, 6, 4), new MeshBasicMaterial({ transparent: true, opacity: 0.6, depthWrite: false, blending: AdditiveBlending }), moonCap * 6);
    this.planets.setColorAt(0, _c.set(0xffffff));
    for (const m of [this.halos, this.rings, ...Object.values(this.marks), this.moons, this.tails]) {
      // The colour attribute must exist before the first draw, or the program is built without it.
      m.setColorAt(0, _c.set(0xffffff));
      m.instanceMatrix.setUsage(DynamicDrawUsage);
      m.frustumCulled = false;
      m.count = 0;
      m.renderOrder = 5;
      this.group.add(m);
    }
    this.group.add(this.planets);
    this.colorsDirty = true;
  }

  setSimple(v: boolean): void { this.simple = v; }
  setHover(id: string | null): void { if (id !== this.hoverId) { this.hoverId = id; this.colorsDirty = true; } }
  setFilter(f: Status[] | null): void { this.filter = f && f.length ? f : null; this.colorsDirty = true; }
  setFresh(f: Record<string, number>): void { this.fresh = f; }
  setPalette(p: Palette): void { this.pal = p; this.colorsDirty = true; }

  /** Rebuilds the instances for a layout; moons are kept by id so they can fade out. */
  build(layout: Layout, tasks: Task[], pal: Palette, coordinatorMoons: Subagent[], now: number): void {
    this.pal = pal;
    this.layout = layout;
    this.tasks = new Map(tasks.map((t) => [t.id, t]));
    this.coordMoons = coordinatorMoons;
    const planets = layout.universes.flatMap((u) => u.planets.map((p) => ({ u, p })));
    if (planets.length > this.cap) this.allocate(Math.ceil(planets.length * 1.4) + 16, this.moonCap);
    this.recs = [];
    this.byId.clear();
    this.planetItems.length = 0;
    planets.forEach(({ u, p }, idx) => {
      const task = this.tasks.get(p.id);
      const item: Item = {
        key: `planet|${p.id}`,
        entity: { kind: 'planet', id: p.id },
        pos: new Vector3(), radius: p.size, label: p.tag, sub: p.title, status: p.status,
        priority: p.status === 'waiting' || p.status === 'failed' ? 60 : p.status === 'ready' || p.status === 'plan' ? 50 : p.status === 'done' ? 5 : 30,
        universe: u.name,
      };
      const rec: PlanetRec = { p, u, task, idx, item, phase: (hash(p.id) % 628) / 100 };
      this.recs.push(rec);
      this.byId.set(p.id, rec);
      this.planetItems.push(item);
    });
    this.beaconRecs = this.recs.filter((r) => !r.p.belt && (r.p.status === 'waiting' || r.p.status === 'plan' || r.p.status === 'ready'));
    this.planets.count = this.recs.length;
    this.halos.count = 0;
    this.syncMoons(now);
    this.colorsDirty = true;
    this.rebuildBeacons();
    // Positions are right at once (a flight started now must not aim at the origin until the next frame).
    for (const rec of this.recs) planetPosition(rec.u, rec.p, this.lastTime, !this.simple, rec.item.pos);
    this.updateMoons(this.lastTime, this.group);
  }

  private rebuildBeacons(): void {
    const n = this.beaconRecs.length;
    this.beacons.geometry.dispose();
    this.beacons.geometry = new BufferGeometry();
    this.beacons.geometry.setAttribute('position', new BufferAttribute(new Float32Array(n * 6), 3).setUsage(DynamicDrawUsage));
    const col = new Float32Array(n * 6);
    this.beaconRecs.forEach((r, i) => {
      const c = this.pal.status[r.p.status];
      for (let k = 0; k < 2; k++) { col[i * 6 + k * 3] = c.r; col[i * 6 + k * 3 + 1] = c.g; col[i * 6 + k * 3 + 2] = c.b; }
    });
    this.beacons.geometry.setAttribute('color', new BufferAttribute(col, 3));
    (this.beacons.material as LineBasicMaterial).vertexColors = true;
    (this.beacons.material as LineBasicMaterial).needsUpdate = true;
    this.beaconSparks.geometry.dispose();
    this.beaconSparks.geometry = new BufferGeometry();
    this.beaconSparks.geometry.setAttribute('position', new BufferAttribute(new Float32Array(n * 3), 3).setUsage(DynamicDrawUsage));
    this.beaconSparks.geometry.setAttribute('color', new BufferAttribute(col.slice(0, n * 3).map((_, i) => col[(Math.floor(i / 3)) * 6 + (i % 3)]), 3));
    (this.beaconSparks.material as PointsMaterial).vertexColors = true;
    (this.beaconSparks.material as PointsMaterial).needsUpdate = true;
  }

  /** Moons of the planets (and of the Quasar) from the sub-agents in the snapshot. */
  private syncMoons(now: number): void {
    const seen = new Set<string>();
    const add = (planetId: string, sa: Subagent, radius: number, label: string): void => {
      const key = `${planetId}#${sa.id}`;
      seen.add(key);
      const old = this.moonRecs.get(key);
      if (old) {
        old.sa = sa;
        old.item.sub = sa.doing || sa.title;
        if (sa.state === 'done' && old.leaving === null) old.leaving = now;
        if (sa.state !== 'done') old.leaving = null;
        return;
      }
      if (sa.state === 'done') return;
      const item: Item = {
        key: `moon|${key}`,
        entity: { kind: 'moon', id: key },
        pos: new Vector3(), radius: 0.9, label: sa.role || label, sub: sa.doing || sa.title, priority: 20,
        universe: planetId === 'quasar' ? '' : (this.byId.get(planetId)?.u.name ?? ''),
      };
      this.moonRecs.set(key, {
        key, planetId, sa, angle0: (hash(key) % 628) / 100, radius,
        speed: 1.5 + (hash(key + 's') % 50) / 100, born: now, leaving: null, item,
      });
    };
    for (const rec of this.recs) {
      const subs = (rec.task?.subagents ?? []).slice(0, MOON_LIMIT);
      subs.forEach((sa, i) => add(rec.p.id, sa, rec.p.size + 2.2 + (i % 3) * 0.9, sa.role));
    }
    this.coordMoons.slice(0, MOON_LIMIT).forEach((sa, i) => add('quasar', sa, 24 + (i % 3) * 3, sa.role));
    for (const [key, rec] of this.moonRecs) {
      if (!seen.has(key) && rec.leaving === null) rec.leaving = now;
    }
    if (this.moonRecs.size > this.moonCap) {
      this.allocate(this.cap, Math.ceil(this.moonRecs.size * 1.5) + 8);
      this.planets.count = this.recs.length;
    }
  }

  get moonList(): MoonRec[] { return [...this.moonRecs.values()]; }

  positionOf(id: string, out = new Vector3()): Vector3 | null {
    const r = this.byId.get(id);
    if (r) return out.copy(r.item.pos);
    const m = this.moonRecs.get(id);
    if (m) return out.copy(m.item.pos);
    return null;
  }

  radiusOf(id: string): number {
    return this.byId.get(id)?.p.size ?? this.moonRecs.get(id)?.item.radius ?? 1;
  }

  universeOf(id: string): string | null {
    return this.byId.get(id)?.u.name ?? null;
  }

  ringFx(id: string, status: Status, now: number, group: Group): void {
    const r = this.byId.get(id);
    if (!r || this.simple) return;
    const mesh = new Mesh(
      new RingGeometry(r.p.size * 1.1, r.p.size * 1.25, 32),
      new MeshBasicMaterial({ color: this.pal.status[status], transparent: true, opacity: 0.9, side: 2, depthWrite: false }),
    );
    mesh.position.copy(r.item.pos);
    mesh.lookAt(mesh.position.clone().add(new Vector3(0, 1, 0)));
    group.add(mesh);
    this.fxRings.push({ mesh, t0: now, dur: 1100 });
  }

  /** Per-frame: positions, matrices, glow. `camPos` decides which decorations are close enough to draw. */
  update(time: number, camPos: Vector3, scale: number, focusUniverse: string | null, group: Group): void {
    this.planetMat.uniforms.uScale.value = scale;
    this.lastTime = time;
    const moving = !this.simple;
    let halo = 0;
    let ring = 0;
    const markCount: Record<string, number> = { sphere: 0, box: 0, octa: 0, tetra: 0 };
    const attrs = this.planets.instanceMatrix;
    void attrs;
    for (const rec of this.recs) {
      const { p, u, item } = rec;
      planetPosition(u, p, time, moving, _p);
      item.pos.copy(_p);
      const dimmed = this.filter !== null && !this.filter.includes(p.status);
      const hidden = false;
      const focusFade = focusUniverse !== null && u.name !== focusUniverse;
      // In the galaxy view the done belt is not drawn (it only adds noise).
      const showBelt = !p.belt || focusUniverse === u.name || camPos.distanceToSquared(item.pos) < 90000;
      const s = hidden || !showBelt ? 0 : p.size * (this.hoverId === p.id ? 1.25 : 1);
      _s.setScalar(s);
      _m.compose(_p, _q.identity(), _s);
      this.planets.setMatrixAt(rec.idx, _m);
      const pulse = p.status === 'waiting' || p.status === 'failed' ? 0.55 + 0.45 * Math.sin(time * 3 + rec.phase) : p.status === 'ready' ? 0.35 : p.status === 'working' ? 0.18 : 0;
      this.glowAttr.array[rec.idx] = moving ? pulse : (pulse > 0 ? 0.4 : 0);
      this.dimAttr.array[rec.idx] = dimmed ? 0.14 : focusFade ? 0.55 : 1;
      // Decorations only near the camera.
      const near = camPos.distanceToSquared(item.pos) < NEAR * NEAR && !p.belt;
      if (near && (p.status === 'ready' || p.status === 'review' || p.status === 'failed' || p.status === 'waiting' || p.status === 'plan') && !dimmed) {
        _e.set(Math.PI / 2 + 0.25, time * 0.5 + rec.phase, 0);
        _q.setFromEuler(_e);
        const hs = p.size * (p.status === 'failed' ? 2.25 : 1.95) * (1 + (p.status === 'waiting' && moving ? 0.06 * Math.sin(time * 4 + rec.phase) : 0));
        _m.compose(_p, _q, _s.setScalar(hs));
        this.halos.setMatrixAt(halo, _m);
        this.halos.setColorAt(halo, _c.copy(this.pal.status[p.status]));
        halo++;
      }
      if (near && rec.task && rec.task.app) {
        const prog = rec.task.app.split('/')[0].toLowerCase();
        _e.set(Math.PI / 2 - 0.5, time * 0.35 + rec.phase, 0.3);
        _q.setFromEuler(_e);
        _m.compose(_p, _q, _s.setScalar(p.size * 2.7));
        this.rings.setMatrixAt(ring, _m);
        this.rings.setColorAt(ring, _c.copy(this.pal.program[prog] ?? this.pal.fg));
        ring++;
        const shape = PROGRAM_SHAPE[prog] ?? 'sphere';
        const mk = this.marks[shape];
        const ang = time * 0.9 + rec.phase;
        // The marker rides on the ring: same tilt, same radius.
        _p.set(Math.cos(ang) * p.size * 2.7, 0, Math.sin(ang) * p.size * 2.7).applyQuaternion(_q).add(item.pos);
        _m.compose(_p, _qi.identity(), _s.setScalar(p.size * 0.55));
        mk.setMatrixAt(markCount[shape], _m);
        mk.setColorAt(markCount[shape], _c.copy(this.pal.program[prog] ?? this.pal.fg));
        markCount[shape]++;
      }
    }
    this.halos.count = halo;
    this.rings.count = ring;
    for (const [k, m] of Object.entries(this.marks)) m.count = markCount[k];
    this.planets.instanceMatrix.needsUpdate = true;
    this.glowAttr.needsUpdate = true;
    this.dimAttr.needsUpdate = true;
    for (const m of [this.halos, this.rings, ...Object.values(this.marks)]) {
      m.instanceMatrix.needsUpdate = true;
      if (m.instanceColor) m.instanceColor.needsUpdate = true;
    }
    if (this.colorsDirty) {
      this.recs.forEach((rec) => {
        _c.copy(this.pal.status[rec.p.status]);
        if (this.hoverId === rec.p.id) _c.lerp(this.pal.fgBright, 0.35);
        this.planets.setColorAt(rec.idx, _c);
      });
      if (this.planets.instanceColor) this.planets.instanceColor.needsUpdate = true;
      this.colorsDirty = false;
    }
    this.updateMoons(time, group);
    this.updateBeacons(time, moving);
    this.updateFx(time * 1000, group);
  }

  private updateMoons(time: number, group: Group): void {
    void group;
    let mi = 0;
    let ti = 0;
    const now = performance.now() / 1000;
    for (const [key, m] of this.moonRecs) {
      let fade = 1;
      if (m.leaving !== null) {
        fade = 1 - (now - m.leaving) / 0.8;
        if (fade <= 0) { this.moonRecs.delete(key); continue; }
      }
      const fadeIn = Math.min(1, (now - m.born) / 0.5);
      const base = m.planetId === 'quasar' ? ZERO : this.byId.get(m.planetId)?.item.pos;
      if (!base) { m.item.radius = 0; continue; }
      const moving = !this.simple;
      const a = m.angle0 + (moving ? time * m.speed : 0);
      const run = m.sa.state === 'running';
      const quiet = m.sa.state === 'quiet';
      const tilt = 0.3;
      m.item.pos.set(
        base.x + Math.cos(a) * m.radius,
        base.y + Math.sin(a) * m.radius * Math.sin(tilt) + 0.3,
        base.z + Math.sin(a) * m.radius * Math.cos(tilt),
      );
      m.item.radius = 0.9;
      const prog = m.sa.kind;
      const colr = this.pal.program[prog] ?? this.pal.fg;
      const s = (quiet ? 0.7 : 1) * fade * fadeIn * (m.planetId === 'quasar' ? 1.8 : 1);
      _m.compose(m.item.pos, _q.identity(), _s.setScalar(s));
      this.moons.setMatrixAt(mi, _m);
      _c.copy(colr);
      if (quiet) _c.lerp(this.pal.muted, 0.6);
      this.moons.setColorAt(mi, _c);
      mi++;
      if (run && moving) {
        for (let k = 1; k <= 5 && ti < this.moonCap * 6; k++) {
          const a2 = a - k * 0.16;
          _p.set(
            base.x + Math.cos(a2) * m.radius,
            base.y + Math.sin(a2) * m.radius * Math.sin(tilt) + 0.3,
            base.z + Math.sin(a2) * m.radius * Math.cos(tilt),
          );
          _m.compose(_p, _q.identity(), _s.setScalar(fade * (1 - k / 6) * (m.planetId === 'quasar' ? 1.8 : 1)));
          this.tails.setMatrixAt(ti, _m);
          this.tails.setColorAt(ti, _c.copy(colr));
          ti++;
        }
      }
    }
    this.moons.count = mi;
    this.tails.count = ti;
    this.moons.instanceMatrix.needsUpdate = true;
    this.tails.instanceMatrix.needsUpdate = true;
    if (this.moons.instanceColor) this.moons.instanceColor.needsUpdate = true;
    if (this.tails.instanceColor) this.tails.instanceColor.needsUpdate = true;
    this.moonItems.length = 0;
    for (const m of this.moonRecs.values()) if (m.leaving === null) this.moonItems.push(m.item);
  }

  private updateBeacons(time: number, moving: boolean): void {
    const pos = this.beacons.geometry.getAttribute('position') as BufferAttribute | undefined;
    const sp = this.beaconSparks.geometry.getAttribute('position') as BufferAttribute | undefined;
    if (!pos || !sp) return;
    this.beaconRecs.forEach((r, i) => {
      const p = r.item.pos;
      pos.setXYZ(i * 2, p.x, p.y, p.z);
      pos.setXYZ(i * 2 + 1, 0, 0, 0);
      const k = moving ? (time * (r.p.status === 'waiting' ? 0.55 : 0.35) + (hash(r.p.id) % 100) / 100) % 1 : 1;
      sp.setXYZ(i, p.x * (1 - k), p.y * (1 - k), p.z * (1 - k));
    });
    pos.needsUpdate = true;
    sp.needsUpdate = true;
    this.beaconSparks.visible = moving && this.beaconRecs.length > 0;
  }

  private updateFx(nowMs: number, group: Group): void {
    void nowMs;
    const now = performance.now();
    for (let i = this.fxRings.length - 1; i >= 0; i--) {
      const f = this.fxRings[i];
      const k = (now - f.t0) / f.dur;
      if (k >= 1) {
        group.remove(f.mesh);
        f.mesh.geometry.dispose();
        (f.mesh.material as MeshBasicMaterial).dispose();
        this.fxRings.splice(i, 1);
      } else {
        f.mesh.scale.setScalar(1 + k * 2.6);
        (f.mesh.material as MeshBasicMaterial).opacity = 0.9 * (1 - k);
      }
    }
  }

  stats(): { planets: number; moons: number; halos: number; rings: number } {
    return { planets: this.recs.length, moons: this.moons.count, halos: this.halos.count, rings: this.rings.count };
  }

  /** Live moons in a state, for the checks. */
  moonStates(): { key: string; state: string; leaving: boolean }[] {
    return [...this.moonRecs.values()].map((m) => ({ key: m.key, state: m.sa.state, leaving: m.leaving !== null }));
  }
}

