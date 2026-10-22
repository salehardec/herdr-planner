/* Shipyards («верфи») just outside the Galaxy's boundary. A yard is a small station lit from the quasar: a hub with a
   spire, a slowly turning ring on six spokes, and a berth boom with a lit pod for every container of a task. The
   state reads at a glance: a yard out of reach is dark and still; one in reach has lit windows and a core that
   brightens with the CPU; two thin scales around it show the CPU and the memory; the pods that hold forgotten
   containers burn in the warning colour. Everything of a yard is two meshes and a points object (one draw call
   each), the turning and the twinkle are a uniform, so the cost does not grow with the world. */
import {
  AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture, Color, ConeGeometry, CylinderGeometry, Group, Matrix4,
  Mesh, NormalBlending, OctahedronGeometry, Points, Quaternion, ShaderMaterial, Sprite, SpriteMaterial, TorusGeometry,
  BoxGeometry, Vector3,
} from 'three';
import { hash } from '../util/format';
import type { Palette } from './palette';

export interface DockEntry {
  id: string;
  ok: boolean;
  /** 0…1 of the CPUs busy. */
  load: number;
  /** 0…1 of the memory in use. */
  mem: number;
  containers: number;
  forgotten: number;
}

interface DockRec {
  id: string;
  pos: Vector3;
  ok: boolean;
  /** One light per container (at most MAX_LIGHTS), so many of them forgotten. */
  lights: number;
  warm: number;
  light: boolean;
  load: number;
  mem: number;
  ring: Group;
  halo: Sprite;
  hullMat: ShaderMaterial;
  lightMat: ShaderMaterial | null;
  geos: BufferGeometry[];
}

/** The yard's outermost scale, in world units: the camera frames this, the selection ring hugs it. */
export const DOCK_RADIUS = 33;
const RING = 20;
const MAX_LIGHTS = 16;
const SPIN = 0.045;

/** Where a yard sits: just outside the Galaxy's boundary, at an angle from its label, never closer to another yard
    than 120 units along the ring. Deterministic: the same list gives the same places. */
export function dockPositions(ids: string[], radius: number): Map<string, Vector3> {
  const out = new Map<string, Vector3>();
  const placed: number[] = [];
  const apart = 2 * Math.asin(Math.min(1, 60 / Math.max(1, radius)));
  const gap = (a: number, b: number): number => {
    const d = Math.abs(a - b) % (Math.PI * 2);
    return Math.min(d, Math.PI * 2 - d);
  };
  for (const id of [...ids].sort()) {
    let a = (hash(id) % 3600) / 3600 * Math.PI * 2;
    for (let k = 0; k < 3600 && placed.some((b) => gap(a, b) < apart); k++) a += apart;
    placed.push(a);
    out.set(id, new Vector3(Math.cos(a) * radius, 0, Math.sin(a) * radius));
  }
  return out;
}

/* ── the hull: flat-shaded facets lit from the quasar, emissive parts for the windows and the scales ── */

const HULL_VERT = /* glsl */ `
attribute vec3 aColor;
attribute float aEmis;
uniform vec3 uLight;
varying vec3 vVP;
varying vec3 vCol;
varying float vEmis;
varying vec3 vL;
void main() {
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  vVP = mv.xyz;
  vCol = aColor;
  vEmis = aEmis;
  vL = normalize(mat3(viewMatrix) * uLight);
  gl_Position = projectionMatrix * mv;
}`;

