/* Nebulae: one particle cloud per project (universe), all in a single draw call.
   The motion is in the vertex shader; LOD and the fade of the clouds we are
   not inside are per-particle attributes written a few times a second. */

import {
  AdditiveBlending, BufferAttribute, BufferGeometry, Color, NormalBlending, Points, ShaderMaterial, Vector3,
} from 'three';
import type { UniverseLayout } from './layout';
import type { Palette } from './palette';
import { hash } from '../util/format';

const VERT = /* glsl */ `
attribute vec3 aCenter;
attribute vec3 aColor;
attribute float aSize;
attribute float aSeed;
attribute float aRank;
attribute float aFade;
attribute float aLod;
uniform float uTime;
uniform float uScale;
uniform float uMotion;
varying vec3 vColor;
varying float vAlpha;
void main() {
  float ang = uTime * uMotion * (0.05 + 0.05 * fract(aSeed * 7.0));
  float c = cos(ang), s = sin(ang);
  vec3 p = position;
  p.xz = mat2(c, -s, s, c) * p.xz;
  p *= 1.0 + 0.045 * sin(uTime * uMotion * 0.6 + aSeed * 20.0);
  vec4 mv = modelViewMatrix * vec4(aCenter + p, 1.0);
  float vis = step(aRank, aLod);
  float dist = max(1.0, -mv.z);
  gl_PointSize = vis * clamp(aSize * uScale / dist, 1.0, 72.0);
  gl_Position = projectionMatrix * mv;
  vColor = aColor;
  vAlpha = aFade * vis;
}`;

const FRAG = /* glsl */ `
uniform float uOpacity;
varying vec3 vColor;
varying float vAlpha;
void main() {
  float d = length(gl_PointCoord - 0.5);
  float a = smoothstep(0.5, 0.0, d);
  gl_FragColor = vec4(vColor, a * a * vAlpha * uOpacity * 0.8);
}`;

interface Range { start: number; count: number; u: UniverseLayout }

export class Nebulae {
  readonly points: Points;
  private geo = new BufferGeometry();
  private mat: ShaderMaterial;
  private ranges: Range[] = [];
  private fade = new Float32Array(0);
  private lod = new Float32Array(0);
  private fadeTarget: number[] = [];
  private fadeNow: number[] = [];
  private lodAt = 0;
  private simple = false;

  constructor() {
    this.mat = new ShaderMaterial({
      vertexShader: VERT,
      fragmentShader: FRAG,
      transparent: true,
      depthWrite: false,
      blending: AdditiveBlending,
      uniforms: { uTime: { value: 0 }, uScale: { value: 800 }, uMotion: { value: 1 }, uOpacity: { value: 1 } },
    });
    this.points = new Points(this.geo, this.mat);
    this.points.frustumCulled = false;
    this.points.renderOrder = 2;
  }

  setSimple(simple: boolean): void {
    this.simple = simple;
    this.mat.uniforms.uMotion.value = simple ? 0 : 1;
  }

