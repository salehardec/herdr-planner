/* The galaxy's layout: deterministic, pure, no drawing.
   Projects (universes) sit on spiral arms, one arm per folder of the projects
   (the parent of the repository's folder); the busy ones close to the Quasar,
   quiet ones on the outer ring, smaller. Planets orbit inside a universe. */

import type { GalaxyView, Project, Status, Task } from '../types';
import { hash } from '../util/format';

export const QUASAR_R = 14;
export const R_MIN = 140;
export const QUIET_R0 = 520;
export const R_BOUND = 760;
export const R_FAR = 1150;
export const SLOTS = 3;

export interface PlanetLayout {
  id: string;
  tag: string;
  title: string;
  status: Status;
  app: string;
  repo: string;
  n: number;
  orbit: number;
  angle0: number;
  radius: number;
  size: number;
  belt: boolean;
  created: number;
}

export interface UniverseLayout {
  name: string;
  key: string;
  group: string;
  groupIndex: number;
  quiet: boolean;
  empty: boolean;
  active: number;
  total: number;
  center: { x: number; y: number; z: number };
  radius: number;
  planets: PlanetLayout[];
  /** Index in the layout (stable per snapshot): the nebula's slot in the shared buffers. */
  index: number;
}

export interface Layout {
  universes: UniverseLayout[];
  byName: Map<string, UniverseLayout>;
  /** Planet id → universe + planet layout. */
  positions: Map<string, { u: UniverseLayout; p: PlanetLayout }>;
  groups: string[];
  /** Radius of the Galaxy's boundary: the ship turns back there. Grows with the number of universes. */
  bound: number;
}

export function groupOf(p: Project): string {
  const folder = (p.folder || '').replace(/\/+$/, '');
  if (!folder) return '—';
  return folder.split('/').filter(Boolean).slice(-1)[0] || '—';
}

const sizeOf = (s: Status): number =>
  s === 'waiting' || s === 'failed' ? 2.3 : s === 'ready' || s === 'plan' ? 2.0 : 1.6;

/** Nebula radius from the number of live planets: grows slowly so that 50 of them still fit. */
export function universeRadius(live: number, quiet: boolean): number {
  const outer = live === 0 ? 0 : 9 + Math.floor((live - 1) / SLOTS) * 6.5;
  return Math.max(quiet ? 12 : 20, outer + 9);
}

