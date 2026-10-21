/* The operator's ship, seen from behind (autopilot) or as the nose in front
   of the camera (manual flight), and the streaks of the warp tunnel. */
import {
  AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture, Color, ConeGeometry, DoubleSide, EdgesGeometry, Group, LineBasicMaterial,
  LineSegments, Mesh, MeshBasicMaterial, ShaderMaterial, Sprite, SpriteMaterial,
} from 'three';
import type { Palette } from './palette';

export type ShipPose = 'parked' | 'nose';

function flameTexture(): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = c.height = 64;
  const g = c.getContext('2d');
  if (g) {
    const grad = g.createRadialGradient(32, 32, 1, 32, 32, 30);
    grad.addColorStop(0, 'rgba(255,255,255,1)');
    grad.addColorStop(0.3, 'rgba(255,255,255,0.55)');
    grad.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 64);
  }
  return new CanvasTexture(c);
}

export class Ship {
  readonly group = new Group();
  private body = new Group();
  private flame: Sprite;
  private flameTex = flameTexture();
  private hullMat: MeshBasicMaterial;
  private wingMat: MeshBasicMaterial;
  private edgeMat!: LineBasicMaterial;
  private wingEdge!: LineBasicMaterial;
  private pose: ShipPose = 'parked';
  private blend = 0; // 0 parked … 1 nose
  private speed = 0;
  private bank = 0;

  constructor() {
    this.hullMat = new MeshBasicMaterial({ color: 0x2a2320 });
    this.wingMat = new MeshBasicMaterial({ color: 0x3a2f28, side: DoubleSide });
    const hullGeo = new ConeGeometry(0.55, 3.2, 5);
    const hull = new Mesh(hullGeo, this.hullMat);
    hull.rotation.x = -Math.PI / 2; // the cone's tip points to -Z
    this.body.add(hull);
    // Neon edges: the ship is drawn like the rest of the Galaxy, in light.
    this.edgeMat = new LineBasicMaterial({ color: 0xffffff });
    const edges = new LineSegments(new EdgesGeometry(hullGeo), this.edgeMat);
    edges.rotation.x = -Math.PI / 2;
    this.body.add(edges);
    const wingGeo = new BufferGeometry();
    wingGeo.setAttribute('position', new BufferAttribute(new Float32Array([
      0, 0, -0.4, 2.1, -0.15, 1.2, 0, 0, 1.3, // right
      0, 0, -0.4, -2.1, -0.15, 1.2, 0, 0, 1.3, // left
    ]), 3));
    wingGeo.computeVertexNormals();
    this.body.add(new Mesh(wingGeo, this.wingMat));
    this.wingEdge = new LineBasicMaterial({ color: 0xffffff });
    this.body.add(new LineSegments(new EdgesGeometry(wingGeo), this.wingEdge));
    this.flame = new Sprite(new SpriteMaterial({
      map: this.flameTex, transparent: true, depthWrite: false, blending: AdditiveBlending, color: 0xffaa55,
    }));
    this.flame.position.set(0, 0, 1.5);
    this.body.add(this.flame);
    this.group.add(this.body);
    this.group.renderOrder = 20;
    this.group.traverse((o) => { o.frustumCulled = false; });
  }

  setPalette(pal: Palette): void {
    this.hullMat.color.copy(pal.bg).lerp(pal.fg, 0.14);
    this.wingMat.color.copy(pal.bg).lerp(pal.accent, 0.22);
    this.edgeMat.color.copy(pal.accent).multiplyScalar(1.4);
    this.wingEdge.color.copy(pal.accent).lerp(pal.fgBright, 0.3).multiplyScalar(1.3);
    (this.flame.material as SpriteMaterial).color.copy(pal.accent).lerp(pal.status.working, 0.45);
  }

  setPose(p: ShipPose): void { this.pose = p; }

  /** `speed` 0…1 (relative to cruise), `bank` in radians, `dt` seconds. */
  update(dt: number, speed: number, bank: number, simple: boolean): void {
    const target = this.pose === 'nose' ? 1 : 0;
    this.blend += (target - this.blend) * (simple ? 1 : 1 - Math.exp(-dt * 7));
    this.speed += (speed - this.speed) * (1 - Math.exp(-dt * 6));
    this.bank += (bank - this.bank) * (1 - Math.exp(-dt * 8));
    const b = this.blend;
    // parked: below and ahead of the camera · nose: closer, lower, bigger.
    this.group.position.set(0, -1.55 - b * 0.9, -7.2 + b * 2.4);
    this.group.scale.setScalar(0.4 + b * 1.0);
    this.body.rotation.z = this.bank;
    this.body.rotation.x = 0.1 + b * 0.18;
    const f = 0.8 + this.speed * 2.2 + (simple ? 0 : 0.18 * Math.sin(performance.now() * 0.02));
    this.flame.scale.set(f, f, 1);
    (this.flame.material as SpriteMaterial).opacity = 0.35 + Math.min(1, this.speed) * 0.65;
  }

  dispose(): void {
    this.flameTex.dispose();
  }
}

const WARP_VERT = /* glsl */ `
attribute float aEnd;
attribute float aSeed;
uniform float uWarp;
uniform float uTime;
varying float vA;
void main() {
  float z = -4.0 - mod(aSeed * 90.0 + uTime * (60.0 + 140.0 * uWarp), 90.0);
  vec3 p = position;
  p.z = z - aEnd * (2.0 + 26.0 * uWarp);
  vA = (1.0 - aEnd) * smoothstep(-94.0, -14.0, z) * uWarp;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(p, 1.0);
}`;
const WARP_FRAG = /* glsl */ `
uniform vec3 uColor;
varying float vA;
void main() { gl_FragColor = vec4(uColor, vA * 0.9); }`;

/** Streaks around the camera that appear during the warp. */
export class WarpTunnel {
  readonly lines: LineSegments;
  private mat: ShaderMaterial;

  constructor(count = 500) {
    const pos = new Float32Array(count * 6);
    const end = new Float32Array(count * 2);
    const seed = new Float32Array(count * 2);
    for (let i = 0; i < count; i++) {
      const a = Math.random() * Math.PI * 2;
      const r = 3 + Math.random() * 16;
      const s = Math.random();
      for (let k = 0; k < 2; k++) {
        pos[i * 6 + k * 3] = Math.cos(a) * r;
        pos[i * 6 + k * 3 + 1] = Math.sin(a) * r;
        end[i * 2 + k] = k;
        seed[i * 2 + k] = s;
      }
    }
    const geo = new BufferGeometry();
    geo.setAttribute('position', new BufferAttribute(pos, 3));
    geo.setAttribute('aEnd', new BufferAttribute(end, 1));
    geo.setAttribute('aSeed', new BufferAttribute(seed, 1));
    this.mat = new ShaderMaterial({
      vertexShader: WARP_VERT, fragmentShader: WARP_FRAG, transparent: true, depthWrite: false, blending: AdditiveBlending,
      uniforms: { uWarp: { value: 0 }, uTime: { value: 0 }, uColor: { value: new Color('#ffffff') } },
    });
    this.lines = new LineSegments(geo, this.mat);
    this.lines.frustumCulled = false;
    this.lines.visible = false;
    this.lines.renderOrder = 30;
  }

  setColor(c: Color): void { this.mat.uniforms.uColor.value.copy(c); }

  set(warp: number, time: number): void {
    this.mat.uniforms.uWarp.value = warp;
    this.mat.uniforms.uTime.value = time;
    this.lines.visible = warp > 0.01;
  }
}
