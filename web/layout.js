/* Детерминированная раскладка мира: угол вселенной запоминается, орбиты
   планет стабильны по номеру задачи. Ничего не рисует. */

const ANGLE_KEY = 'herdr-planner.world.angles';

export function hash(str) {
  let h = 2166136261 >>> 0;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h >>> 0;
}

export function loadAngles() {
  try {
    const raw = localStorage.getItem(ANGLE_KEY);
    const v = raw ? JSON.parse(raw) : {};
    return v && typeof v === 'object' ? v : {};
  } catch { return {}; }
}

export function saveAngles(angles) {
  try { localStorage.setItem(ANGLE_KEY, JSON.stringify(angles)); } catch { /* private mode */ }
}

/** The angle of `name`: kept from before, or the free slot farthest from the used ones. */
export function angleFor(name, used, angles) {
  const old = Number(angles[name]);
  if (Number.isFinite(old)) return old;
  const slots = 36;
  let best = 0;
  let bestGap = -1;
  for (let i = 0; i < slots; i++) {
    const a = (i / slots) * Math.PI * 2;
    let gap = Infinity;
    for (const u of used) {
      let d = Math.abs(a - u) % (Math.PI * 2);
      if (d > Math.PI) d = Math.PI * 2 - d;
      gap = Math.min(gap, d);
    }
    if (gap > bestGap) { bestGap = gap; best = a; }
  }
  return best;
}

const ACTIVE_R = 190;
const QUIET_R = 330;
const SLOTS = 3;

/**
 * @param {{tasks:Array, projects:Array, showQuiet?:boolean}} input
 * @returns {{universes:Array, positions:Map<string,object>, god:{x:number,y:number,z:number}}}
 */
export function computeLayout({ tasks, projects, showQuiet = true }) {
  const angles = loadAngles();
  const byRepo = new Map();
  for (const t of tasks) {
    if (!byRepo.has(t.repo)) byRepo.set(t.repo, []);
    byRepo.get(t.repo).push(t);
  }
  const names = [...new Set([
    ...projects.map((p) => p.name),
    ...byRepo.keys(),
  ])].sort();
  const quietNames = new Set(projects.filter((p) => p.quiet).map((p) => p.name));
  const shown = names.filter((n) => showQuiet || !quietNames.has(n));
  const used = [];
  const universes = [];
  let dirty = false;

  for (const name of shown) {
    const quiet = quietNames.has(name);
    const a = angleFor(name, used, angles);
    if (Number(angles[name]) !== a) { angles[name] = a; dirty = true; }
    used.push(a);
    const r = (quiet ? QUIET_R : ACTIVE_R) + ((hash(name) % 41) - 20);
    const center = { x: Math.cos(a) * r, y: 0, z: Math.sin(a) * r };
    const list = (byRepo.get(name) || []).slice().sort((x, y) => x.n - y.n);
    const live = list.filter((t) => t.s !== 'done');
    const done = list.filter((t) => t.s === 'done');
    const planets = [];
    live.forEach((t, i) => {
      const orbit = Math.floor(i / SLOTS);
      const slot = i % SLOTS;
      const count = Math.min(SLOTS, live.length - orbit * SLOTS);
      const radius = 9 + orbit * 6.5;
      // Even slots plus a small stable jitter: planets never bunch up on an orbit.
      const jitter = (((hash(t.id) % 40) - 20) / 360) * Math.PI * 2;
      const angle0 = (slot / count) * Math.PI * 2 + jitter;
      const size = t.s === 'waiting' || t.s === 'failed' ? 2.3 : t.s === 'ready' || t.s === 'plan' ? 2.0 : 1.6;
      planets.push({
        id: t.id, tag: t.tag, title: t.title, s: t.s, agent: t.agent || '', repo: name,
        line: t.line || '', num: t.n, orbit, angle0, radius, size,
      });
    });
    const outer = planets.reduce((m, p) => Math.max(m, p.radius), 0);
    const universeR = Math.max(quiet ? 14 : 22, outer + 9);
    done.forEach((t, i) => {
      const jitter = (((hash(t.id) % 40) - 20) / 360) * Math.PI * 2;
      planets.push({
        id: t.id, tag: t.tag, title: t.title, s: 'done', agent: t.agent || '', repo: name,
        line: t.line || '', num: t.n, orbit: -1, angle0: (i / Math.max(1, done.length)) * Math.PI * 2 + jitter,
        radius: universeR + 6, size: 1.1, belt: true,
      });
    });
    universes.push({
      name,
      key: projects.find((p) => p.name === name)?.key || name.slice(0, 3).toUpperCase(),
      quiet,
      center,
      radius: universeR,
      planets,
      done: done.length,
      active: live.length,
    });
  }
  if (dirty) saveAngles(angles);

  const positions = new Map();
  for (const u of universes) {
    for (const p of u.planets) positions.set(p.id, { u, p, x: u.center.x, y: 0, z: u.center.z });
  }
  return { universes, positions, god: { x: 0, y: 0, z: 0 } };
}

/** Where a planet is at `time` (seconds): orbits turn slowly; the belt stands. */
export function planetPosition(entry, time, moving = true) {
  if (!entry) return { x: 0, y: 0, z: 0 };
  const { u, p } = entry;
  const turn = moving && !p.belt ? time * (0.07 - Math.min(0.05, p.orbit * 0.008)) : 0;
  const a = p.angle0 + turn;
  return {
    x: u.center.x + Math.cos(a) * p.radius,
    y: u.center.y,
    z: u.center.z + Math.sin(a) * p.radius,
  };
}