export function computeLayout(view: GalaxyView, opts: { showQuiet?: boolean } = {}): Layout {
  const showQuiet = opts.showQuiet !== false;
  const byRepo = new Map<string, Task[]>();
  for (const t of view.tasks) {
    const list = byRepo.get(t.repo) ?? [];
    list.push(t);
    byRepo.set(t.repo, list);
  }
  const projects = new Map<string, Project>();
  for (const p of view.projects) projects.set(p.name, p);
  for (const name of byRepo.keys()) {
    if (!projects.has(name)) {
      projects.set(name, {
        name, key: name.slice(0, 3).toUpperCase(), active: 0, done: 0, total: 0, statuses: {}, quiet: true,
        root: '', folder: '', origin: '', lag: null, empty: false,
      });
    }
  }
  const all = [...projects.values()].sort((a, b) => a.name.localeCompare(b.name));
  const groups = [...new Set(all.map(groupOf))].sort();
  const universes: UniverseLayout[] = [];
  const positions = new Map<string, { u: UniverseLayout; p: PlanetLayout }>();

  // Busy universes first (closest to the Quasar), each along its group's arm; quiet ones on the outer ring.
  // Every nebula takes the first free place (no overlap), so 50 of them still make an orderly spiral.
  const live = (name: string): Task[] => (byRepo.get(name) ?? []).filter((t) => t.status !== 'done');
  const ordered = all.slice().sort((a, b) => live(b.name).length - live(a.name).length || a.name.localeCompare(b.name));
  const arms = Math.max(1, groups.length);
  const placed: { x: number; z: number; r: number }[] = [];
  const GAP = 16;
  const free = (x: number, z: number, r: number): boolean => placed.every((p) => Math.hypot(p.x - x, p.z - z) >= p.r + r + GAP);
  const armFirst = new Map<string, number>();
  let quietSeq = 0;

  for (const proj of ordered) {
    const list = (byRepo.get(proj.name) ?? []).slice().sort((x, y) => x.n - y.n);
    const liveList = list.filter((t) => t.status !== 'done');
    const doneList = list.filter((t) => t.status === 'done');
    const quiet = liveList.length === 0;
    if (quiet && !showQuiet) continue;
    const group = groupOf(proj);
    const gi = groups.indexOf(group);
    const planets: PlanetLayout[] = [];
    liveList.forEach((t, i) => {
      const orbit = Math.floor(i / SLOTS);
      const slot = i % SLOTS;
      const count = Math.min(SLOTS, liveList.length - orbit * SLOTS);
      const jitter = (((hash(t.id) % 40) - 20) / 360) * Math.PI * 2;
      planets.push({
        id: t.id, tag: t.tag, title: t.title, status: t.status, app: t.app || '', repo: proj.name, n: t.n,
        orbit, angle0: (slot / count) * Math.PI * 2 + jitter, radius: 9 + orbit * 6.5,
        size: sizeOf(t.status), belt: false, created: t.created,
      });
    });
    const outer = planets.reduce((m, p) => Math.max(m, p.radius), 0);
    const radius = Math.max(quiet ? 12 : 20, outer + 9);
    let center: { x: number; y: number; z: number } | null = null;
    const yOff = ((hash(proj.name + 'y') % 21) - 10) * (quiet ? 1.4 : 0.9);
    if (!quiet) {
      const base = (gi / arms) * Math.PI * 2;
      const lanes = [0, 1, -1, 2, -2];
      let r = armFirst.get(group) ?? R_MIN;
      search: for (let step = 0; step < 400; step++, r += 6) {
        for (const lane of lanes) {
          const a = base + (r - R_MIN) * 0.0105 + (lane * (radius + GAP + 22)) / Math.max(120, r);
          const x = Math.cos(a) * r, z = Math.sin(a) * r;
          if (free(x, z, radius)) { center = { x, y: yOff, z }; armFirst.set(group, Math.max(armFirst.get(group) ?? R_MIN, r - 40)); break search; }
        }
      }
    } else {
      const q = quietSeq++;
      let r = QUIET_R0;
      for (let k = 0; k < 800 && !center; k++) {
        const a = (q + k * 0.37) * 2.399963 + (gi / arms) * 0.7;
        const rr = r + Math.floor(k / 12) * 22;
        const x = Math.cos(a) * rr, z = Math.sin(a) * rr;
        if (free(x, z, radius)) center = { x, y: yOff, z };
      }
    }
    if (!center) center = { x: QUIET_R0 + placed.length * 9, y: yOff, z: 0 };
    placed.push({ x: center.x, z: center.z, r: radius });
    doneList.forEach((t, i) => {
      const jitter = (((hash(t.id) % 40) - 20) / 360) * Math.PI * 2;
      planets.push({
        id: t.id, tag: t.tag, title: t.title, status: 'done', app: t.app || '', repo: proj.name, n: t.n,
        orbit: -1, angle0: (i / Math.max(1, doneList.length)) * Math.PI * 2 + jitter, radius: radius + 6,
        size: 1.1, belt: true, created: t.created,
      });
    });
    const u: UniverseLayout = {
      name: proj.name,
      key: proj.key || proj.name.slice(0, 3).toUpperCase(),
      group, groupIndex: gi, quiet, empty: !!proj.empty && list.length === 0,
      active: liveList.length, total: list.length,
      center, radius, planets, index: universes.length,
    };
    universes.push(u);
    for (const p of planets) positions.set(p.id, { u, p });
  }
  const byName = new Map(universes.map((u) => [u.name, u]));
  const extent = universes.reduce((m, u) => Math.max(m, Math.hypot(u.center.x, u.center.z) + u.radius), 0);
  const bound = Math.max(R_BOUND, Math.ceil((extent + 120) / 20) * 20);
  return { universes, byName, positions, groups, bound };
}

/** Where a planet is at `time` seconds. Orbits turn slowly; the belt of done planets turns slower. */
export function planetPosition(
  u: UniverseLayout, p: PlanetLayout, time: number, moving: boolean,
  out: { x: number; y: number; z: number } = { x: 0, y: 0, z: 0 },
): { x: number; y: number; z: number } {
  const turn = moving ? time * (p.belt ? 0.02 : 0.07 - Math.min(0.05, p.orbit * 0.008)) : 0;
  const a = p.angle0 + turn;
  out.x = u.center.x + Math.cos(a) * p.radius;
  out.y = u.center.y;
  out.z = u.center.z + Math.sin(a) * p.radius;
  return out;
}

/** Normalised git origin: no scheme, user or `.git`; `host:path` → `host/path`; lower case. */
export function normalizeOrigin(url: string): string {
  let s = (url || '').trim();
  if (!s) return '';
  s = s.replace(/^[a-z+]+:\/\//i, '').replace(/^[^@/]+@/, '');
  s = s.replace(/^([^/:]+):(?!\/\/)/, '$1/').replace(/\.git$/i, '').replace(/\/+$/, '');
  return s.toLowerCase();
}

export function liveCount(tasks: Task[]): number {
  return tasks.filter((t) => t.status !== 'done').length;
}
