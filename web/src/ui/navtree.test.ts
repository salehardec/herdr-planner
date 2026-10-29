import { describe, expect, it } from 'vitest';
import { buildTree } from './navtree';
import type { GalaxyView, Project, Task } from '../types';

const day = (d: number, h = 12): number => new Date(2026, 9, d, h).getTime();
const task = (n: number, created: number, status: Task['status'] = 'working', extra: Partial<Task> = {}): Task => ({
  id: `alpha/${n}`, repo: 'alpha', n, tag: `ALP-${n}`, title: `Задача ${n}`, state: '', kind: '', stage: '', msg: '', app: 'claude', progress: null, eta: null, pr: '', review: null, round: 0,
  queue: null, status, label: '', board_label: '', review_failed: false, merge_blocked: false, archived: false, actions: [], created, description: '', worker_agent: '', model: '',
  session: '', worktree: '', exited_at: 0, key: 'ALP', subagents: [], subagents_note: '', ...extra,
});
const project = (name: string, quiet = false): Project => ({ name, key: name.slice(0, 3).toUpperCase(), active: 1, done: 0, total: 1, statuses: {}, quiet, root: '', folder: '', origin: '', lag: null, empty: false });

const view = (tasks: Task[], projects: Project[] = [project('alpha')]): GalaxyView => ({ label: 'host', ok: true, tasks, projects, links: [] });
const now = day(20, 15);

const build = (v: GalaxyView, folds: Record<string, boolean> = {}, extra: Partial<Parameters<typeof buildTree>[0]> = {}) =>
  buildTree({ view: v, selected: null, folds, hostLabel: 'host', coordinator: null, attentionWaits: 0, runners: [], now, ...extra });