const HULL_FRAG = /* glsl */ `
uniform vec3 uBase;
uniform vec3 uGlow;
uniform float uOn;
uniform float uCore;
uniform float uAmbient;
uniform float uAlpha;
uniform float uOff;
uniform vec3 uBg;
varying vec3 vVP;
varying vec3 vCol;
varying float vEmis;
varying vec3 vL;
void main() {
  vec3 n = normalize(cross(dFdx(vVP), dFdy(vVP)));
  float ndl = max(dot(n, vL), 0.0);
  float rim = pow(1.0 - max(n.z, 0.0), 2.0);
  vec3 hull = uBase * vCol * (uAmbient + 0.85 * ndl) + uGlow * uOn * rim * 0.22;
  vec3 col = hull;
  if (vEmis > 0.0) {
    float k = vEmis > 1.5 ? uCore : vEmis;
    // lit: the colour over the ground behind (so a faint track is faint on a light ground too), brighter than 1 burns
    vec3 lit = mix(uBg, vCol, min(k, 1.0)) * max(k, 1.0);
    col = mix(hull * uOff, lit, uOn);
  }
  gl_FragColor = vec4(col, uAlpha);
}`;

/* ── the lights: one soft point per container, a twinkle in the shader ── */

const LIGHT_VERT = /* glsl */ `
attribute vec3 aColor;
attribute vec3 aWave;
uniform float uTime;
uniform float uScale;
uniform float uMotion;
varying vec3 vCol;
varying float vK;
void main() {
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  float dist = max(1.0, -mv.z);
  float wave = 0.5 + 0.5 * sin(uTime * aWave.y + aWave.x * 6.2831);
  vK = 1.0 - uMotion * (1.0 - wave) * aWave.z;
  vCol = aColor;
  gl_PointSize = clamp(2.2 * uScale / dist, 3.5, 34.0);
  gl_Position = projectionMatrix * mv;
}`;

const LIGHT_FRAG = /* glsl */ `
uniform float uSoft;
varying vec3 vCol;
varying float vK;
void main() {
  float d = length(gl_PointCoord - 0.5) * 2.0;
  if (d > 1.0) discard;
  float core = smoothstep(1.0, 0.0, d);
  gl_FragColor = vec4(vCol * (0.55 + 0.9 * core * core) * vK, mix(1.0, core, uSoft));
}`;

/** A merged, flat-shaded set of parts: positions and the two per-vertex attributes of the hull shader. */
class Parts {
  private pos: number[] = [];
  private col: number[] = [];
  private emis: number[] = [];
  private m = new Matrix4();
  private q = new Quaternion();
  private v = new Vector3();

  add(g: BufferGeometry, at: Vector3, rotY: number, col: Color, emis = 0): void {
    this.m.compose(at, this.q.setFromAxisAngle(this.v.set(0, 1, 0), rotY), new Vector3(1, 1, 1));
    const flat = g.index ? g.toNonIndexed() : g;
    flat.applyMatrix4(this.m);
    const p = flat.getAttribute('position');
    for (let i = 0; i < p.count; i++) {
      this.pos.push(p.getX(i), p.getY(i), p.getZ(i));
      this.col.push(col.r, col.g, col.b);
      this.emis.push(emis);
    }
    flat.dispose();
    if (flat !== g) g.dispose();
  }

  build(): BufferGeometry {
    const geo = new BufferGeometry();
    geo.setAttribute('position', new BufferAttribute(new Float32Array(this.pos), 3));
    geo.setAttribute('aColor', new BufferAttribute(new Float32Array(this.col), 3));
    geo.setAttribute('aEmis', new BufferAttribute(new Float32Array(this.emis), 1));
    return geo;
  }
}

let glowTex: CanvasTexture | null = null;
function glowTexture(): CanvasTexture {
  if (glowTex) return glowTex;
  const c = document.createElement('canvas');
  c.width = c.height = 64;
  const g = c.getContext('2d');
  if (g) {
    const r = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    r.addColorStop(0, 'rgba(255,255,255,1)');
    r.addColorStop(0.25, 'rgba(255,255,255,.5)');
    r.addColorStop(0.6, 'rgba(255,255,255,.12)');
    r.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = r;
    g.fillRect(0, 0, 64, 64);
  }
  glowTex = new CanvasTexture(c);
  return glowTex;
}

const WHITE = new Color(1, 1, 1);
const at = (x: number, y: number, z: number): Vector3 => new Vector3(x, y, z);

