/* The world: the Galaxy of the board, flown by the operator's ship.
   `createWorld` assembles the parts (nebulae, planets, Quasar, links, the
   ship), owns the camera (autopilot orbit rig, manual ship, warp flights),
   picks things under the pointer and draws the labels. The page talks to it
   through this interface only. */

import {
  NoToneMapping, Color, Group, Mesh, MeshBasicMaterial, PerspectiveCamera, Quaternion, Scene,
  TorusGeometry, Vector3, WebGLRenderer, HalfFloatType, Line, BufferGeometry, LineDashedMaterial,
} from 'three';
import { BloomEffect, EffectComposer, EffectPass, RenderPass } from 'postprocessing';
import type { Entity, GalaxyView, Level, Link, Status, Usage, CoordinatorState } from '../types';
import { Backdrop } from './backdrop';
import { Bodies, type Item } from './bodies';
import {
  aimedAt, clampPitch, CRUISE_DEFAULT, flightDuration, forwardOf, newRig, newShip, rigFromCamera, rigPosition,
  shipFromCamera, shipQuaternion, stepShip, wheelCruise, type RigState, type ShipState,
} from './flight';
import { Labels, type LabelCandidate } from './labels';
import { computeLayout, QUASAR_R, type Layout } from './layout';
import { Links } from './links';
import { Nebulae } from './nebulae';
import { DEFAULT_COLORS, makePalette, type Palette } from './palette';
import { Quasar } from './quasar';
import { Ship, WarpTunnel } from './ship';
import { t } from '../i18n';
import { scaleK } from '../scale';
import { clamp, firstSentence } from '../util/format';

export interface WorldHooks {
  /** A thing was clicked in the autopilot view: the page decides what that means. */
  onPick: (e: Entity) => void;
  /** The crosshair dive in manual flight (click on what is aimed at). */
  onDive: (e: Entity) => void;
  onEmpty: () => void;
  onLink: (l: Link) => void;
  onConnect: (from: string, to: string) => void;
  onHover: (e: Entity | null) => void;
  onLevel: (level: Level, universe: string | null) => void;
  onEdge: (on: boolean) => void;
  onLock: (on: boolean) => void;
  onFlight: (phase: 'idle' | 'accel' | 'warp' | 'decel', target: Entity | 'overview' | null) => void;
  onAutoSimple: () => void;
  onAim: (e: Entity | null, hold: number) => void;
  onSpeed: (v: number, cruise: number) => void;
  onMode: (m: 'auto' | 'manual') => void;
}

export interface WorldData {
  view: GalaxyView;
  limits: Usage | null;
  coordinator: CoordinatorState | null;
  attention: number;
  fresh: Record<string, number>;
  showQuiet: boolean;
  filter: Status[] | null;
}

export type Dest =
  | { kind: 'overview' }
  | { kind: 'entity'; entity: Entity }
  | { kind: 'pose'; pose: Pose };

/** A camera pose to fly to: where it looks from and at, and what it follows on arrival. */
export interface Pose { pos: Vector3; look: Vector3; rig: RigState; follow: string | null; enter: string | null; leave: boolean }

export interface WorldOptions {
  canvas: HTMLCanvasElement;
  labelsEl: HTMLElement;
  warpEl: HTMLElement | null;
  simple: boolean;
  bloom: boolean;
  autoSimple: boolean;
  sensitivity: number;
  invertY: boolean;
}

interface Proj { item: Item; x: number; y: number; depth: number; rpx: number }

interface Flight {
  t0: number;
  dur: number;
  fromPos: Vector3;
  fromLook: Vector3;
  warp: boolean;
  dest: Dest;
  resolve: () => void;
  phase: string;
}

const FOV = 45;
const NEBULA_MIN = 14;

const smooth = (a: number, b: number, x: number): number => {
  const t = clamp((x - a) / (b - a), 0, 1);
  return t * t * (3 - 2 * t);
};
const easeInOut = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

export interface World {
  readonly webgl: boolean;
  hooks: WorldHooks;
  setData(d: WorldData): void;
  setPalette(colors: Record<string, string>, mode: 'dark' | 'light'): void;
  setSimple(v: boolean): void;
  setSelection(e: Entity | null): void;
  setFilter(f: Status[] | null): void;
  setLinkMode(v: boolean): void;
  setDropTarget(e: Entity | null): void;
  setSensitivity(v: number, invert: boolean): void;
  setMode(m: 'auto' | 'manual', opts?: { lock?: boolean }): void;
  flyTo(d: Dest, opts?: { instant?: boolean }): Promise<void>;
  /** Leaves the nebula we are inside and looks at the Galaxy again. */
  leaveNebula(opts?: { instant?: boolean }): Promise<void>;
  cameraState(): CamState;
  restoreCamera(s: CamState, opts?: { instant?: boolean }): Promise<void>;
  entityAt(x: number, y: number, kinds?: Entity['kind'][]): Entity | null;
  /** Distance from the camera to a thing, in world units. */
  distanceTo(e: Entity): number | null;
  screenOf(e: Entity): { x: number; y: number; ok: boolean } | null;
  zoomBy(f: number): void;
  orbitBy(dx: number, dy: number): void;
  requestRender(): void;
  releaseLock(): void;
  resize(): void;
  debug(): Record<string, unknown>;
  /** Draws the link «to starts after from» as the gesture would (for checks and the menu). */
  connect(from: string, to: string): void;
  readonly mode: 'auto' | 'manual';
  readonly level: Level;
  readonly universe: string | null;
  readonly locked: boolean;
  readonly flying: boolean;
}

export interface CamState {
  mode: 'auto' | 'manual';
  universe: string | null;
  rig: { x: number; y: number; z: number; yaw: number; pitch: number; dist: number };
  ship: { x: number; y: number; z: number; yaw: number; pitch: number };
  follow: string | null;
}

export function webglSupported(): boolean {
  try {
    const c = document.createElement('canvas');
    return !!(c.getContext('webgl2') || c.getContext('webgl'));
  } catch { return false; }
}