describe('navigator tree', () => {
  const tasks = [task(1, day(20, 9)), task(2, day(20, 10), 'waiting'), task(3, day(19, 9)), task(4, day(14, 9), 'done'), task(5, day(19, 20))];

  it('starts with the galaxy, the quasar, then universes', () => {
    const rows = build(view(tasks));
    expect(rows[0]).toMatchObject({ kind: 'galaxy', label: 'host' });
    expect(rows[1]).toMatchObject({ kind: 'quasar' });
    expect(rows[2]).toMatchObject({ kind: 'universe', label: 'alpha' });
  });

  it('opens «Сегодня» and the dates with live tasks, newest first, and folds the finished ones', () => {
    const rows = build(view(tasks));
    const groups = rows.filter((r) => r.kind === 'group');
    expect(groups.map((g) => g.label)).toEqual(['Сегодня', '19.10.2026', '14.10.2026']);
    expect(groups.map((g) => g.open)).toEqual([true, true, false]);
    const planets = rows.filter((r) => r.kind === 'planet');
    expect(planets.map((p) => p.entity?.id)).toEqual(['alpha/2', 'alpha/1', 'alpha/5', 'alpha/3']);
  });

  it('opens a past date that holds a live task, and keeps the dates of finished tasks folded', () => {
    const rows = build(view([task(1, day(20, 9)), task(2, day(19, 9), 'review'), task(3, day(14, 9), 'done'), task(4, day(14, 10), 'done', { archived: true })]));
    const groups = rows.filter((r) => r.kind === 'group');
    expect(groups.map((g) => [g.label, g.open])).toEqual([['Сегодня', true], ['19.10.2026', true], ['14.10.2026', false]]);
    expect(rows.filter((r) => r.kind === 'planet').map((p) => p.entity?.id)).toEqual(['alpha/1', 'alpha/2']);
  });

  it('respects a date the operator folded by hand, even with a live task in it', () => {
    const rows = build(view([task(1, day(19, 9), 'working')]), { 'grp|alpha|19.10.2026': false });
    expect(rows.filter((r) => r.kind === 'group').map((g) => g.open)).toEqual([false]);
    expect(rows.filter((r) => r.kind === 'planet')).toEqual([]);
  });

  it('shows no «this machine» mark on the galaxy', () => {
    expect(build(view(tasks))[0].sub).toBeUndefined();
  });

  it('opens a date when the operator folded it open, and the choice wins', () => {
    const rows = build(view(tasks), { 'grp|alpha|14.10.2026': true, 'grp|alpha|today': false });
    const groups = rows.filter((r) => r.kind === 'group');
    expect(groups.map((g) => g.open)).toEqual([false, true, true]);
    expect(rows.filter((r) => r.kind === 'planet').map((p) => p.entity?.id)).toEqual(['alpha/5', 'alpha/3', 'alpha/4']);
  });

  it('puts the quiet universes in one folded group', () => {
    const v = view([task(1, day(20, 9))], [project('alpha'), project('docs', true), project('old', true)]);
    const rows = build(v);
    const quiet = rows.find((r) => r.kind === 'quiet');
    expect(quiet?.count).toBe(2);
    expect(quiet?.open).toBe(false);
    expect(rows.some((r) => r.kind === 'universe' && r.label === 'docs')).toBe(false);
  });

  it('shows sub-agents under their planet, alive ones only', () => {
    const sub = (id: string, state: 'running' | 'quiet' | 'done') => ({ id, kind: 'claude', role: `R-${id}`, title: '', started_at_ms: 1, last_at_ms: 2, state, doing: 'работает', result: '', note: '' });
    const t = task(1, day(20, 9), 'working', { subagents: [sub('a', 'running'), sub('b', 'done'), sub('c', 'quiet')] });
    const rows = build(view([t]));
    expect(rows.filter((r) => r.kind === 'moon').map((r) => r.label)).toEqual(['R-a', 'R-c']);
  });

  it('has no row for other machines: the galaxy, the quasar, the universes and «add a universe» only', () => {
    const rows = build(view(tasks));
    expect(rows.filter((r) => r.kind === 'galaxy').map((g) => g.label)).toEqual(['host']);
    expect(rows[rows.length - 1].kind).toBe('addRunner');
  });

  it('lists the shipyards after the universes, with their containers and forgotten count', () => {
    const r = {
      id: 'vm-r', target: 'me@192.0.2.29', ports_host: '192.0.2.29', tags: [], protected: [], note: '', ok: true,
      why: null, fetched_at_ms: 1, host: {} as any, networks: [], volumes: [],
      containers: [{ class: 'active' }, { class: 'forgotten' }, { class: 'unsure' }, { class: 'foreign' }] as any,
    } as any;
    const rows = buildTree({ view: view(tasks), selected: null, folds: {}, hostLabel: 'host', coordinator: null, attentionWaits: 0, runners: [r], now });
    const group = rows.find((x) => x.kind === 'runners' && x.label === 'Верфи');
    expect(group?.count).toBe(1);
    const yard = rows.find((x) => x.kind === 'runner');
    expect(yard?.entity).toEqual({ kind: 'runner', id: 'vm-r' });
    expect(yard?.badge).toBe('2');
    expect(yard?.sub).toContain('на связи');
    expect(rows[rows.length - 2].kind).toBe('runner');
    expect(rows[rows.length - 1].kind).toBe('addRunner');
  });

  it('carries who is on a task: the reviewer under review, none once the review is over', () => {
    const reviewer = { role: 'reviewer' as const, program: 'pi', model: 'kimi-coding/k3.1-pro', took_over: false };
    const t = [task(1, day(20, 9), 'review', { seat: reviewer }), task(2, day(20, 10), 'ready', { seat: null }), task(3, day(20, 11), 'working')];
    const planets = build(view(t)).filter((r) => r.kind === 'planet');
    expect(planets.find((p) => p.entity?.id === 'alpha/1')?.seat).toEqual(reviewer);
    expect(planets.find((p) => p.entity?.id === 'alpha/2')?.seat).toBeUndefined();
    expect(planets.find((p) => p.entity?.id === 'alpha/3')?.seat).toBeUndefined();
  });
});
