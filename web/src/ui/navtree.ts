/* The navigator's tree: Galaxy → Universe → planets grouped by the day they were made → moons.
   Pure: the component only draws and handles keys. */
import type { Entity, GalaxyView, RoleMark, Runner, Seat, Status, Task } from '../types';
import { ATTENTION, STATUS_ORDER } from '../types';
import { groupByDate } from '../util/dates';
import { t } from '../i18n';

export interface NavNode {
  key: string;
  depth: number;
  kind: 'galaxy' | 'quasar' | 'universe' | 'quiet' | 'group' | 'planet' | 'moon' | 'runner' | 'runners' | 'addUniverse' | 'addRunner';
  label: string;
  sub?: string;
  badge?: string;
  status?: Status | 'running' | 'quiet' | 'done';
  entity?: Entity;
  expandable: boolean;
  open: boolean;
  dim?: boolean;
  count?: number;
  attention?: boolean;
  /** A planet: who is on the task now. */
  seat?: Seat;
  /** A planet: the crew's own roles put on it. */
  roles?: RoleMark[];
}

export interface NavInput {
  view: GalaxyView;
  selected: Entity | null;
  folds: Record<string, boolean>;
  hostLabel: string;
  coordinator: { status: string; subagents: { id: string; role: string; state: string; doing: string }[] } | null;
  attentionWaits: number;
  runners: Runner[];
  now?: number;
}

const order = (s: Status): number => STATUS_ORDER.indexOf(s);

/** Whether a node is open: the saved choice, else the default for its kind. */
function isOpen(folds: Record<string, boolean>, key: string, dflt: boolean): boolean {
  return key in folds ? folds[key] : dflt;
}

function planetNodes(
  tasks: Task[], u: string, depth: number, i: NavInput, out: NavNode[],
): void {
  const groups = groupByDate(tasks, (x) => x.created, i.now);
  for (const g of groups) {
    const gkey = `grp|${u}|${g.key}`;
    /* A date is open by default when it is today or holds a live task; only dates of finished tasks start folded. */
    const open = isOpen(i.folds, gkey, g.today || g.items.some((x) => x.status !== 'done' && !x.archived));
    out.push({
      key: gkey, depth, kind: 'group', label: g.today ? t('nav.today') : g.key, count: g.items.length, expandable: true, open,
      attention: g.items.some((x) => ATTENTION.includes(x.status)),
    });
    if (!open) continue;
    const sorted = g.items.slice().sort((a, b) => order(a.status) - order(b.status) || b.n - a.n);
    for (const task of sorted) {
      const moons = task.subagents.filter((s) => s.state !== 'done');
      const pkey = `planet|${task.id}`;
      const mopen = isOpen(i.folds, pkey, moons.some((m) => m.state === 'running'));
      out.push({
        key: pkey, depth: depth + 1, kind: 'planet', label: `${task.tag} ${task.title}`, sub: task.board_label || task.label,
        status: task.status, entity: { kind: 'planet', id: task.id }, expandable: moons.length > 0, open: mopen,
        dim: task.status === 'done', attention: ATTENTION.includes(task.status), seat: task.seat ?? undefined,
        roles: task.crew_roles?.length ? task.crew_roles : undefined,
      });
      if (mopen) {
        for (const m of moons) {
          out.push({
            key: `moon|${task.id}#${m.id}`, depth: depth + 2, kind: 'moon', label: m.role || m.title,
            sub: m.doing || m.title, status: m.state, entity: { kind: 'moon', id: `${task.id}#${m.id}` },
            expandable: false, open: false,
          });
        }
      }
    }
  }
}