export function createWorld(opts: WorldOptions): World {
  const hooks: WorldHooks = {
    onPick: () => {}, onDive: () => {}, onEmpty: () => {}, onLink: () => {}, onConnect: () => {}, onHover: () => {},
    onLevel: () => {}, onEdge: () => {}, onLock: () => {}, onFlight: () => {}, onAutoSimple: () => {}, onAim: () => {},
    onSpeed: () => {}, onMode: () => {},
  };
  const { canvas } = opts;
  let simple = opts.simple;
  let renderer: WebGLRenderer | null = null;
  if (webglSupported()) {
    try {
      renderer = new WebGLRenderer({ canvas, antialias: !simple, powerPreference: 'high-performance' });
      if (!renderer.getContext()) throw new Error('no context');
    } catch { renderer = null; }
  }
  const labels = new Labels(opts.labelsEl);
  if (!renderer) return noWorld(hooks);

  /* ── scene ── */
  const scene = new Scene();
  const camera = new PerspectiveCamera(FOV, 1, 0.5, 9000);
  camera.rotation.order = 'YXZ';
  scene.add(camera);
  let pal: Palette = makePalette(DEFAULT_COLORS, 'dark');
  scene.background = pal.bg.clone();
  renderer.toneMapping = NoToneMapping;
  renderer.toneMappingExposure = 1;
  renderer.setPixelRatio(simple ? 1 : Math.min(devicePixelRatio || 1, 1.75));

  const root = new Group();
  scene.add(root);
  const backdrop = new Backdrop();
  const nebulae = new Nebulae();
  const bodies = new Bodies();
  const quasar = new Quasar();
  const links = new Links();
  const ship = new Ship();
  const warp = new WarpTunnel();
  const fxGroup = new Group();
  root.add(backdrop.group, nebulae.points, bodies.group, quasar.group, links.group, fxGroup);
  camera.add(ship.group, warp.lines);

  const selRing = new Mesh(new TorusGeometry(1, 0.06, 6, 40), new MeshBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.9, depthWrite: false }));
  selRing.visible = false;
  selRing.renderOrder = 9;
  root.add(selRing);

  /* ── composer (bloom for the neon) ── */
  let composer: EffectComposer | null = null;
  let bloomEffect: BloomEffect | null = null;
  const useBloom = (): boolean => opts.bloom && !simple && pal.mode === 'dark';
  function buildComposer(): void {
    if (!renderer) return;
    composer?.dispose();
    composer = null;
    if (!useBloom()) return;
    composer = new EffectComposer(renderer, { frameBufferType: HalfFloatType });
    composer.addPass(new RenderPass(scene, camera));
    bloomEffect = new BloomEffect({
      intensity: 1.15, luminanceThreshold: 0.72, luminanceSmoothing: 0.25, mipmapBlur: true, radius: 0.72,
    });
    composer.addPass(new EffectPass(camera, bloomEffect));
  }

  /* ── state ── */
  let data: WorldData | null = null;
  let layout: Layout = computeLayout({ label: '', ok: true, tasks: [], projects: [], links: [] });
  const bound = (): number => layout.bound;
  let sig = '';
  let universeSig = '';
  let mode: 'auto' | 'manual' = 'auto';
  let level: Level = 'galaxy';
  let focusUniverse: string | null = null;
  let selected: Entity | null = null;
  let hovered: Entity | null = null;
  let followKey: string | null = null;
  let dropTarget: Entity | null = null;
  let linkMode = false;
  let sens = opts.sensitivity;
  let invertY = opts.invertY;
  let time = 0;
  let last = performance.now();
  let raf = 0;
  let pending = false;
  let locked = false;
  let rigT: RigState = newRig();
  const rigC: RigState = newRig();
  let shipS: ShipState = newShip();
  let flight: Flight | null = null;
  let edgeOn = false;
  const keys = new Set<string>();
  let mouseDX = 0;
  let mouseDY = 0;
  let boostOn = false;
  let aimed: Item | null = null;
  let aimHold = 0;
  let frames = 0;
  let updateMs = 0;
  let fpsStart = performance.now();
  let fps = 0;
  const frameTimes: number[] = [];
  let autoChecked = false;
  const started = performance.now();
  let projs: Proj[] = [];
  let universeItems: Item[] = [];
  let quasarItem: Item = mkItem('quasar', 'quasar', QUASAR_R * 1.6, 100);
  let scale = 800;
  let warpNow = 0;
  const tmp = new Vector3();
  const tmp2 = new Vector3();
  const tmpQ = new Quaternion();

  function mkItem(kind: Entity['kind'], id: string, radius: number, priority: number): Item {
    return {
      key: `${kind}|${id}`, entity: { kind, id }, pos: new Vector3(), radius,
      label: '', sub: '', priority, universe: '',
    } as Item;
  }

  /* ───────── data ───────── */

  function signature(d: WorldData): string {
    const v = d.view;
    return JSON.stringify([
      v.projects.map((p) => [p.name, p.quiet, p.active, p.folder, p.empty]),
      v.tasks.map((x) => [x.id, x.status, x.app, x.title, x.msg, x.progress, x.subagents.map((s) => [s.id, s.state, s.doing])]),
      v.links.map((l) => [l.from, l.to, l.kind]),
      d.showQuiet,
    ]);
  }

  function setData(d: WorldData): void {
    const prevStatus = new Map(layout.positions.entries());
    data = d;
    const s = signature(d);
    const same = s === sig;
    sig = s;
    if (!same) {
      const old = new Map<string, Status>();
      for (const [id, v] of prevStatus) old.set(id, v.p.status);
      layout = computeLayout(d.view, { showQuiet: d.showQuiet });
      const uSig = JSON.stringify(layout.universes.map((u) => [u.name, u.quiet, u.groupIndex, u.active, u.radius, Math.round(u.center.x), Math.round(u.center.z)]));
      if (uSig !== universeSig) {
        universeSig = uSig;
        nebulae.setSimple(simple);
        nebulae.build(layout.universes, pal);
        backdrop.build(layout.groups.length, pal, simple, layout.bound);
        nebulae.setFocus(focusUniverse);
      }
      bodies.build(layout, d.view.tasks, pal, d.coordinator?.subagents ?? [], performance.now() / 1000);
      links.build(d.view.links);
      universeItems = layout.universes.map((u) => {
        const it = mkItem('universe', u.name, u.radius, u.quiet ? 10 : 40);
        it.pos.set(u.center.x, u.center.y, u.center.z);
        it.label = u.name;
        it.sub = u.key;
        it.universe = u.name;
        return it;
      });
      for (const [id, v] of layout.positions) {
        const prev = old.get(id);
        if (prev && prev !== v.p.status && !simple) bodies.ringFx(id, v.p.status, performance.now(), fxGroup);
      }
      if (focusUniverse && !layout.byName.has(focusUniverse)) setFocusUniverse(null);
    }
    bodies.setFresh(d.fresh);
    bodies.setFilter(d.filter);
    links.setFilter((id) => {
      const r = layout.positions.get(id);
      return !r || !d.filter || d.filter.includes(r.p.status);
    });
    quasar.setCoordinator(d.coordinator);
    quasar.setLimits(d.limits);
    selRing.visible = !!selected;
    requestRender();
  }

  function setPalette(colors: Record<string, string>, m: 'dark' | 'light'): void {
    const next = makePalette(colors, m);
    const changedMode = next.mode !== pal.mode;
    pal = next;
    (scene.background as Color).copy(pal.bg);
    quasar.setPalette(pal);
    bodies.setPalette(pal);
    links.setPalette(pal);
    ship.setPalette(pal);
    warp.setColor(pal.fgBright);
    (selRing.material as MeshBasicMaterial).color.copy(pal.accent);
    if (data) {
      universeSig = '';
      sig = '';
      setData(data);
    }
    if (changedMode) buildComposer();
    requestRender();
  }

  /* ───────── selection, level ───────── */

  function setFocusUniverse(name: string | null): void {
    if (name === focusUniverse) return;
    focusUniverse = name;
    level = name ? 'nebula' : 'galaxy';
    nebulae.setFocus(name);
    hooks.onLevel(level, name);
    requestRender();
  }

  function entityPos(e: Entity, out = new Vector3()): Vector3 | null {
    switch (e.kind) {
      case 'quasar': return out.set(0, 0, 0);
      case 'universe': {
        const u = layout.byName.get(e.id);
        return u ? out.set(u.center.x, u.center.y, u.center.z) : null;
      }
      case 'planet': case 'moon': return bodies.positionOf(e.id, out);
    }
  }

  function universeOfEntity(e: Entity): string | null {
    if (e.kind === 'universe') return e.id;
    if (e.kind === 'planet') return layout.positions.get(e.id)?.u.name ?? null;
    if (e.kind === 'moon') {
      const task = e.id.split('#')[0];
      return task === 'quasar' ? null : (layout.positions.get(task)?.u.name ?? null);
    }
    return null;
  }

  function setSelection(e: Entity | null): void {
    selected = e;
    selRing.visible = !!e && e.kind !== 'quasar';
    requestRender();
  }

  /* ───────── destinations ───────── */

  function poseFor(d: Dest, out: Pose): Pose {
    const keepYaw = flight ? rigT.yaw : rigC.yaw;
    if (d.kind === 'pose') {
      return d.pose;
    }
    if (d.kind === 'overview') {
      out.rig = { focus: new Vector3(), yaw: keepYaw, pitch: 0.62, dist: Math.max(950, bound() * 1.55) };
      out.follow = null; out.enter = null; out.leave = true;
    } else {
      const e = (d as { entity: Entity }).entity;
      const p = entityPos(e) ?? new Vector3();
      const u = universeOfEntity(e);
      const lay = u ? layout.byName.get(u) : null;
      const outward = lay ? Math.atan2(lay.center.x, lay.center.z) : keepYaw;
      switch (e.kind) {
        case 'quasar':
          out.rig = { focus: p, yaw: keepYaw, pitch: 0.42, dist: 170 }; out.follow = null; out.enter = null; out.leave = true; break;
        case 'universe':
          out.rig = { focus: p, yaw: outward + 0.3, pitch: 0.55, dist: Math.max(70, (lay?.radius ?? 20) * 2.9) }; out.follow = null; out.enter = e.id; out.leave = false; break;
        case 'planet':
          out.rig = { focus: p, yaw: outward + 0.3, pitch: 0.5, dist: 36 }; out.follow = e.id; out.enter = u; out.leave = false; break;
        case 'moon':
          out.rig = { focus: p, yaw: outward + 0.3, pitch: 0.4, dist: 15 }; out.follow = e.id; out.enter = u; out.leave = !u; break;
      }
    }
    const rp = rigPosition(out.rig);
    out.pos = rp;
    out.look = out.rig.focus.clone();
    return out;
  }

  /** The live pose of the destination (planets move). */
  function livePose(d: Dest): Pose {
    return poseFor(d, { pos: new Vector3(), look: new Vector3(), rig: newRig(), follow: null, enter: null, leave: false });
  }

  function lookPoint(): Vector3 {
    camera.getWorldDirection(tmp);
    return camera.position.clone().addScaledVector(tmp, 80);
  }

  function snapTo(p: Pose): void {
    camera.position.copy(p.pos);
    camera.lookAt(p.look);
    rigT = { ...p.rig, focus: p.rig.focus.clone() };
    rigC.focus.copy(rigT.focus); rigC.yaw = rigT.yaw; rigC.pitch = rigT.pitch; rigC.dist = rigT.dist;
    followKey = p.follow;
    if (mode === 'manual') shipS = shipFromCamera(p.pos, p.look, shipS.cruise);
  }

  function applyLevelFor(p: Pose): void {
    if (p.enter) setFocusUniverse(p.enter);
    else if (p.leave) setFocusUniverse(null);
  }

  function flyTo(d: Dest, o: { instant?: boolean } = {}): Promise<void> {
    if (flight) finishFlight();
    const target = livePose(d);
    applyLevelFor(target);
    hooks.onFlight('idle', null);
    const instant = o.instant || simple || document.hidden;
    if (instant) {
      snapTo(target);
      requestRender();
      return Promise.resolve();
    }
    const dist = camera.position.distanceTo(target.pos);
    const warpOn = dist > 160;
    const dur = warpOn ? flightDuration(dist) : clamp(0.55 + dist / 380, 0.55, 1.0);
    const from = camera.position.clone();
    const look0 = lookPoint();
    if (mode === 'auto') followKey = null;
    return new Promise<void>((resolve) => {
      flight = { t0: performance.now(), dur: dur * 1000, fromPos: from, fromLook: look0, warp: warpOn, dest: d, resolve, phase: 'accel' };
      hooks.onFlight('accel', destTarget(d));
      ensureLoop();
    });
  }

  function destTarget(d: Dest): Entity | 'overview' | null {
    if (d.kind === 'entity') return d.entity;
    return d.kind === 'overview' ? 'overview' : null;
  }

  function finishFlight(): void {
    if (!flight) return;
    const f = flight;
    flight = null;
    const end = livePose(f.dest);
    snapTo(end);
    setWarp(0);
    camera.fov = FOV;
    camera.updateProjectionMatrix();
    hooks.onFlight('idle', null);
    f.resolve();
    requestRender();
  }

  function setWarp(w: number): void {
    warpNow = w;
    warp.set(w, time);
    if (opts.warpEl) opts.warpEl.style.opacity = String(Math.min(1, w * 0.9));
  }

  function stepFlight(now: number): void {
    const f = flight;
    if (!f) return;
    const k = clamp((now - f.t0) / f.dur, 0, 1);
    const e = easeInOut(k);
    const end = livePose(f.dest);
    camera.position.lerpVectors(f.fromPos, end.pos, e);
    tmp.lerpVectors(f.fromLook, end.look, easeInOut(Math.min(1, k * 1.15)));
    camera.lookAt(tmp);
    const w = f.warp ? smooth(0.04, 0.2, k) * (1 - smooth(0.74, 0.98, k)) : 0;
    camera.fov = FOV + 28 * w;
    camera.updateProjectionMatrix();
    setWarp(w);
    const phase = k < 0.16 ? 'accel' : k < 0.78 ? 'warp' : 'decel';
    if (phase !== f.phase) {
      f.phase = phase;
      hooks.onFlight(phase as 'accel' | 'warp' | 'decel', destTarget(f.dest));
    }
    if (k >= 1) finishFlight();
  }

  /* ───────── modes ───────── */

  function setMode(m: 'auto' | 'manual', o: { lock?: boolean } = {}): void {
    if (m === mode) { if (m === 'manual' && o.lock) requestLock(); return; }
    if (flight) finishFlight();
    if (m === 'manual') {
      mode = 'manual';
      followKey = null;
      camera.getWorldDirection(tmp);
      shipS = shipFromCamera(camera.position, camera.position.clone().add(tmp), shipS.cruise || CRUISE_DEFAULT);
      ship.setPose('nose');
      if (o.lock) requestLock();
      // The overview looks from outside the Galaxy: take the ship in, to where the boundary lets it fly.
      if (shipS.pos.length() > bound() * 0.95) {
        const pos = shipS.pos.clone().setLength(bound() * 0.82);
        pos.y = Math.min(pos.y, bound() * 0.35);
        const look = new Vector3(0, 0, 0);
        void flyTo({ kind: 'pose', pose: { pos, look, rig: rigFromCamera(pos, look), follow: null, enter: null, leave: false } });
      }
    } else {
      mode = 'auto';
      releaseLock();
      ship.setPose('parked');
      const focusE = selected ? entityPos(selected) : null;
      const focus = focusE && focusE.distanceTo(camera.position) < 2500 ? focusE : lookPoint();
      const r = rigFromCamera(camera.position, focus);
      rigT = r;
      rigC.focus.copy(r.focus); rigC.yaw = r.yaw; rigC.pitch = r.pitch; rigC.dist = r.dist;
      followKey = selected && (selected.kind === 'planet' || selected.kind === 'moon') ? selected.id : null;
    }
    hooks.onMode(mode);
    requestRender();
  }

  function requestLock(): void {
    try { void canvas.requestPointerLock?.(); } catch { /* not allowed */ }
  }
  function releaseLock(): void {
    if (document.pointerLockElement === canvas) document.exitPointerLock();
  }
  document.addEventListener('pointerlockchange', () => {
    locked = document.pointerLockElement === canvas;
    hooks.onLock(locked);
    if (!locked) { mouseDX = 0; mouseDY = 0; }
  });

  /* ───────── frame ───────── */

  function ensureLoop(): void {
    if (!simple) { if (!raf && !document.hidden) raf = requestAnimationFrame(loop); return; }
    requestRender();
  }

  function requestRender(): void {
    if (!simple) { ensureLoop(); return; }
    if (pending) return;
    pending = true;
    const go = (): void => { if (!pending) return; pending = false; frame(performance.now()); };
    requestAnimationFrame(go);
    setTimeout(go, 80);
  }

  function loop(now: number): void {
    raf = requestAnimationFrame(loop);
    frame(now);
  }

  function frame(now: number): void {
    // A frame's own timestamp can be earlier than the moment `last` was taken (a frame that began before the loop was
    // restarted): time never runs backwards, or the sparks of the links would look up a point before the first one.
    const dt = Math.max(0, Math.min(0.1, (now - last) / 1000));
    if (now - last >= 0 && now - last < 250) frameTimes.push(now - last);
    last = now;
    time += dt;
    frames++;
    if (now - fpsStart > 1000) { fps = Math.round((frames * 1000) / (now - fpsStart)); frames = 0; fpsStart = now; }
    if (opts.autoSimple && !autoChecked && !simple && now - started > 4500) {
      autoChecked = true;
      const sorted = frameTimes.slice().sort((a, b) => a - b);
      const median = sorted[Math.floor(sorted.length / 2)] ?? 0;
      if (sorted.length > 20 && median > 40) { setSimple(true); hooks.onAutoSimple(); }
    }
    const t0 = performance.now();
    update(dt, now);
    updateMs = updateMs * 0.9 + (performance.now() - t0) * 0.1;
    render();
  }

  function update(dt: number, now: number): void {
    const w = canvas.clientWidth || innerWidth;
    const h = canvas.clientHeight || innerHeight;
    scale = h / (2 * Math.tan((camera.fov * Math.PI) / 360));
    if (flight) stepFlight(now);
    else if (mode === 'manual') stepManual(dt);
    else stepRig(dt);
    camera.updateMatrixWorld(true);
    const speed01 = mode === 'manual' ? clamp(shipS.vel.length() / Math.max(1, shipS.cruise * 2.4), 0, 1) : flight ? 0.8 : 0.08;
    ship.update(dt, speed01, mode === 'manual' ? shipS.roll : 0, simple);
    warp.set(warpNow, time);
    nebulae.update(time, dt, scale, camera.position);
    bodies.update(time, camera.position, scale, focusUniverse, fxGroup);
    quasar.update(time, camera.position);
    links.setMoving(!simple);
    links.update(time, resolveRef);
    // The selection ring follows its thing.
    if (selected && selRing.visible) {
      const p = entityPos(selected, tmp2);
      if (p) {
        const r = selected.kind === 'planet' ? bodies.radiusOf(selected.id) : selected.kind === 'moon' ? 0.9 : selected.kind === 'universe' ? (layout.byName.get(selected.id)?.radius ?? 20) : 4;
        const rr = selected.kind === 'universe' ? r * 1.05 : Math.max(r * 1.7, (Math.max(1, camera.position.distanceTo(p)) / scale) * 11);
        selRing.position.copy(p);
        selRing.scale.setScalar(rr);
        selRing.quaternion.copy(camera.quaternion);
      } else selRing.visible = false;
    }
    project(w, h);
    if (mode === 'manual' && !flight) updateAim(dt, w, h);
    drawLabels(w, h);
  }

  function stepRig(dt: number): void {
    if (followKey) {
      const p = bodies.positionOf(followKey, tmp);
      if (p) rigT.focus.copy(p);
    }
    const k = 1 - Math.exp(-dt * 9);
    rigC.focus.lerp(rigT.focus, followKey ? 1 : k);
    rigC.yaw += (rigT.yaw - rigC.yaw) * k;
    rigC.pitch += (rigT.pitch - rigC.pitch) * k;
    rigC.dist += (rigT.dist - rigC.dist) * k;
    rigPosition(rigC, camera.position);
    camera.lookAt(rigC.focus);
  }

  function stepManual(dt: number): void {
    const input = {
      forward: (keys.has('KeyW') || keys.has('ArrowUp') ? 1 : 0) - (keys.has('KeyS') || keys.has('ArrowDown') ? 1 : 0),
      strafe: (keys.has('KeyD') || keys.has('ArrowRight') ? 1 : 0) - (keys.has('KeyA') || keys.has('ArrowLeft') ? 1 : 0),
      lift: (keys.has('Space') ? 1 : 0) - (keys.has('KeyC') || keys.has('KeyZ') ? 1 : 0),
      boost: boostOn,
      mx: mouseDX, my: mouseDY,
    };
    mouseDX = 0; mouseDY = 0;
    const r = stepShip(shipS, input, dt, { sensitivity: sens, invertY, bound: bound() });
    camera.position.copy(shipS.pos);
    shipQuaternion(shipS, tmpQ);
    camera.quaternion.copy(tmpQ);
    const fovWant = FOV + (boostOn && shipS.vel.length() > 20 ? 6 : 0);
    if (Math.abs(camera.fov - fovWant) > 0.05) { camera.fov += (fovWant - camera.fov) * (1 - Math.exp(-dt * 8)); camera.updateProjectionMatrix(); }
    if (r.edge !== edgeOn) { edgeOn = r.edge; hooks.onEdge(edgeOn); }
    hooks.onSpeed(r.speed, shipS.cruise);
    // Leaving a nebula by flying out.
    if (focusUniverse) {
      const u = layout.byName.get(focusUniverse);
      if (u && camera.position.distanceTo(tmp.set(u.center.x, u.center.y, u.center.z)) > u.radius * 4.2 + 90) setFocusUniverse(null);
    }
  }

  function resolveRef(ref: string): Vector3 | null {
    const p = bodies.positionOf(ref, new Vector3());
    if (p) return p;
    return null;
  }

  function project(w: number, h: number): void {
    projs = [];
    const items: Item[] = [quasarItem, ...universeItems, ...bodies.planetItems, ...bodies.moonItems];
    quasarItem.pos.set(0, 0, 0);
    camera.updateMatrixWorld();
    for (const item of items) {
      tmp.copy(item.pos).applyMatrix4(camera.matrixWorldInverse);
      const depth = -tmp.z;
      if (depth < 0.5) continue;
      tmp.copy(item.pos).project(camera);
      const x = (tmp.x * 0.5 + 0.5) * w;
      const y = (-tmp.y * 0.5 + 0.5) * h;
      const rpx = (item.radius * scale) / depth;
      projs.push({ item, x, y, depth, rpx });
    }
  }

  function projectPoint(v: Vector3): { x: number; y: number; ok: boolean } {
    tmp.copy(v).applyMatrix4(camera.matrixWorldInverse);
    if (-tmp.z < 0.5) return { x: 0, y: 0, ok: false };
    tmp.copy(v).project(camera);
    return { x: (tmp.x * 0.5 + 0.5) * (canvas.clientWidth || innerWidth), y: (-tmp.y * 0.5 + 0.5) * (canvas.clientHeight || innerHeight), ok: true };
  }

  /* ───────── picking ───────── */

  function pickAt(x: number, y: number, kinds?: Entity['kind'][]): Item | null {
    let best: Item | null = null;
    let bestScore = Infinity;
    for (const p of projs) {
      const k = p.item.entity.kind;
      if (kinds && !kinds.includes(k)) continue;
      const d = Math.hypot(p.x - x, p.y - y);
      let reach: number;
      let bias: number;
      // From the Galaxy a nebula is one target: the click dives into it, so the planets and moons inside count only
      // when the pointer is right on one of them (and moons not at all).
      const far = focusUniverse === null;
      switch (k) {
        case 'moon': reach = far ? 0 : Math.max(10, p.rpx * 1.8); bias = 0; break;
        case 'planet': reach = far ? Math.max(4, p.rpx * 1.1) : Math.max(11, p.rpx * 1.6); bias = 1; break;
        case 'quasar': reach = Math.max(18, p.rpx * 1.2); bias = 2; break;
        default: reach = Math.max(16, p.rpx); bias = 4; break;
      }
      if (d > reach * scaleK()) continue;
      // Universe: only when nothing smaller is under the pointer; deep inside a nebula prefer its planets.
      const score = bias * 1000 + d;
      if (score < bestScore) { bestScore = score; best = p.item; }
    }
    return best;
  }

  function updateAim(dt: number, w: number, h: number): void {
    void w; void h;
    camera.getWorldDirection(tmp2);
    const things = projs
      .filter((p) => {
        const k = p.item.entity.kind;
        if (k === 'universe') return focusUniverse === null;
        return true;
      })
      .map((p) => ({ item: p.item, pos: p.item.pos, radius: p.item.radius }));
    const hit = aimedAt(camera.position, tmp2, things, 0.05);
    const item = hit ? hit.item : null;
    if (item !== aimed) {
      aimed = item;
      aimHold = 0;
      hooks.onAim(item ? item.entity : null, 0);
    } else if (item && holdDown) {
      aimHold += dt;
      hooks.onAim(item.entity, Math.min(1, aimHold / 0.35));
      if (aimHold >= 0.35) { aimHold = -1e6; dive(item); }
    }
  }

  function dive(item: Item): void {
    hooks.onDive(item.entity);
  }

  /* ───────── labels ───────── */

  function drawLabels(w: number, h: number): void {
    const cands: LabelCandidate[] = [];
    const sel = selected;
    const attention = data?.attention ?? 0;
    for (const p of projs) {
      const it = p.item;
      const e = it.entity;
      const isSel = !!sel && sel.kind === e.kind && sel.id === e.id;
      const isHover = !!hovered && hovered.kind === e.kind && hovered.id === e.id;
      const isDrop = !!dropTarget && dropTarget.kind === e.kind && dropTarget.id === e.id;
      if (e.kind === 'quasar') {
        cands.push({
          key: it.key, kind: 'quasar', x: p.x, y: p.y, lift: Math.max(30, p.rpx * 1.1 + 22), priority: isSel ? 100 : 95, force: true,
          text: t('quasar.label'), badge: attention ? t('quasar.waits', { n: attention }) : '', selected: isSel, hovered: isHover, drop: isDrop,
        });
      } else if (e.kind === 'universe') {
        const u = layout.byName.get(e.id);
        if (!u) continue;
        const inside = focusUniverse === e.id;
        const proj = data?.view.projects.find((x) => x.name === e.id);
        const lag = proj?.lag && proj.lag.behind > 0 ? `main −${proj.lag.behind}` : '';
        cands.push({
          key: it.key, kind: 'universe', x: p.x, y: p.y, lift: Math.min(70, p.rpx * 0.9 + 12),
          priority: isSel ? 100 : isHover ? 92 : inside ? 85 : u.quiet ? 12 : 42 + Math.min(15, u.active),
          force: isSel || isHover,
          text: u.name, sub: u.key, badge: lag, selected: isSel, hovered: isHover, drop: isDrop,
          dim: focusUniverse !== null && !inside,
        });
      } else if (e.kind === 'planet') {
        const rec = layout.positions.get(e.id);
        if (!rec) continue;
        const near = p.depth < 330;
        if (!isSel && !isHover && !near) continue;
        if (focusUniverse !== null && rec.u.name !== focusUniverse && !isSel && !isHover) continue;
        if (rec.p.belt && !isSel && !isHover && p.depth > 120) continue;
        const task = data?.view.tasks.find((x) => x.id === e.id);
        const full = isSel || isHover || (p.depth < 60 && rec.p.status !== 'done');
        cands.push({
          key: it.key, kind: 'planet', x: p.x, y: p.y, lift: Math.max(10, p.rpx + 8),
          priority: (isSel ? 100 : isHover ? 92 : it.priority) - Math.min(30, p.depth / 20),
          force: isSel || isHover, status: rec.p.status, text: rec.p.tag,
          sub: full ? rec.p.title : '', line: full ? firstSentence(task?.msg || task?.board_label || '', 90) : undefined,
          full, selected: isSel, hovered: isHover, drop: isDrop, dim: data?.filter ? !data.filter.includes(rec.p.status) : false,
          badge: full && rec.p.app ? rec.p.app.split('/')[0] : '',
        });
      } else if (e.kind === 'moon') {
        if (!isSel && !isHover && p.depth > 70) continue;
        cands.push({
          key: it.key, kind: 'moon', x: p.x, y: p.y, lift: Math.max(8, p.rpx + 6), priority: isSel ? 100 : isHover ? 92 : 25,
          force: isSel || isHover, text: it.label, sub: '', selected: isSel, hovered: isHover,
        });
      }
    }
    labels.render(cands, simple ? 14 : 24, w, h);
  }

  /* ───────── render ───────── */

  function render(): void {
    if (!renderer) return;
    if (composer) composer.render(); else renderer.render(scene, camera);
  }

  function setSimple(v: boolean): void {
    if (simple === v) return;
    simple = v;
    renderer?.setPixelRatio(simple ? 1 : Math.min(devicePixelRatio || 1, 1.75));
    nebulae.setSimple(simple);
    bodies.setSimple(simple);
    quasar.setSimple(simple);
    buildComposer();
    universeSig = '';
    sig = '';
    if (data) setData(data);
    resize();
    if (simple) { if (raf) cancelAnimationFrame(raf); raf = 0; }
    ensureLoop();
    requestRender();
  }

  function resize(): void {
    if (!renderer) return;
    const w = innerWidth, h = innerHeight;
    renderer.setSize(w, h, false);
    camera.aspect = w / Math.max(1, h);
    camera.updateProjectionMatrix();
    composer?.setSize(w, h);
    requestRender();
  }

  /* ───────── pointer, wheel, keys ───────── */

  let down: { x: number; y: number; t: number; moved: number; dragLink: string | null; orbit: boolean } | null = null;
  let holdDown = false;
  let linkLine: Line | null = null;
  let linkFrom: Item | null = null;

  function startLinkDrag(from: Item): void {
    linkFrom = from;
    const geo = new BufferGeometry().setFromPoints([from.pos.clone(), from.pos.clone()]);
    linkLine = new Line(geo, new LineDashedMaterial({ color: pal.accent, transparent: true, opacity: 0.95, dashSize: 2, gapSize: 2 }));
    linkLine.computeLineDistances();
    fxGroup.add(linkLine);
  }
  function endLinkDrag(): void {
    if (linkLine) { fxGroup.remove(linkLine); linkLine.geometry.dispose(); (linkLine.material as LineDashedMaterial).dispose(); linkLine = null; }
    linkFrom = null;
  }

  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    if (flight && mode === 'auto') finishFlight();
    holdDown = true;
    const item = pickAt(e.offsetX, e.offsetY);
    let dragLink: string | null = null;
    if (mode === 'auto' && item?.entity.kind === 'planet' && (linkMode || e.shiftKey)) {
      dragLink = item.entity.id;
      startLinkDrag(item);
      canvas.setPointerCapture?.(e.pointerId);
    } else if (!locked) {
      canvas.setPointerCapture?.(e.pointerId);
    }
    down = { x: e.clientX, y: e.clientY, t: Date.now(), moved: 0, dragLink, orbit: !dragLink };
  });

  canvas.addEventListener('pointermove', (e) => {
    if (locked) {
      mouseDX += e.movementX;
      mouseDY += e.movementY;
      return;
    }
    if (down) {
      const dx = e.clientX - down.x, dy = e.clientY - down.y;
      down.moved += Math.abs(e.movementX) + Math.abs(e.movementY);
      down.x = e.clientX; down.y = e.clientY;
      if (down.dragLink && linkLine && linkFrom) {
        const ray = rayFromPointer(e.offsetX, e.offsetY);
        // Intersect the plane through the source planet.
        if (Math.abs(ray.dir.y) > 1e-4) {
          const tt = (linkFrom.pos.y - ray.origin.y) / ray.dir.y;
          if (tt > 0) {
            const end = ray.origin.clone().addScaledVector(ray.dir, tt);
            const arr = linkLine.geometry.getAttribute('position');
            arr.setXYZ(1, end.x, end.y, end.z);
            arr.needsUpdate = true;
            linkLine.computeLineDistances();
          }
        }
        requestRender();
        return;
      }
      if (down.orbit && !flight) {
        if (mode === 'manual') { mouseDX += dx; mouseDY += dy; }
        else {
          rigT.yaw -= dx * 0.006;
          rigT.pitch = clampPitch(rigT.pitch + dy * 0.005, -0.2, 1.45);
          requestRender();
        }
      }
      return;
    }
    // Hover, throttled.
    const now = performance.now();
    if (now - ((canvas as unknown as { _hv?: number })._hv ?? 0) < 60) return;
    (canvas as unknown as { _hv?: number })._hv = now;
    const item = pickAt(e.offsetX, e.offsetY);
    const next = item ? item.entity : null;
    const same = (!next && !hovered) || (!!next && !!hovered && next.kind === hovered.kind && next.id === hovered.id);
    if (!same) {
      hovered = next;
      bodies.setHover(next && next.kind === 'planet' ? next.id : null);
      canvas.style.cursor = next ? 'pointer' : mode === 'manual' ? 'crosshair' : 'grab';
      hooks.onHover(next);
      requestRender();
    }
  });

  const finishPointer = (e: PointerEvent, cancelled: boolean): void => {
    holdDown = false;
    aimHold = 0;
    if (!down) return;
    const d = down;
    down = null;
    try { canvas.releasePointerCapture?.(e.pointerId); } catch { /* not captured */ }
    if (d.dragLink) {
      const item = pickAt(e.offsetX, e.offsetY, ['planet']);
      endLinkDrag();
      if (!cancelled && item && item.entity.id !== d.dragLink) hooks.onConnect(d.dragLink, item.entity.id);
      requestRender();
      return;
    }
    if (cancelled || d.moved > 5 || Date.now() - d.t > 600) return;
    // A click.
    if (mode === 'manual') {
      const target = locked ? aimed : pickAt(e.offsetX, e.offsetY);
      if (target) { dive(target); return; }
      if (!locked) requestLock();
      return;
    }
    const item = pickAt(e.offsetX, e.offsetY);
    if (item) { hooks.onPick(item.entity); return; }
    const lk = links.pick(projectPoint, e.offsetX, e.offsetY, 9);
    if (lk) { hooks.onLink(lk.link); return; }
    hooks.onEmpty();
  };
  canvas.addEventListener('pointerup', (e) => finishPointer(e, false));
  canvas.addEventListener('pointercancel', (e) => finishPointer(e, true));

  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    if (mode === 'manual') {
      shipS.cruise = wheelCruise(shipS.cruise, e.deltaY);
      hooks.onSpeed(shipS.vel.length(), shipS.cruise);
    } else {
      const f = Math.pow(1.0016, e.deltaY);
      zoom(f);
    }
    requestRender();
  }, { passive: false });

  function zoom(f: number): void {
    const maxD = focusUniverse ? Math.max(260, (layout.byName.get(focusUniverse)?.radius ?? 30) * 8) : 1500;
    const minD = selected && (selected.kind === 'planet') ? 9 : selected?.kind === 'moon' ? 5 : focusUniverse ? 18 : 60;
    rigT.dist = clamp(rigT.dist * f, minD, maxD);
    requestRender();
  }

  function rayFromPointer(x: number, y: number): { origin: Vector3; dir: Vector3 } {
    const nx = (x / (canvas.clientWidth || innerWidth)) * 2 - 1;
    const ny = -(y / (canvas.clientHeight || innerHeight)) * 2 + 1;
    const origin = camera.position.clone();
    const dir = new Vector3(nx, ny, 0.5).unproject(camera).sub(origin).normalize();
    return { origin, dir };
  }

  const MOVE_KEYS = new Set(['KeyW', 'KeyA', 'KeyS', 'KeyD', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Space', 'KeyC', 'KeyZ']);
  window.addEventListener('keydown', (e) => {
    if (mode !== 'manual') return;
    const tg = e.target as HTMLElement | null;
    if (tg && tg.matches?.('input,textarea,select,[contenteditable]')) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (MOVE_KEYS.has(e.code)) { keys.add(e.code); e.preventDefault(); ensureLoop(); }
    if (e.key === 'Shift') { boostOn = true; ensureLoop(); }
    if (e.code === 'KeyX' && !e.repeat) { shipS.vel.set(0, 0, 0); }
  });
  window.addEventListener('keyup', (e) => {
    keys.delete(e.code);
    if (e.key === 'Shift') boostOn = false;
  });
  window.addEventListener('blur', () => { keys.clear(); boostOn = false; });
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) { if (raf) cancelAnimationFrame(raf); raf = 0; keys.clear(); if (flight) finishFlight(); }
    else { last = performance.now(); ensureLoop(); }
  });
  window.addEventListener('resize', resize);

  labels.onClick = (key) => {
    const p = projs.find((x) => x.item.key === key);
    if (p) hooks.onPick(p.item.entity);
  };

  /* ───────── api ───────── */

  buildComposer();
  setPalette(DEFAULT_COLORS, 'dark');
  quasar.setSimple(simple);
  bodies.setSimple(simple);
  backdrop.build(1, pal, simple);
  resize();
  ensureLoop();

  const world: World = {
    webgl: true,
    hooks,
    setData, setPalette, setSimple, setSelection,
    setFilter(f) { bodies.setFilter(f); links.setFilter((id) => { const r = layout.positions.get(id); return !r || !f || f.includes(r.p.status); }); requestRender(); },
    setLinkMode(v) { linkMode = v; canvas.classList.toggle('link-mode', v); },
    setDropTarget(e) { dropTarget = e; canvas.classList.toggle('drop-target', !!e); requestRender(); },
    setSensitivity(v, inv) { sens = v; invertY = inv; },
    setMode,
    flyTo,
    leaveNebula(o) { return flyTo({ kind: 'overview' }, o); },
    cameraState() {
      const rig = flight ? rigT : rigC;
      return {
        mode, universe: focusUniverse,
        rig: { x: rig.focus.x, y: rig.focus.y, z: rig.focus.z, yaw: rig.yaw, pitch: rig.pitch, dist: rig.dist },
        ship: { x: shipS.pos.x, y: shipS.pos.y, z: shipS.pos.z, yaw: shipS.yaw, pitch: shipS.pitch },
        follow: followKey,
      };
    },
    restoreCamera(s, o) {
      if (mode !== s.mode) setMode(s.mode);
      setFocusUniverse(s.universe);
      const rig: RigState = { focus: new Vector3(s.rig.x, s.rig.y, s.rig.z), yaw: s.rig.yaw, pitch: s.rig.pitch, dist: s.rig.dist };
      const pos = s.mode === 'manual' ? new Vector3(s.ship.x, s.ship.y, s.ship.z) : rigPosition(rig);
      const look = s.mode === 'manual' ? pos.clone().add(forwardOf(s.ship.yaw, s.ship.pitch).multiplyScalar(80)) : rig.focus.clone();
      const pose: Pose = { pos, look, rig, follow: s.follow, enter: null, leave: false };
      if (o?.instant || simple || document.hidden) { snapTo(pose); requestRender(); return Promise.resolve(); }
      return flyTo({ kind: 'pose', pose });
    },
    entityAt(x, y, kinds) {
      const it = pickAt(x, y, kinds);
      return it ? it.entity : null;
    },
    distanceTo(e) {
      const p = entityPos(e, new Vector3());
      return p ? camera.position.distanceTo(p) : null;
    },
    screenOf(e) {
      const p = entityPos(e, new Vector3());
      if (!p) return null;
      return projectPoint(p);
    },
    zoomBy(f) { if (mode === 'auto') zoom(f); },
    orbitBy(dx, dy) { if (mode === 'auto') { rigT.yaw -= dx; rigT.pitch = clampPitch(rigT.pitch + dy, -0.2, 1.45); requestRender(); } },
    requestRender,
    releaseLock,
    resize,
    debug() {
      const st = bodies.stats();
      const sorted = frameTimes.slice(-120).sort((a, b) => a - b);
      return {
        webgl: true, simple, mode, level, universe: focusUniverse,
        planets: layout.positions.size,
        livePlanets: [...layout.positions.values()].filter((v) => v.p.status !== 'done').length,
        statuses: [...new Set([...layout.positions.values()].filter((v) => v.p.status !== 'done').map((v) => v.p.status))].sort(),
        universes: layout.universes.length, quiet: layout.universes.filter((u) => u.quiet).length,
        links: (data?.view.links ?? []).map((l) => ({ from: l.from, to: l.to, kind: l.kind })),
        selected, hovered,
        updateMs: Math.round(updateMs * 100) / 100, fps, frameMs: sorted.length ? Math.round((sorted[Math.floor(sorted.length / 2)] ?? 0) * 10) / 10 : 0,
        labels: labels.count, collisions: labels.collisions, overlaps: labels.overlaps,
        limitArcs: quasar.limitArcs, limitCrit: quasar.limitCrit, quasar: { color: quasar.colorNow, state: quasar.stateNow },
        particles: nebulae.particleCount, lod: nebulae.lodSample, moons: bodies.moonStates(), halos: st.halos, rings: st.rings,
        camera: { x: Math.round(camera.position.x), y: Math.round(camera.position.y), z: Math.round(camera.position.z), fov: Math.round(camera.fov) },
        ship: { x: Math.round(shipS.pos.x), y: Math.round(shipS.pos.y), z: Math.round(shipS.pos.z), yaw: shipS.yaw, pitch: shipS.pitch, speed: Math.round(shipS.vel.length()), cruise: Math.round(shipS.cruise), roll: shipS.roll },
        flight: flight ? flight.phase : 'idle', locked, edge: edgeOn, bloom: !!composer, aimed: aimed ? aimed.entity : null,
        positionOf: (id: string) => { const p = bodies.positionOf(id); return p ? { x: p.x, y: p.y, z: p.z } : null; },
        project: (id: string) => { const p = bodies.positionOf(id); return p ? projectPoint(p) : null; },
      };
    },
    connect(from, to) { if (from && to && from !== to) hooks.onConnect(from, to); },
    get mode() { return mode; },
    get level() { return level; },
    get universe() { return focusUniverse; },
    get locked() { return locked; },
    get flying() { return !!flight; },
  };

  return world;
}

function noWorld(hooks: WorldHooks): World {
  const none = Promise.resolve();
  return {
    webgl: false, hooks, setData() {}, setPalette() {}, setSimple() {}, setSelection() {}, setFilter() {}, setLinkMode() {},
    setDropTarget() {}, setSensitivity() {}, setMode() {}, flyTo: () => none, leaveNebula: () => none,
    cameraState: () => ({ mode: 'auto', universe: null, rig: { x: 0, y: 0, z: 0, yaw: 0, pitch: 0, dist: 0 }, ship: { x: 0, y: 0, z: 0, yaw: 0, pitch: 0 }, follow: null }),
    restoreCamera: () => none, entityAt: () => null, distanceTo: () => null, screenOf: () => null, zoomBy() {}, orbitBy() {}, requestRender() {},
    releaseLock() {}, resize() {}, connect() {}, debug: () => ({ webgl: false }), mode: 'auto', level: 'galaxy', universe: null, locked: false, flying: false,
  };
}
