/* Stars, the faint disc of the Galaxy with its spiral arms, distant galaxies
   (the other machines of the board) and the bridges between same projects. */
import {
  AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture, Color, DoubleSide, Group, Line, LineDashedMaterial,
  Mesh, MeshBasicMaterial, NormalBlending, PlaneGeometry, Points, PointsMaterial, Sprite, SpriteMaterial, Vector3,
} from 'three';
import { R_FAR, R_MIN } from './layout';
import type { Palette } from './palette';
import { hash } from '../util/format';

export function makeStars(simple: boolean, pal: Palette): Points {
  const n = simple ? 300 : 1500;
  const pos = new Float32Array(n * 3);
  for (let i = 0; i < n; i++) {
    const a = (hash('star' + i) / 4294967295) * Math.PI * 2;
    const r = 2600 + (hash('r' + i) % 1200);
    const y = ((hash('y' + i) % 2000) / 1000 - 1) * 0.8;
    const k = Math.sqrt(1 - y * y);
    pos[i * 3] = Math.cos(a) * r * k;
    pos[i * 3 + 1] = y * r;
    pos[i * 3 + 2] = Math.sin(a) * r * k;
  }
  const geo = new BufferGeometry();
  geo.setAttribute('position', new BufferAttribute(pos, 3));
  const stars = new Points(geo, new PointsMaterial({
    color: pal.mode === 'light' ? pal.fg : pal.fgBright, size: 1.9, sizeAttenuation: false, transparent: true,
    opacity: 0.8, depthWrite: false,
  }));
  stars.frustumCulled = false;
  stars.renderOrder = 0;
  return stars;
}

/** Soft spiral texture; `arms` strokes follow the same law as the layout (angle grows with radius). */
function discTexture(groups: number, pal: Palette, radius: number, size = 512): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const g = c.getContext('2d');
  if (g) {
    const half = size / 2;
    const scale = half / (radius + 60); // world units → px
    g.translate(half, half);
    const core = g.createRadialGradient(0, 0, 0, 0, 0, half * 0.5);
    core.addColorStop(0, 'rgba(255,255,255,0.16)');
    core.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = core;
    g.fillRect(-half, -half, size, size);
    const arms = Math.max(1, groups);
    for (let a = 0; a < arms; a++) {
      const col = pal.arms[a % pal.arms.length];
      const css = `${Math.round(col.r * 255)},${Math.round(col.g * 255)},${Math.round(col.b * 255)}`;
      for (let pass = 0; pass < 3; pass++) {
        g.beginPath();
        for (let r = 40; r <= radius; r += 6) {
          const theta = (a / arms) * Math.PI * 2 + (r - R_MIN) * 0.0105;
          // Canvas y is down; the world's z is the second axis of the disc.
          const x = Math.cos(theta) * r * scale;
          const y = Math.sin(theta) * r * scale;
          if (r === 40) g.moveTo(x, y); else g.lineTo(x, y);
        }
        g.strokeStyle = `rgba(${css},${0.05 + (2 - pass) * 0.03})`;
        g.lineWidth = (10 + pass * 9) * scale * 3;
        g.lineCap = 'round';
        g.stroke();
      }
    }
  }
  const tex = new CanvasTexture(c);
  tex.anisotropy = 4;
  return tex;
}

export class Backdrop {
  readonly group = new Group();
  private disc: Mesh | null = null;
  private stars: Points | null = null;

  build(groups: number, pal: Palette, simple: boolean, bound = 760): void {
    if (this.disc) { this.group.remove(this.disc); (this.disc.material as MeshBasicMaterial).map?.dispose(); (this.disc.material as MeshBasicMaterial).dispose(); this.disc.geometry.dispose(); }
    if (this.stars) { this.group.remove(this.stars); this.stars.geometry.dispose(); (this.stars.material as PointsMaterial).dispose(); }
    this.disc = new Mesh(
      new PlaneGeometry((bound + 60) * 2, (bound + 60) * 2),
      new MeshBasicMaterial({
        map: discTexture(groups, pal, bound), transparent: true, depthWrite: false, side: DoubleSide,
        blending: pal.mode === 'light' ? NormalBlending : AdditiveBlending, opacity: pal.mode === 'light' ? 0.34 : 0.17,
      }),
    );
    this.disc.rotation.x = -Math.PI / 2;
    this.disc.position.y = -2;
    this.disc.renderOrder = 1;
    this.group.add(this.disc);
    this.stars = makeStars(simple, pal);
    this.group.add(this.stars);
  }
}

