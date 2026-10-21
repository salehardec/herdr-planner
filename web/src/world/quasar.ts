/* The Quasar: the Coordinator at the centre of the Galaxy. A neon sphere with a
   halo, two polar jets (its voice), and the subscription limits drawn as an
   accretion disk of arcs around it. */

import {
  AdditiveBlending, CanvasTexture, Color, DoubleSide, Group, Mesh, MeshBasicMaterial, PlaneGeometry,
  RingGeometry, ShaderMaterial, Sprite, SpriteMaterial, SphereGeometry, Texture, Vector3,
} from 'three';
import { QUASAR_R } from './layout';
import type { Palette } from './palette';
import { PROGRAM_COLOR_KEY } from './palette';
import type { AgentUsage, CoordinatorState, Usage } from '../types';

const clamp = (v: number, a: number, b: number): number => Math.max(a, Math.min(b, v));

const CORE_VERT = /* glsl */ `
varying vec3 vN;
varying vec3 vV;
void main() {
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  vN = normalize(normalMatrix * normal);
  vV = normalize(-mv.xyz);
  gl_Position = projectionMatrix * mv;
}`;
const CORE_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform float uPulse;
varying vec3 vN;
varying vec3 vV;
void main() {
  float f = pow(1.0 - max(dot(vN, vV), 0.0), 2.0);
  vec3 c = mix(uColor * 1.6, vec3(1.0), f * 0.55) * (1.0 + uPulse * 0.35);
  gl_FragColor = vec4(c, 1.0);
}`;

function glowTexture(): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = c.height = 128;
  const g = c.getContext('2d');
  if (g) {
    const grad = g.createRadialGradient(64, 64, 2, 64, 64, 62);
    grad.addColorStop(0, 'rgba(255,255,255,0.95)');
    grad.addColorStop(0.22, 'rgba(255,255,255,0.4)');
    grad.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = grad;
    g.fillRect(0, 0, 128, 128);
  }
  return new CanvasTexture(c);
}

function jetTexture(): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = 64;
  c.height = 256;
  const g = c.getContext('2d');
  if (g) {
    const grad = g.createLinearGradient(0, 256, 0, 0);
    grad.addColorStop(0, 'rgba(255,255,255,0.95)');
    grad.addColorStop(0.35, 'rgba(255,255,255,0.35)');
    grad.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 256);
    // Fade the sides so the plane reads as a beam, not a card.
    const side = g.createLinearGradient(0, 0, 64, 0);
    side.addColorStop(0, 'rgba(0,0,0,1)');
    side.addColorStop(0.5, 'rgba(0,0,0,0)');
    side.addColorStop(1, 'rgba(0,0,0,1)');
    g.globalCompositeOperation = 'destination-out';
    g.fillStyle = side;
    g.fillRect(0, 0, 64, 256);
  }
  return new CanvasTexture(c);
}

export class Quasar {
  readonly group = new Group();
  readonly disk = new Group();
  private core: Mesh;
  private coreMat: ShaderMaterial;
  private halo: Sprite;
  private jets: Mesh[] = [];
  private glow: Texture;
  private jetTex: Texture;
  private state: 'idle' | 'busy' | 'dialog' | 'error' = 'idle';
  private critical = false;
  private pal!: Palette;
  private simple = false;
  private ownMats: { dispose(): void }[] = [];
  private ownGeo: { dispose(): void }[] = [];

  constructor() {
    this.glow = glowTexture();
    this.jetTex = jetTexture();
    this.coreMat = new ShaderMaterial({
      vertexShader: CORE_VERT,
      fragmentShader: CORE_FRAG,
      uniforms: { uColor: { value: new Color('#dfa360') }, uPulse: { value: 0 } },
    });
    this.core = new Mesh(new SphereGeometry(QUASAR_R, 32, 24), this.coreMat);
    this.core.userData.quasar = true;
    this.group.add(this.core);
    this.halo = new Sprite(new SpriteMaterial({
      map: this.glow, transparent: true, depthWrite: false, blending: AdditiveBlending, opacity: 0.9,
    }));
    this.halo.scale.setScalar(150);
    this.group.add(this.halo);
    for (const dir of [1, -1]) {
      const m = new Mesh(new PlaneGeometry(10, 120), new MeshBasicMaterial({
        map: this.jetTex, transparent: true, depthWrite: false, blending: AdditiveBlending, opacity: 0.75, side: DoubleSide,
      }));
      m.position.y = dir * 68;
      if (dir < 0) m.rotation.z = Math.PI;
      const cross = m.clone();
      cross.rotation.y = Math.PI / 2;
      const holder = new Group();
      holder.add(m, cross);
      this.jets.push(holder as unknown as Mesh);
      this.group.add(holder);
    }
    this.group.add(this.disk);
  }

  setSimple(v: boolean): void { this.simple = v; for (const j of this.jets) j.visible = true; }

  setPalette(pal: Palette): void {
    this.pal = pal;
    this.applyState();
    if (this.usage) this.setLimits(this.usage);
  }

  private usage: Usage | null = null;

  setCoordinator(c: CoordinatorState | null): void {
    const next = !c ? 'idle' : c.error ? 'error' : c.dialog ? 'dialog' : c.busy || c.status === 'working' ? 'busy' : 'idle';
    if (next !== this.state) { this.state = next; this.applyState(); }
  }

  private applyState(): void {
    if (!this.pal) return;
    const p = this.pal;
    const col = this.state === 'error' ? p.status.failed : this.state === 'dialog' ? p.status.waiting : p.accent;
    this.coreMat.uniforms.uColor.value.copy(col);
    (this.halo.material as SpriteMaterial).color.copy(col);
    for (const j of this.jets) {
      for (const c of (j as unknown as Group).children) ((c as Mesh).material as MeshBasicMaterial).color.copy(col);
    }
  }

  /** One ring per subscription: the arc is the share of the window already spent. */
  setLimits(limits: Usage | null): void {
    this.usage = limits;
    for (const c of [...this.disk.children]) this.disk.remove(c);
    for (const d of this.ownGeo) d.dispose();
    for (const d of this.ownMats) d.dispose();
    this.ownGeo = [];
    this.ownMats = [];
    this.critical = false;
    if (!this.pal) return;
    const agents = limits?.agents ?? [];
    let slot = 0;
    const now = Date.now();
    for (const agent of agents) {
      const w = worstWindow(agent, now);
      if (!w) continue;
      const radius = 38 + slot * 7;
      const used = clamp(Number(w.percent || 0), 0, 1);
      const severity = w.severity === 'crit' ? 'crit' : w.severity === 'warn' ? 'warn' : 'ok';
      if (severity === 'crit') this.critical = true;
      const key = severity === 'crit' ? 'red' : severity === 'warn' ? 'yellow' : (PROGRAM_COLOR_KEY[agent.id] ?? 'accent');
      const color = new Color(this.pal.raw[key] ?? '#ffffff');
      const dim = agent.stale ? 0.45 : 1;
      const trackGeo = new RingGeometry(radius, radius + 1.1, 96, 1);
      const trackMat = new MeshBasicMaterial({ color: this.pal.muted, transparent: true, opacity: 0.2 * dim, side: DoubleSide, depthWrite: false });
      const track = new Mesh(trackGeo, trackMat);
      track.rotation.x = -Math.PI / 2 + 0.12;
      const arcGeo = new RingGeometry(radius, radius + 1.8, 96, 1, Math.PI / 2, Math.PI * 2 * Math.max(0.01, used));
      const arcMat = new MeshBasicMaterial({ color, transparent: true, opacity: 0.95 * dim, side: DoubleSide, depthWrite: false, blending: AdditiveBlending });
      const arc = new Mesh(arcGeo, arcMat);
      arc.rotation.x = -Math.PI / 2 + 0.12;
      arc.userData.agent = agent.id;
      arc.userData.severity = severity;
      this.disk.add(track, arc);
      this.ownGeo.push(trackGeo, arcGeo);
      this.ownMats.push(trackMat, arcMat);
      slot++;
    }
  }

  get limitArcs(): number { return this.disk.children.filter((c) => c.userData.agent).length; }
  get limitCrit(): boolean { return this.critical; }
  get colorNow(): string { return `#${this.coreMat.uniforms.uColor.value.getHexString()}`; }
  get stateNow(): string { return this.state; }

  /** `viewDir` is camera → quasar; the jets keep their length at any distance. */
  update(time: number, camPos: Vector3): void {
    const moving = !this.simple;
    const busy = this.state === 'busy';
    const pulse = moving && (busy || this.critical) ? 0.5 + 0.5 * Math.sin(time * 4.2) : 0;
    this.coreMat.uniforms.uPulse.value = pulse;
    const dist = camPos.length();
    const haloScale = 150 * (1 + pulse * 0.06);
    this.halo.scale.setScalar(haloScale);
    (this.halo.material as SpriteMaterial).opacity = this.state === 'dialog' ? 0.95 : busy ? 0.72 + 0.12 * pulse : 0.8;
    const jetOn = busy ? 1 : this.state === 'dialog' ? 0.6 : 0.35;
    for (const j of this.jets) {
      const g = j as unknown as Group;
      for (const c of g.children) {
        const mat = (c as Mesh).material as MeshBasicMaterial;
        mat.opacity = (0.35 + 0.4 * jetOn) * (moving ? 0.85 + 0.15 * Math.sin(time * 5 + g.id) : 1);
      }
      g.scale.set(1, (busy ? 1.15 : 1) * (moving ? 1 + 0.06 * Math.sin(time * 3.1) : 1), 1);
    }
    void dist;
    void clamp;
  }

  dispose(): void {
    this.glow.dispose();
    this.jetTex.dispose();
  }
}

function worstWindow(agent: AgentUsage, now: number): { percent: number; severity: string } | null {
  const windows = (agent.limits ?? []).filter((w) => !w.resets_at_ms || w.resets_at_ms > now);
  let best: { percent: number; severity: string } | null = null;
  for (const w of windows) if (!best || Number(w.percent) > Number(best.percent)) best = w;
  return best;
}