/** A flat arc scale in the plane of the yard, from the «north» (-z) clockwise. */
function arc(parts: Parts, radius: number, tube: number, share: number, col: Color, emis: number): void {
  const g = new TorusGeometry(radius, tube, 4, Math.max(3, Math.round(72 * share)), Math.PI * 2 * share);
  g.rotateX(Math.PI / 2);
  g.rotateY(Math.PI / 2);
  parts.add(g, at(0, 0, 0), 0, col, emis);
}

export class Docks {
  readonly group = new Group();
  readonly sprites = new Map<string, DockRec>();
  private motion = true;
  private selected: string | null = null;
  private lightMats: ShaderMaterial[] = [];

  /** Without motion (calm mode, prefers-reduced-motion) the ring stands still and the lights burn evenly. */
  setMotion(on: boolean): void {
    this.motion = on;
    for (const m of this.lightMats) m.uniforms.uMotion.value = on ? 1 : 0;
    if (!on) for (const r of this.sprites.values()) r.ring.rotation.y = 0;
  }

  private clear(): void {
    for (const rec of this.sprites.values()) {
      for (const g of rec.geos) g.dispose();
      rec.hullMat.dispose();
      rec.lightMat?.dispose();
      rec.halo.material.dispose();
    }
    this.sprites.clear();
    this.lightMats = [];
    for (const o of [...this.group.children]) this.group.remove(o);
  }

  build(entries: DockEntry[], pal: Palette, radius = 800): void {
    this.clear();
    const light = pal.mode === 'light';
    const pos = dockPositions(entries.map((e) => e.id), radius);
    for (const e of entries) {
      const at0 = pos.get(e.id) ?? new Vector3(radius, 0, 0);
      this.group.add(this.makeDock(e, at0, pal, light));
    }
  }

