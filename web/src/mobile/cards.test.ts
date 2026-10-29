import { describe, expect, it } from 'vitest';
import { blockOf, can, listed, planOf, safeUrl } from './cards';
import { collectRequests } from '../requests';
import type { Request, Task } from '../types';

const task = (over: Partial<Task>): Task => ({
  id: 'alpha/1', repo: 'alpha', n: 1, tag: 'ALP-1', title: 'T', state: 'blocked', kind: '', stage: '', msg: '', app: '', progress: null,
  eta: null, pr: '', review: null, round: 0, queue: null, status: 'waiting', label: '', board_label: '', review_failed: false,
  merge_blocked: false, archived: false, actions: ['tell'], created: 0, description: '', worker_agent: '', model: '', session: '',
  worktree: '', exited_at: 0, key: '', subagents: [], subagents_note: '', seat: null, ...over,
});

const reqOf = (t: Task): Request => collectRequests({ tasks: [t], coordinator: null })[0];

describe('the phone\'s cards', () => {
  it('a plan can be approved from the phone only when the board lists the action', () => {
    const ok = planOf(reqOf(task({ status: 'plan', stage: 'plan', actions: ['approve', 'tell'] })));
    expect(ok).toMatchObject({ block: 'plan', approve: true, tell: true, plan: true, note: '' });
    const no = planOf(reqOf(task({ status: 'plan', stage: 'plan', actions: ['tell'] })));
    expect(no.approve).toBe(false);
    expect(no.note).toBe('m.note.plan');
  });

  it('a merge past a failed review is the computer\'s: no approve, no «anyway», a note', () => {
    const bad = planOf(reqOf(task({ status: 'ready', stage: 'approval', review_failed: true, actions: ['review', 'anyway', 'tell'], pr: 'https://github.com/a/b/pull/1' })));
    expect(bad).toMatchObject({ block: 'approval', approve: false, note: 'm.note.approval', pr: 'https://github.com/a/b/pull/1' });
    const ok = planOf(reqOf(task({ status: 'ready', stage: 'approval', actions: ['approve', 'anyway', 'review', 'tell'] })));
    expect(ok.approve).toBe(true);
  });

  it('a GitLab merge that needs a clean checkout says why and what to do instead of offering a dead approve', () => {
    const blocked = task({
      id: 'all-gis-services-next/3', repo: 'all-gis-services-next', n: 3, tag: 'AGSVN-3',
      status: 'waiting', kind: 'permission', stage: 'approval', msg: 'Review: Pass · 1 minor · Merge needs a clean checkout',
      merge_blocked: true, actions: ['tell'], pr: '',
    });
    const p = planOf(reqOf(blocked));
    expect(p).toMatchObject({ block: 'approval', approve: false, note: 'm.note.mergeBlockedCheckout', pr: '' });
    expect(can(blocked, 'approve')).toBe(false);
  });

  it('other merge blocks keep the board reason and offer the computer or the Coordinator', () => {
    const p = planOf(reqOf(task({
      status: 'waiting', kind: 'permission', stage: 'approval', msg: 'The pull request checks fail',
      merge_blocked: true, actions: ['tell'],
    })));
    expect(p).toMatchObject({ block: 'approval', approve: false, note: 'm.note.mergeBlocked' });
  });

  it('a merge the board asks as a «permission» (the review failed) is still the merge', () => {
    const p = planOf(reqOf(task({ status: 'waiting', kind: 'permission', stage: 'approval', review_failed: true, actions: ['review', 'anyway', 'tell'] })));
    expect(p).toMatchObject({ block: 'approval', approve: false, screen: false, note: 'm.note.approval' });
  });

  it('what the phone has never reaches a card: anyway, stop, finish, fix are not actions of a phone', () => {
    const t = task({ status: 'failed', actions: ['retry', 'tell', 'stop', 'anyway', 'finish', 'fix'] });
    for (const a of ['stop', 'anyway', 'finish', 'fix', 'keys', 'yazi']) expect(can(t, a), a).toBe(false);
    for (const a of ['retry', 'tell']) expect(can(t, a), a).toBe(true);
    expect(planOf(reqOf(t))).toMatchObject({ block: 'failed', retry: true, tell: true, approve: false });
  });

  it('a task that stopped at a limit is continued, not talked to', () => {
    const p = planOf(reqOf(task({ status: 'waiting', stage: 'stopped', state: 'blocked', actions: ['retry', 'tell'] })));
    expect(p).toMatchObject({ block: 'limit', retry: true, tell: false });
  });

  it('attempts are picked from the phone, a question and a permission read the agent\'s screen', () => {
    expect(planOf(reqOf(task({ status: 'waiting', stage: 'pick', actions: [] })))).toMatchObject({ block: 'pick', pick: true, tell: false });
    expect(planOf(reqOf(task({ status: 'waiting', kind: 'question', actions: ['answer', 'tell', 'stop'] })))).toMatchObject({ block: 'question', screen: true, tell: true });
    expect(planOf(reqOf(task({ status: 'waiting', kind: 'permission', actions: ['tell'] })))).toMatchObject({ block: 'permission', screen: true });
  });

  it('a shipyard\'s alarm is read-only', () => {
    const r = collectRequests({
      tasks: [], coordinator: null,
      runners: [{ id: 'yard', ok: true, alerts: [{ key: 'k', kind: 'disk', free_pct: 8, free_bytes: 1, total_bytes: 2 }] } as never],
    })[0];
    expect(blockOf(r)).toBe('alarm');
    expect(planOf(r)).toMatchObject({ approve: false, retry: false, tell: false, note: 'm.note.alarm' });
  });

  it('the Coordinator\'s choices and dialog have their own blocks', () => {
    const list = collectRequests({
      tasks: [],
      coordinator: { reply: 'Что делаем?\n1) один\n2) другой', dialog: false } as never,
    });
    expect(list.map(blockOf)).toEqual(['options']);
    const dlg = collectRequests({ tasks: [], coordinator: { reply: 'x', dialog: true } as never });
    expect(dlg.map(blockOf)).toEqual(['dialog']);
  });

  it('only http(s) addresses are followed', () => {
    expect(safeUrl('https://github.com/a/b/pull/1')).toBe('https://github.com/a/b/pull/1');
    for (const bad of ['javascript:alert(1)', 'file:///etc/passwd', '//evil.example/x', 'data:text/html,x', '']) expect(safeUrl(bad), bad).toBe('');
  });

  it('a memory proposal is not listed', () => {
    const mem = { key: 'memory:x', type: 'memory' } as Request;
    expect(listed([mem])).toEqual([]);
  });
});