function universeNodes(view: GalaxyView, i: NavInput, depth: number, out: NavNode[]): void {
  const live = (name: string): number => view.tasks.filter((x) => x.repo === name && x.status !== 'done').length;
  const projects = view.projects.slice().sort((a, b) => live(b.name) - live(a.name) || a.name.localeCompare(b.name));
  const quiet = projects.filter((p) => live(p.name) === 0);
  const busy = projects.filter((p) => live(p.name) > 0);
  const emit = (p: (typeof projects)[number], d: number): void => {
    const tasks = view.tasks.filter((x) => x.repo === p.name);
    const ukey = `uni|${p.name}`;
    const sel = i.selected && (
      (i.selected.kind === 'universe' && i.selected.id === p.name) ||
      (i.selected.kind === 'planet' && tasks.some((x) => x.id === i.selected?.id))
    );
    const open = isOpen(i.folds, ukey, !!sel || tasks.some((x) => x.status !== 'done'));
    const lag = p.lag && p.lag.behind > 0 ? `main −${p.lag.behind}` : '';
    out.push({
      key: ukey, depth: d, kind: 'universe', label: p.name, sub: p.key, badge: lag, entity: { kind: 'universe', id: p.name },
      expandable: tasks.length > 0, open, count: tasks.filter((x) => x.status !== 'done').length,
      attention: tasks.some((x) => ATTENTION.includes(x.status)), dim: p.empty,
    });
    if (open) planetNodes(tasks, p.name, d + 1, i, out);
  };
  for (const p of busy) emit(p, depth);
  if (quiet.length) {
    const qkey = 'quiet';
    const open = isOpen(i.folds, qkey, false) || (!!i.selected && quiet.some((p) => i.selected?.id === p.name || view.tasks.some((x) => x.repo === p.name && x.id === i.selected?.id)));
    out.push({ key: qkey, depth, kind: 'quiet', label: t('nav.quiet'), count: quiet.length, expandable: true, open, dim: true });
    if (open) for (const p of quiet) emit(p, depth + 1);
  }
}

export function buildTree(i: NavInput): NavNode[] {
  const out: NavNode[] = [];
  const v = i.view;
  const open = isOpen(i.folds, 'gal', true);
  out.push({
    key: 'gal', depth: 0, kind: 'galaxy', label: i.hostLabel || v.label, expandable: true, open, attention: false,
  });
  if (!open) return out;
  const subs = (i.coordinator?.subagents ?? []).filter((s) => s.state !== 'done');
  const qopen = isOpen(i.folds, 'quasar', subs.some((s) => s.state === 'running'));
  out.push({
    key: 'quasar', depth: 1, kind: 'quasar', label: t('quasar.label'), sub: i.coordinator?.status ?? '',
    entity: { kind: 'quasar', id: 'quasar' }, expandable: subs.length > 0, open: qopen,
    badge: i.attentionWaits ? String(i.attentionWaits) : '', attention: i.attentionWaits > 0,
  });
  if (qopen) {
    for (const m of subs) {
      out.push({
        key: `moon|quasar#${m.id}`, depth: 2, kind: 'moon', label: m.role, sub: m.doing, status: m.state as 'running' | 'quiet',
        entity: { kind: 'moon', id: `quasar#${m.id}` }, expandable: false, open: false,
      });
    }
  }
  universeNodes(v, i, 1, out);
  out.push({ key: 'addu', depth: 1, kind: 'addUniverse', label: t('nav.addUniverse', { galaxy: i.hostLabel || v.label }), expandable: false, open: false });
  const ykey = 'runners';
  const yopen = isOpen(i.folds, ykey, true);
  out.push({ key: ykey, depth: 1, kind: 'runners', label: t('nav.runners'), count: i.runners.length, expandable: true, open: yopen });
  if (yopen) {
    for (const r of i.runners) {
      const active = r.containers.filter((c) => c.class === 'active').length;
      const forgotten = r.containers.filter((c) => c.class === 'forgotten' || c.class === 'unsure').length;
      out.push({
        key: `run|${r.id}`, depth: 2, kind: 'runner', label: r.id,
        sub: r.ok ? t('runner.navLine', { n: active }) : t('runner.offline'),
        entity: { kind: 'runner', id: r.id }, expandable: false, open: false,
        dim: !r.ok, badge: forgotten > 0 ? String(forgotten) : '', attention: forgotten > 0,
      });
    }
    out.push({ key: 'addr', depth: 2, kind: 'addRunner', label: t('nav.addRunner'), expandable: false, open: false });
  }
  return out;
}