  private makeDock(e: DockEntry, p: Vector3, pal: Palette, light: boolean): Group {
    const root = new Group();
    root.position.copy(p);
    const live = pal.accent.clone().lerp(pal.status.working, 0.45);
    const warn = pal.status.waiting.clone().lerp(pal.status.failed, 0.3);
    const hot = pal.status.failed.clone();
    const cpuCol = e.load > 0.85 ? hot : live;
    const memCol = e.mem > 0.9 ? hot : pal.status.plan.clone();
    const hullBase = light ? pal.fg.clone().lerp(pal.muted, 0.6) : pal.muted.clone().lerp(pal.fgBright, 0.55);
    const glow = live.clone();
    const n = Math.min(MAX_LIGHTS, e.containers);
    const warm = Math.min(n, e.forgotten);

    // The still part: hub, keel, spire, the two scales.
    const still = new Parts();
    const metal = new Color(0.92, 0.92, 0.96);
    const dark = new Color(0.62, 0.62, 0.7);
    still.add(new CylinderGeometry(4.4, 4.8, 7, 12), at(0, 0, 0), 0, metal);
    still.add(new CylinderGeometry(2.7, 3.6, 3.4, 12), at(0, 5.2, 0), 0, dark);
    still.add(new CylinderGeometry(5.6, 5.6, 0.9, 12), at(0, -3.2, 0), 0, dark);
    still.add(new ConeGeometry(3.4, 5, 12), at(0, -6.4, 0), 0, dark);
    still.add(new CylinderGeometry(0.28, 0.28, 9, 5), at(0, 11, 0), 0, dark);
    still.add(new OctahedronGeometry(0.95), at(0, 16, 0), 0, live.clone().multiplyScalar(1.15), 1);
    still.add(new CylinderGeometry(4.55, 4.95, 0.9, 12), at(0, 0.6, 0), 0, live.clone().multiplyScalar(1.1), 2);
    if (e.ok) {
      // the scales: CPU outside, memory just inside it, each with a faint full track and a notch where it starts
      arc(still, RING + 11.5, 0.38, 1, cpuCol.clone().multiplyScalar(0.55), 0.22);
      arc(still, RING + 11.5, 0.7, Math.max(0.02, Math.min(1, e.load)), cpuCol.clone().multiplyScalar(1.1), 1);
      arc(still, RING + 9, 0.3, 1, memCol.clone().multiplyScalar(0.55), 0.22);
      arc(still, RING + 9, 0.5, Math.max(0.02, Math.min(1, e.mem)), memCol.clone().multiplyScalar(1.05), 1);
      still.add(new BoxGeometry(0.7, 0.7, 4.4), at(0, 0, -(RING + 10.2)), 0, WHITE.clone().multiplyScalar(0.9), 1);
    }

    // The turning part: the ring, six spokes with nodes, a berth boom and a pod per container.
    const ring = new Parts();
    const lightPos: number[] = [];
    const lightCol: number[] = [];
    const lightWave: number[] = [];
    ring.add(new TorusGeometry(RING, 1.5, 6, 48).rotateX(Math.PI / 2), at(0, 0, 0), 0, metal);
    ring.add(new TorusGeometry(RING, 0.42, 4, 48).rotateX(Math.PI / 2), at(0, 1.45, 0), 0, live.clone().multiplyScalar(0.9), 1);
    for (let i = 0; i < 6; i++) {
      const a = (i / 6) * Math.PI * 2;
      const len = RING - 4.8;
      const mid = 4.8 + len / 2;
      ring.add(new BoxGeometry(0.9, 0.9, len), at(Math.sin(a) * mid, 0, Math.cos(a) * mid), a, dark);
      ring.add(new BoxGeometry(3.6, 2.5, 3.6), at(Math.sin(a) * RING, 0, Math.cos(a) * RING), a, dark);
      ring.add(new BoxGeometry(2.4, 0.5, 0.5), at(Math.sin(a) * RING, 1.6, Math.cos(a) * RING), a, live.clone().multiplyScalar(0.9), 1);
    }
    for (let i = 0; i < n; i++) {
      // between the nodes where there is room: offset half a step from the spokes
      const a = ((i + 0.5) / n) * Math.PI * 2;
      const s = Math.sin(a);
      const c = Math.cos(a);
      const isWarm = i < warm;
      const tint = isWarm ? warn : live;
      ring.add(new BoxGeometry(0.55, 0.55, 5), at(s * (RING + 3.6), 0, c * (RING + 3.6)), a, dark);
      ring.add(new BoxGeometry(2.4, 1.7, 2.4), at(s * (RING + 7), 0, c * (RING + 7)), a, isWarm ? warn.clone().multiplyScalar(0.8) : metal, isWarm ? 0.6 : 0);
      lightPos.push(s * (RING + 7), 1.8, c * (RING + 7));
      lightCol.push(tint.r * 1.25, tint.g * 1.25, tint.b * 1.25);
      lightWave.push((hash(e.id + ':' + i) % 1000) / 1000, isWarm ? 2.2 : 1.1, isWarm ? 0.55 : 0.18);
    }

    const hullMat = new ShaderMaterial({
      vertexShader: HULL_VERT, fragmentShader: HULL_FRAG,
      uniforms: {
        uBase: { value: hullBase }, uGlow: { value: glow }, uOn: { value: e.ok ? 1 : 0 },
        uCore: { value: 0.55 + 0.9 * Math.max(0, Math.min(1, e.load)) }, uAmbient: { value: light ? 0.5 : 0.3 },
        uAlpha: { value: 1 }, uOff: { value: light ? 0.95 : 0.55 }, uBg: { value: pal.bg.clone() }, uLight: { value: p.clone().normalize().multiplyScalar(-1) },
      },
      transparent: !e.ok, depthWrite: true,
    });
    if (!e.ok) hullMat.uniforms.uAlpha.value = light ? 0.5 : 0.62;
    // an offline yard is the hull only, grey-blue and unlit
    if (!e.ok) hullMat.uniforms.uBase.value = pal.muted.clone().lerp(pal.bg, light ? 0.1 : 0.35);

    const geos: BufferGeometry[] = [still.build(), ring.build()];
    const stillMesh = new Mesh(geos[0], hullMat);
    const ringMesh = new Mesh(geos[1], hullMat);
    const ringGroup = new Group();
    ringGroup.add(ringMesh);
    root.add(stillMesh, ringGroup);

    let lightMat: ShaderMaterial | null = null;
    if (e.ok && n > 0) {
      const geo = new BufferGeometry();
      geo.setAttribute('position', new BufferAttribute(new Float32Array(lightPos), 3));
      geo.setAttribute('aColor', new BufferAttribute(new Float32Array(lightCol), 3));
      geo.setAttribute('aWave', new BufferAttribute(new Float32Array(lightWave), 3));
      lightMat = new ShaderMaterial({
        vertexShader: LIGHT_VERT, fragmentShader: LIGHT_FRAG,
        uniforms: { uTime: { value: 0 }, uScale: { value: 1000 }, uMotion: { value: this.motion ? 1 : 0 }, uSoft: { value: light ? 0 : 1 } },
        transparent: true, depthWrite: false, blending: light ? NormalBlending : AdditiveBlending,
      });
      geos.push(geo);
      const pts = new Points(geo, lightMat);
      pts.frustumCulled = false;
      pts.renderOrder = 3;
      ringGroup.add(pts);
      this.lightMats.push(lightMat);
    }
    ringMesh.frustumCulled = false;
    stillMesh.frustumCulled = false;

    // A halo behind: the beacon that keeps the yard findable from far away, and the glow of the core near.
    const halo = new Sprite(new SpriteMaterial({
      map: glowTexture(), color: e.ok ? live : pal.muted, transparent: true, depthWrite: false,
      blending: light ? NormalBlending : AdditiveBlending, opacity: e.ok ? (light ? 0.5 : 0.55) : 0.2,
    }));
    halo.renderOrder = 1;
    root.add(halo);

    const rec: DockRec = {
      id: e.id, pos: p.clone(), ok: e.ok, lights: e.ok ? n : 0, warm: e.ok ? warm : 0, light, load: e.load, mem: e.mem,
      ring: ringGroup, halo, hullMat, lightMat, geos,
    };
    this.sprites.set(e.id, rec);
    return root;
  }

