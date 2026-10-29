/* Requests to the human: one list from the board's tasks, the Coordinator and the shipyards' alarms. */
import type { Entity, Request, Runner, RunnerAlert, Task, Status, CoordinatorState, MemorySuggestion } from './types';
import { firstSentence } from './util/format';
import { t } from './i18n';

const ORDER: Record<Status, number> = { waiting: 0, failed: 1, ready: 2, plan: 3, review: 4, working: 5, queued: 6, done: 7 };
const ATTN = new Set<Status>(['waiting', 'failed', 'ready', 'plan']);
/** Between a ready plan (3) and a task in review (4): an alarm needs the human, but not before a task's own request. */
const ALARM_URGENCY = 3.5;
/** A «В память» proposal waits for the human like the rest, but never before a task's own request or an alarm. */
const MEMORY_URGENCY = 3.8;
const GB = 1024 * 1024 * 1024;
const gb = (b?: number): string => String(((b ?? 0) / GB).toFixed(1));

export const PERMISSION_ALLOW = (): string => t('perm.allow.text');
export const PERMISSION_DENY = (): string => t('perm.deny.text');

/** Answer options in the Coordinator's words: a numbered/lettered list AFTER a question.
 *  Plain bullets and explanations are not options. */
export function parseOptions(reply: string): { label: string; value: string }[] {
  const lines = String(reply ?? '').split('\n');
  let at = -1;
  for (let i = 0; i < lines.length; i++) if (lines[i].includes('?')) at = i;
  if (at < 0) return [];
  const out: { label: string; value: string }[] = [];
  const re = /^\s*(?:(\d+)[).:]|([a-zA-Z])[).:])\s+(.+?)\s*$/;
  for (let i = at + 1; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim()) {
      if (out.length) break;
      continue;
    }
    const m = line.match(re);
    if (!m) {
      if (out.length) break;
      continue;
    }
    const label = m[1] ? `${m[1]})` : `${m[2].toUpperCase()})`;
    const value = m[3].replace(/\*\*/g, '').replace(/`/g, '').trim();
    if (value.length > 1 && value.length <= 300 && !out.some((o) => o.value === value)) out.push({ label, value });
  }
  return out.length >= 2 ? out.slice(0, 9) : [];
}

/** What a waiting task asks, in the board's own vocabulary. */
export function requestType(task: Task): string {
  if (task.stage === 'pick') return 'pick';
  if (task.stage === 'stopped' && (task.state === 'blocked' || task.status === 'waiting')) return 'limit';
  if (task.stage === 'reviewneeds') return 'reviewneeds';
  if (task.status === 'plan') return 'plan';
  if (task.status === 'ready') return 'approval';
  if (task.status === 'failed') return 'failed';
  if (task.kind === 'permission') return 'permission';
  if (task.kind === 'auth') return 'auth';
  return 'question';
}

/** Why a task waits for the human, in one word: the reason the badge and the queue name (PLN-61).
 *  A task the board knows as a question (or as nothing at all) may be an agent on a dialog: the caller that has read
 *  its screen passes `permission` and the reason is «разрешение». */
export function waitReason(task: Task, permission = false): string {
  if (task.status === 'plan') return t('reason.plan');
  if (task.status === 'ready') return t('reason.ready');
  if (task.status === 'failed') return t('reason.failed');
  if (task.stage === 'pick') return t('reason.pick');
  if (task.stage === 'stopped') return t('reason.limit');
  if (task.stage === 'reviewneeds') return t('reason.review');
  if (task.stage === 'approval') return task.review_failed || task.merge_blocked ? t('reason.review') : t('reason.ready');
  if (permission || task.kind === 'permission') return t('reason.permission');
  if (task.kind === 'auth') return t('reason.auth');
  // `question`, no kind at all («Needs you»): the human is asked something, even when the board has no words for it.
  return t('reason.question');
}

/** The status badge's words; a task that waits for the human says why (`Ждёт тебя · разрешение`). */
export function attentionLabel(task: Task, permission = false): string {
  const base = t(`status.${task.status}`);
  return task.status === 'waiting' ? `${base} · ${waitReason(task, permission)}` : base;
}

const TITLE_KEY: Record<string, string> = {
  pick: 'req.pick', limit: 'req.limit', reviewneeds: 'req.reviewneeds', plan: 'req.planReady', approval: 'req.approval',
  failed: 'req.failed', memory: 'req.memory', permission: 'req.permission', auth: 'req.auth', question: 'req.question',
  'coordinator-dialog': 'req.coordinator.dialog', 'coordinator-options': 'req.coordinator.options',
};

const ALARM_TITLE_KEY: Record<string, string> = {
  offline: 'runner.alertKind.offline', disk: 'runner.alertKind.disk',
  crash: 'runner.alertKind.crash', forgotten: 'runner.alertKind.forgotten',
};

/** What an alarm says, in the words of the current language. */
export function alertText(a: RunnerAlert): string {
  if (a.kind === 'offline') return t('runner.alert.offline', { n: a.minutes ?? 0 });
  if (a.kind === 'disk') {
    return t('runner.alert.disk', { pct: a.free_pct ?? 0, free: gb(a.free_bytes), total: gb(a.total_bytes) });
  }
  if (a.kind === 'crash') {
    const status = a.status || a.state || '';
    return a.tag
      ? t('runner.alert.crash', { name: a.name ?? '', tag: a.tag, status })
      : t('runner.alert.crashNoTag', { name: a.name ?? '', status });
  }
  return a.tag
    ? t('runner.alert.forgotten', { name: a.name ?? '', tag: a.tag })
    : t('runner.alert.forgottenNoTag', { name: a.name ?? '' });
}

/** What a request leads to: its task's planet, the Quasar for the Coordinator, the yard for an alarm. */
export function requestEntity(r: Pick<Request, 'type' | 'id'>): Entity {
  if (r.type === 'runner-alert') return { kind: 'runner', id: r.id };
  return r.id === 'coordinator' ? { kind: 'quasar', id: 'quasar' } : { kind: 'planet', id: r.id };
}

export const requestTitle = (type: string): string => t(TITLE_KEY[type] ?? 'req.other');

export function collectRequests(input: { tasks: Task[]; coordinator: CoordinatorState | null; runners?: Runner[]; suggestions?: MemorySuggestion[] }): Request[] {
  const out: Request[] = [];
  for (const task of input.tasks) {
    if (!ATTN.has(task.status)) continue;
    const type = requestType(task);
    // A ready plan says exactly what it waits for; the agent's own words go on the Focus card.
    const text = type === 'plan'
      ? requestTitle('plan')
      : firstSentence(task.msg || task.board_label || '') || requestTitle(type);
    out.push({
      key: `${type}:${task.id}`, type, title: requestTitle(type), id: task.id, tag: task.tag, repo: task.repo,
      msg: task.msg || task.board_label || '', text, why: waitReason(task), task,
      urgency: ORDER[task.status] ?? 9,
    });
  }
  // The shipyards' alarms: a row of their own, leading to that yard's focus (PLN-42).
  for (const r of input.runners ?? []) {
    for (const a of r.alerts ?? []) {
      out.push({
        key: `runner-alert:${a.key}`, type: 'runner-alert', title: t(ALARM_TITLE_KEY[a.kind] ?? 'req.other'),
        id: r.id, tag: r.id, repo: t('nav.runners'), msg: alertText(a), text: alertText(a), alert: a,
        why: t('reason.yard'), urgency: ALARM_URGENCY,
      });
    }
  }
  // Lines a model proposed for a universe's memory after a task was done: each one waits until it is kept or dropped.
  for (const m of input.suggestions ?? []) {
    const first = m.lines.find((l) => l.text.trim())?.text ?? '';
    const text = m.title || firstSentence(first);
    out.push({
      key: `memory:${m.task}`, type: 'memory', title: requestTitle('memory'), id: m.task, tag: m.tag, repo: m.repo,
      msg: text, text, why: t('reason.memory'), urgency: MEMORY_URGENCY,
    });
  }
  const c = input.coordinator;
  if (c?.dialog) {
    out.push({
      key: 'coordinator-dialog', type: 'coordinator-dialog', title: requestTitle('coordinator-dialog'), id: 'coordinator',
      tag: t('quasar.short'), repo: 'coordinator', msg: c.reply || '', text: firstSentence(c.reply || ''),
      why: t('reason.coordinator'), urgency: -1,
    });
  } else if (c) {
    const options = parseOptions(c.reply);
    if (options.length) {
      out.push({
        key: 'coordinator-options', type: 'coordinator-options', title: requestTitle('coordinator-options'), id: 'coordinator',
        tag: t('quasar.short'), repo: 'coordinator', msg: c.reply || '', text: firstSentence(c.reply || ''), options,
        why: t('reason.coordinator'), urgency: 0.5,
      });
    }
  }
  return out.sort((a, b) => a.urgency - b.urgency || (a.task?.n || 0) - (b.task?.n || 0));
}
