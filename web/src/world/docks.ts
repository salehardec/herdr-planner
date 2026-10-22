/* Shipyards («верфи») just outside the Galaxy's boundary: a ring with four spokes, a gauge arc for the CPU load,
   and a small light per container of a task. */
import {
  AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture, Color, Group, NormalBlending, Points,
  PointsMaterial, Sprite, SpriteMaterial, Vector3,
} from 'three';
import { hash } from '../util/format';
import type { Palette } from './palette';

export interface DockEntry {
  id: string;
  ok: boolean;
  /** 0…1 of the CPUs busy. */
  load: number;
  containers: number;
  forgotten: number;
}

interface SpriteRec {
  pos: Vector3;
  ok: boolean;
  sprite: Sprite;
}

const SIZE = 256;
const RING = 92;

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

function dockTexture(color: Color, ok: boolean, load: number, containers: number, forgotten: number): CanvasTexture {
  const c = document.createElement('canvas');
  c.width = c.height = SIZE;
  const g = c.getContext('2d');
  if (g) {
    const css = `${Math.round(color.r * 255)},${Math.round(color.g * 255)},${Math.round(color.b * 255)}`;
    const alpha = ok ? 1 : 0.35;
    g.translate(SIZE / 2, SIZE / 2);
    g.lineCap = 'round';
    // the dock ring and four crossbeams
    g.strokeStyle = `rgba(${css},${alpha})`;
    g.lineWidth = 9;
    g.beginPath();
    g.arc(0, 0, RING, 0, Math.PI * 2);
    g.stroke();
    g.lineWidth = 5;
    for (let i = 0; i < 4; i++) {
      const a = i * Math.PI / 2 + Math.PI / 4;
      g.beginPath();
      g.moveTo(Math.cos(a) * 26, Math.sin(a) * 26);
      g.lineTo(Math.cos(a) * RING, Math.sin(a) * RING);
      g.stroke();
    }
    g.strokeStyle = `rgba(${css},${alpha * 0.65})`;
    g.lineWidth = 2;
    g.beginPath();
    g.arc(0, 0, 26, 0, Math.PI * 2);
    g.stroke();
    // the CPU load arc, warm over 80 %
    if (ok) {
      const hot = load > 0.8;
      g.strokeStyle = hot ? 'rgba(224,112,112,.95)' : `rgba(${css},.95)`;
      g.lineWidth = 11;
      g.beginPath();
      g.arc(0, 0, RING + 10, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * Math.max(0.02, Math.min(1, load)));
      g.stroke();
    }
    // one light per container; the forgotten ones warm
    const n = Math.min(8, containers);
    for (let i = 0; i < n; i++) {
      const a = -Math.PI / 2 + (i / Math.max(1, n)) * Math.PI * 2;
      g.fillStyle = i < forgotten ? 'rgba(240,170,90,.95)' : `rgba(${css},.95)`;
      g.beginPath();
      g.arc(Math.cos(a) * RING, Math.sin(a) * RING, 4.5, 0, Math.PI * 2);
      g.fill();
    }
  }
  const t = new CanvasTexture(c);
  t.needsUpdate = true;
  return t;
}

export class Docks {
  readonly group = new Group();
  readonly sprites = new Map<string, SpriteRec>();
  private points: Points | null = null;
  private pointsMat: PointsMaterial | null = null;
  private forgotten = 0;

  build(entries: DockEntry[], pal: Palette, radius = 800): void {
    for (const s of this.sprites.values()) {
      s.sprite.material.map?.dispose();
      s.sprite.material.dispose();
      this.group.remove(s.sprite);
    }
    this.sprites.clear();
    if (this.points) {
      this.group.remove(this.points);
      this.points.geometry.dispose();
      this.pointsMat?.dispose();
      this.points = null;
      this.pointsMat = null;
    }
    const pos = dockPositions(entries.map((e) => e.id), radius);
    const dotPos: number[] = [];
    const dotCol: number[] = [];
    this.forgotten = 0;
    for (const e of entries) {
      const at = pos.get(e.id) ?? new Vector3(radius, 0, 0);
      const color = e.ok ? pal.accent.clone().lerp(pal.status.working, 0.45) : pal.muted.clone();
      const tex = dockTexture(color, e.ok, e.load, e.containers, e.forgotten);
      const sprite = new Sprite(new SpriteMaterial({
        map: tex, transparent: true, depthWrite: false,
        blending: pal.mode === 'light' ? NormalBlending : AdditiveBlending,
        opacity: e.ok ? 1 : 0.55,
      }));
      sprite.scale.setScalar(220);
      sprite.position.copy(at);
      sprite.renderOrder = 1;
      this.group.add(sprite);
      this.sprites.set(e.id, { pos: at, ok: e.ok, sprite });
      this.forgotten += e.forgotten;
      const n = Math.min(8, e.containers);
      for (let i = 0; i < n; i++) {
        const a = -Math.PI / 2 + (i / Math.max(1, n)) * Math.PI * 2;
        const rr = (RING / SIZE) * 220;
        dotPos.push(at.x + Math.cos(a) * rr, at.y + Math.sin(a) * rr, at.z);
        const col = i < e.forgotten ? new Color(0.95, 0.66, 0.35) : color;
        dotCol.push(col.r, col.g, col.b);
      }
    }
    if (dotPos.length > 0) {
      const geo = new BufferGeometry();
      geo.setAttribute('position', new BufferAttribute(new Float32Array(dotPos), 3));
      geo.setAttribute('color', new BufferAttribute(new Float32Array(dotCol), 3));
      this.pointsMat = new PointsMaterial({
        size: 6, sizeAttenuation: true, transparent: true, depthWrite: false, vertexColors: true,
        blending: pal.mode === 'light' ? NormalBlending : AdditiveBlending,
      });
      this.points = new Points(geo, this.pointsMat);
      this.points.renderOrder = 2;
      this.group.add(this.points);
    }
  }

  /** The forgotten containers blink slowly, as «ждёт тебя» does. */
  update(time: number): void {
    if (!this.pointsMat) return;
    this.pointsMat.opacity = this.forgotten > 0 ? 0.55 + 0.35 * (0.5 + 0.5 * Math.sin(time * 2.2)) : 0.9;
  }
}