  /** Where a thread from `from` meets the yard: on its outer scale, not through the hub. */
  linkEnd(id: string, from: Vector3, out: Vector3): Vector3 {
    const rec = this.sprites.get(id);
    if (!rec) return out.copy(from);
    out.copy(from).sub(rec.pos);
    const d = out.length();
    return out.multiplyScalar(d > 0 ? Math.min(1, DOCK_RADIUS * 0.8 / d) : 0).add(rec.pos);
  }

  /** The yard that is selected glows wider. */
  setSelected(id: string | null): void { this.selected = id; }

  /** Slow turn of the ring and the twinkle of the lights; a halo that stays visible at any distance. */
  update(time: number, camera: Vector3, scale: number): void {
    const spin = this.motion ? time * SPIN : 0;
    for (const rec of this.sprites.values()) {
      if (rec.ok) rec.ring.rotation.y = spin;
      const dist = Math.max(1, camera.distanceTo(rec.pos));
      // at least ~26 px across, and the core's glow when close
      const far = dist * 26 / Math.max(1, scale);
      const sel = rec.id === this.selected;
      rec.halo.scale.setScalar(Math.max(far * (sel ? 1.5 : 1), (sel ? 52 : 22) + (rec.ok ? 14 * rec.load : 0)));
      rec.halo.material.opacity = (rec.ok ? 0.55 : 0.2) * (sel ? 1.5 : 1);
      if (rec.lightMat) {
        rec.lightMat.uniforms.uTime.value = time;
        rec.lightMat.uniforms.uScale.value = scale;
      }
    }
  }
}
