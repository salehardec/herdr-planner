/* Requests to the human: one list from the board's tasks and the Coordinator. */
import type { Request, Task, Status, CoordinatorState } from './types';
import { firstSentence } from './util/format';
import { t } from './i18n';

const ORDER: Record<Status, number> = { waiting: 0, failed: 1, ready: 2, plan: 3, review: 4, working: 5, queued: 6, done: 7 };
const ATTN = new Set<Status>(['waiting', 'failed', 'ready', 'plan']);

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

const TITLE_KEY: Record<string, string> = {
  pick: 'req.pick', limit: 'req.limit', reviewneeds: 'req.reviewneeds', plan: 'req.plan', approval: 'req.approval',
  failed: 'req.failed', permission: 'req.permission', auth: 'req.auth', question: 'req.question',
  'coordinator-dialog': 'req.coordinator.dialog', 'coordinator-options': 'req.coordinator.options',
};

export const requestTitle = (type: string): string => t(TITLE_KEY[type] ?? 'req.other');

export function collectRequests(input: { tasks: Task[]; coordinator: CoordinatorState | null; galaxy: string }): Request[] {
  const out: Request[] = [];
  for (const task of input.tasks) {
    if (!ATTN.has(task.status)) continue;
    const type = requestType(task);
    out.push({
      key: `${type}:${task.id}`, type, title: requestTitle(type), id: task.id, tag: task.tag, repo: task.repo,
      msg: task.msg || task.board_label || '', text: firstSentence(task.msg || task.board_label || ''), task,
      urgency: ORDER[task.status] ?? 9, galaxy: input.galaxy,
    });
  }
  const c = input.coordinator;
  if (c?.dialog) {
    out.push({
      key: 'coordinator-dialog', type: 'coordinator-dialog', title: requestTitle('coordinator-dialog'), id: 'coordinator',
      tag: t('quasar.short'), repo: 'coordinator', msg: c.reply || '', text: firstSentence(c.reply || ''), urgency: -1, galaxy: input.galaxy,
    });
  } else if (c) {
    const options = parseOptions(c.reply);
    if (options.length) {
      out.push({
        key: 'coordinator-options', type: 'coordinator-options', title: requestTitle('coordinator-options'), id: 'coordinator',
        tag: t('quasar.short'), repo: 'coordinator', msg: c.reply || '', text: firstSentence(c.reply || ''), options, urgency: 0.5, galaxy: input.galaxy,
      });
    }
  }
  return out.sort((a, b) => a.urgency - b.urgency || (a.task?.n || 0) - (b.task?.n || 0));
}
