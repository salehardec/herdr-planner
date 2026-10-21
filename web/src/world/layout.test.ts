import { describe, expect, it } from 'vitest';
import { computeLayout, normalizeOrigin, planetPosition, R_BOUND, universeRadius } from './layout';
import type { GalaxyView, Project, Status, Task } from '../types';

const proj = (name: string, folder: string, quiet = false): Project => ({
  name, key: name.slice(0, 3).toUpperCase(), active: quiet ? 0 : 1, done: 0, total: 1, statuses: {}, quiet, root: `${folder}/${name}`, folder, origin: '', lag: null, empty: false,
});
const task = (repo: string, n: number, status: Status = 'working'): Task => ({
  id: `${repo}/${n}`, repo, n, tag: `${repo.slice(0, 3).toUpperCase()}-${n}`, title: 't', state: '', kind: '', stage: '', msg: '', app: 'claude', progress: null, eta: null, pr: '',
  review: null, round: 0, queue: null, status, label: '', board_label: '', review_failed: false, merge_blocked: false, archived: false, actions: [], created: n, description: '',
  worker_agent: '', model: '', session: '', worktree: '', exited_at: 0, key: '', subagents: [], subagents_note: '',
});

function big(universes: number, perUniverse: number): GalaxyView {
  const projects: Project[] = [];
  const tasks: Task[] = [];
  for (let i = 0; i < universes; i++) {
    const quiet = i % 3 === 2;
    const name = `proj-${i}`;
    projects.push(proj(name, `/home/u/Projects/${['AI', 'herdr', 'Tern', 'misc'][i % 4]}`, quiet));
    for (let n = 1; n <= perUniverse; n++) tasks.push(task(name, n, quiet ? 'done' : n % 4 === 0 ? 'waiting' : 'working'));
  }
  return { id: '', label: 'host', home: true, ok: true, tasks, projects, links: [] };
}

describe('galaxy layout', () => {
  it('is deterministic', () => {
    const a = computeLayout(big(12, 5));
    const b = computeLayout(big(12, 5));
    expect(a.universes.map((u) => [u.name, u.center])).toEqual(b.universes.map((u) => [u.name, u.center]));
  });

  it('does not turn into a mess at 50 universes and 500 planets: nebulae never overlap and all stay in the Galaxy', () => {
    const layout = computeLayout(big(50, 10));
    expect(layout.universes.length).toBe(50);
    expect(layout.positions.size).toBe(500);
    for (const u of layout.universes) {
      expect(Math.hypot(u.center.x, u.center.z)).toBeLessThan(R_BOUND);
    }
    let overlaps = 0;
    for (let i = 0; i < layout.universes.length; i++) {
      for (let j = i + 1; j < layout.universes.length; j++) {
        const a = layout.universes[i], b = layout.universes[j];
        const d = Math.hypot(a.center.x - b.center.x, a.center.z - b.center.z);
        if (d < a.radius + b.radius) overlaps++;
      }
    }
    expect(overlaps).toBe(0);
  });

  it('puts quiet universes on the outer ring and busy ones inside', () => {
    const layout = computeLayout(big(12, 3));
    const quiet = layout.universes.filter((u) => u.quiet);
    const busy = layout.universes.filter((u) => !u.quiet);
    expect(quiet.length).toBeGreaterThan(0);
    const minQuiet = Math.min(...quiet.map((u) => Math.hypot(u.center.x, u.center.z)));
    const maxBusy = Math.max(...busy.map((u) => Math.hypot(u.center.x, u.center.z)));
    expect(minQuiet).toBeGreaterThan(maxBusy);
    expect(quiet[0].radius).toBeLessThan(busy[0].radius + 1);
  });

  it('gives one arm per folder of the projects', () => {
    const layout = computeLayout(big(8, 2));
    expect(layout.groups).toEqual(['AI', 'Tern', 'herdr', 'misc']);
  });

  it('keeps planets on orbits around the universe', () => {
    const layout = computeLayout(big(2, 7));
    const u = layout.universes[0];
    const p = u.planets[0];
    const pos = planetPosition(u, p, 3, true);
    expect(Math.hypot(pos.x - u.center.x, pos.z - u.center.z)).toBeCloseTo(p.radius, 5);
    expect(universeRadius(7, false)).toBeGreaterThan(universeRadius(1, false));
  });

  it('can hide the quiet universes', () => {
    expect(computeLayout(big(9, 2), { showQuiet: false }).universes.every((u) => !u.quiet)).toBe(true);
  });
});

describe('origin', () => {
  it('compares the same repository in any form', () => {
    const forms = ['https://github.com/Owner/Repo.git', 'git@github.com:owner/repo.git', 'ssh://git@github.com/owner/repo', 'github.com/owner/repo/'];
    expect(new Set(forms.map(normalizeOrigin)).size).toBe(1);
    expect(normalizeOrigin('')).toBe('');
  });
});
