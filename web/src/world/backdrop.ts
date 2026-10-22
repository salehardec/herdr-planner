/* Stars and the faint disc of the Galaxy with its spiral arms. */
import {
  AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture, DoubleSide, Group,
  Mesh, MeshBasicMaterial, NormalBlending, PlaneGeometry, Points, PointsMaterial,
} from 'three';
import { R_MIN } from './layout';
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