function spiralTexture(color: Color, ok: boolean): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = c.height = 256;
  const g = c.getContext('2d');
  if (g) {
    g.translate(128, 128);
    const css = `${Math.round(color.r * 255)},${Math.round(color.g * 255)},${Math.round(color.b * 255)}`;
    const core = g.createRadialGradient(0, 0, 0, 0, 0, 70);
    core.addColorStop(0, `rgba(255,255,255,${ok ? 0.9 : 0.35})`);
    core.addColorStop(0.3, `rgba(${css},${ok ? 0.5 : 0.2})`);
    core.addColorStop(1, `rgba(${css},0)`);
    g.fillStyle = core;
    g.fillRect(-128, -128, 256, 256);
    for (let arm = 0; arm < 2; arm++) {
      g.beginPath();
      for (let r = 6; r <= 118; r += 3) {
        const th = arm * Math.PI + r * 0.055;
        const x = Math.cos(th) * r * 1.0, y = Math.sin(th) * r * 0.55;
        if (r === 6) g.moveTo(x, y); else g.lineTo(x, y);
      }
      g.strokeStyle = `rgba(${css},${ok ? 0.55 : 0.2})`;
      g.lineWidth = 9;
      g.lineCap = 'round';
      g.stroke();
    }
  }
  return new CanvasTexture(c);
}

export interface FarSprite {
  label: string;
  pos: Vector3;
  ok: boolean;
  sprite: Sprite;
}

export class FarGalaxies {
  readonly group = new Group();
  readonly sprites = new Map<string, FarSprite>();
  private bridges: { line: Line; a: string; b: string; label: string; mid: Vector3 }[] = [];

  /** `labels` are the other galaxies (not the one the scene shows). */
  build(entries: { label: string; ok: boolean }[], pal: Palette, radius = R_FAR): void {
    for (const s of this.sprites.values()) { s.sprite.material.map?.dispose(); s.sprite.material.dispose(); this.group.remove(s.sprite); }
    this.sprites.clear();
    entries.forEach((e, i) => {
      const a = (hash(e.label) % 360) / 360 * Math.PI * 2 + i * 0.35;
      const pos = new Vector3(Math.cos(a) * radius, ((hash(e.label + 'y') % 40) - 20) * 3, Math.sin(a) * radius);
      const col = e.ok ? pal.accent.clone().lerp(pal.status.working, 0.5) : pal.muted.clone();
      const sprite = new Sprite(new SpriteMaterial({
        map: spiralTexture(col, e.ok), transparent: true, depthWrite: false,
        blending: pal.mode === 'light' ? NormalBlending : AdditiveBlending, opacity: e.ok ? 1 : 0.6,
      }));
      sprite.scale.setScalar(260);
      sprite.position.copy(pos);
      sprite.renderOrder = 1;
      this.group.add(sprite);
      this.sprites.set(e.label, { label: e.label, pos, ok: e.ok, sprite });
    });
  }

  /** Arcs from a nebula to the far galaxy that has the same project. */
  buildBridges(items: { a: Vector3; galaxy: string; label: string }[], pal: Palette): void {
    for (const b of this.bridges) { this.group.remove(b.line); b.line.geometry.dispose(); (b.line.material as LineDashedMaterial).dispose(); }
    this.bridges = [];
    for (const it of items) {
      const far = this.sprites.get(it.galaxy);
      if (!far) continue;
      const mid = it.a.clone().add(far.pos).multiplyScalar(0.5);
      mid.y += 120;
      const pts: Vector3[] = [];
      for (let i = 0; i <= 40; i++) {
        const t = i / 40;
        pts.push(new Vector3(
          (1 - t) * (1 - t) * it.a.x + 2 * (1 - t) * t * mid.x + t * t * far.pos.x,
          (1 - t) * (1 - t) * it.a.y + 2 * (1 - t) * t * mid.y + t * t * far.pos.y,
          (1 - t) * (1 - t) * it.a.z + 2 * (1 - t) * t * mid.z + t * t * far.pos.z,
        ));
      }
      const geo = new BufferGeometry().setFromPoints(pts);
      const line = new Line(geo, new LineDashedMaterial({ color: pal.accent, transparent: true, opacity: 0.45, dashSize: 6, gapSize: 6 }));
      line.computeLineDistances();
      line.frustumCulled = false;
      this.group.add(line);
      this.bridges.push({ line, a: it.label, b: it.galaxy, label: it.label, mid: pts[20].clone() });
    }
  }

  get bridgeLabels(): { label: string; pos: Vector3 }[] {
    return this.bridges.map((b) => ({ label: b.label, pos: b.mid }));
  }
}