  /** Builds the clouds for a layout. Positions are stable per universe name. */
  build(universes: UniverseLayout[], pal: Palette): void {
    const per = universes.map((u) => this.countFor(u));
    const total = per.reduce((a, b) => a + b, 0);
    const pos = new Float32Array(total * 3);
    const ctr = new Float32Array(total * 3);
    const col = new Float32Array(total * 3);
    const size = new Float32Array(total);
    const seed = new Float32Array(total);
    const rank = new Float32Array(total);
    this.fade = new Float32Array(total).fill(1);
    this.lod = new Float32Array(total).fill(1);
    this.ranges = [];
    const tmp = new Color();
    const light = pal.mode === 'light';
    let at = 0;
    universes.forEach((u, ui) => {
      const n = per[ui];
      this.ranges.push({ start: at, count: n, u });
      const base = pal.arms[u.groupIndex % pal.arms.length];
      const dim = u.quiet ? 0.5 : 1;
      let s = hash(u.name) || 1;
      const rnd = (): number => { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; return s / 4294967296; };
      const gauss = (): number => (rnd() + rnd() + rnd() - 1.5) / 1.5;
      const order = Array.from({ length: n }, (_, i) => i);
      for (let i = n - 1; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [order[i], order[j]] = [order[j], order[i]]; }
      const rankOf = new Float32Array(n);
      order.forEach((idx, j) => { rankOf[idx] = (j + 1) / n; });
      for (let i = 0; i < n; i++) {
        const k = at + i;
        let x = 0, y = 0, z = 0, sz = 2.2, bright = 1;
        if (i === 0) {
          sz = Math.max(5, u.radius * 0.5);
          bright = 1.4;
        } else {
          const r = u.radius * 1.15 * Math.sqrt(rnd());
          const arm = i % 2 === 0 ? 0 : Math.PI;
          const a = r * 0.16 + arm + gauss() * 0.5;
          x = Math.cos(a) * r;
          z = Math.sin(a) * r;
          y = gauss() * u.radius * 0.1;
          sz = 1.4 + rnd() * 2.6 * (1 - r / (u.radius * 1.3) * 0.5);
          bright = 0.55 + rnd() * 0.5;
        }
        pos[k * 3] = x; pos[k * 3 + 1] = y; pos[k * 3 + 2] = z;
        ctr[k * 3] = u.center.x; ctr[k * 3 + 1] = u.center.y; ctr[k * 3 + 2] = u.center.z;
        tmp.copy(base);
        if (i % 5 === 0) tmp.lerp(pal.fgBright, 0.45);
        const m = bright * dim * (light ? 0.55 : 1);
        col[k * 3] = tmp.r * m; col[k * 3 + 1] = tmp.g * m; col[k * 3 + 2] = tmp.b * m;
        size[k] = sz;
        seed[k] = rnd();
        rank[k] = i === 0 ? 0 : rankOf[i];
      }
      at += n;
    });
    this.geo.dispose();
    this.geo = new BufferGeometry();
    this.geo.setAttribute('position', new BufferAttribute(pos, 3));
    this.geo.setAttribute('aCenter', new BufferAttribute(ctr, 3));
    this.geo.setAttribute('aColor', new BufferAttribute(col, 3));
    this.geo.setAttribute('aSize', new BufferAttribute(size, 1));
    this.geo.setAttribute('aSeed', new BufferAttribute(seed, 1));
    this.geo.setAttribute('aRank', new BufferAttribute(rank, 1));
    this.geo.setAttribute('aFade', new BufferAttribute(this.fade, 1));
    this.geo.setAttribute('aLod', new BufferAttribute(this.lod, 1));
    this.points.geometry = this.geo;
    this.mat.blending = light ? NormalBlending : AdditiveBlending;
    this.mat.uniforms.uOpacity.value = light ? 0.8 : 1;
    this.fadeTarget = universes.map(() => 1);
    this.fadeNow = universes.map(() => 1);
    this.lodAt = 0;
  }

  private countFor(u: UniverseLayout): number {
    if (this.simple) return 150;
    const base = u.quiet ? 200 : 420 + Math.min(360, u.active * 18);
    return Math.min(800, base);
  }

  /** Which nebula we are inside (others thin out); `null` for the whole galaxy. */
  setFocus(name: string | null): void {
    this.fadeTarget = this.ranges.map((r) => (name === null || r.u.name === name ? 1 : 0.18));
    // Inside a nebula its own cloud is thinner too, so the planets are seen.
    if (name !== null) {
      const i = this.ranges.findIndex((r) => r.u.name === name);
      if (i >= 0) this.fadeTarget[i] = 0.5;
    }
  }

  /** `scale` is the pixels per world unit at distance 1 (height / 2tan(fov/2)). */
  update(time: number, dt: number, scale: number, cam: Vector3): void {
    const u = this.mat.uniforms;
    u.uTime.value = time;
    u.uScale.value = scale;
    let fadeDirty = false;
    const k = 1 - Math.exp(-dt * 5);
    for (let i = 0; i < this.ranges.length; i++) {
      const diff = this.fadeTarget[i] - this.fadeNow[i];
      if (Math.abs(diff) > 0.002) {
        this.fadeNow[i] = this.simple ? this.fadeTarget[i] : this.fadeNow[i] + diff * k;
        const r = this.ranges[i];
        this.fade.fill(this.fadeNow[i], r.start, r.start + r.count);
        fadeDirty = true;
      }
    }
    if (fadeDirty) (this.geo.getAttribute('aFade') as BufferAttribute).needsUpdate = true;
    // LOD by the screen size of each nebula, four times a second.
    if (time - this.lodAt > 0.25) {
      this.lodAt = time;
      let lodDirty = false;
      for (const r of this.ranges) {
        const dist = Math.max(1, cam.distanceTo(new Vector3(r.u.center.x, r.u.center.y, r.u.center.z)));
        const px = (r.u.radius * scale) / dist;
        const lod = px > 70 ? 1 : px > 28 ? 0.7 : px > 12 ? 0.45 : 0.28;
        if (this.lod[r.start] !== lod) {
          this.lod.fill(lod, r.start, r.start + r.count);
          lodDirty = true;
        }
      }
      if (lodDirty) (this.geo.getAttribute('aLod') as BufferAttribute).needsUpdate = true;
    }
  }

  get particleCount(): number {
    return this.ranges.reduce((a, r) => a + r.count, 0);
  }

  get lodSample(): number[] {
    return this.ranges.map((r) => this.lod[r.start]);
  }

  dispose(): void {
    this.geo.dispose();
    this.mat.dispose();
  }
}
