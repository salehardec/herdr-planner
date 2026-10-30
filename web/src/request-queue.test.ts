import { beforeEach, expect, it, vi } from 'vitest';
import type { Request } from './types';
import { forgetChangedRequests, pendingRequests, resolveRequest, resolvedRequests, restoreRequest } from './request-queue';
const row = (msg = 'First question') => ({ key: 'question:a/1', id: 'a/1', msg } as Request);
beforeEach(() => { resolvedRequests.value = new Map(); });
it('keeps a stale projection hidden until the board catches up', () => {
  resolveRequest(row());
  forgetChangedRequests([row()]);
  expect(pendingRequests([row()])).toEqual([]);
  forgetChangedRequests([]);
  expect(pendingRequests([row()])).toHaveLength(1);
});
it('shows a new question at the same task', () => {
  resolveRequest(row());
  forgetChangedRequests([row('Another question')]);
  expect(pendingRequests([row('Another question')])).toHaveLength(1);
});
it('restores a reversible answer without hiding another part of the task', () => {
  resolveRequest(row());
  const memory = { ...row(), key: 'memory:a/1' };
  expect(pendingRequests([row(), memory])).toEqual([memory]);
  restoreRequest(row().key);
  expect(pendingRequests([row(), memory])).toHaveLength(2);
});

it('hides an answered dialog until the board moves or the screen shows the dialog again (PLN-101)', async () => {
  const { answeredDialogs, dialogAnswered, markDialogAnswered, screenReread } = await import('./request-queue');
  answeredDialogs.value = new Map();
  const task = { id: 'a/1', status: 'waiting', state: 'blocked', kind: 'permission', stage: '', msg: 'Needs you', round: 0, session: 's' } as never;
  expect(dialogAnswered(task)).toBe(false);
  markDialogAnswered(task, 'sig1');
  expect(dialogAnswered(task)).toBe(true);
  // the screen no longer has the menu, or has the next one: still answered, or answered and a new question
  screenReread('a/1', undefined);
  expect(dialogAnswered(task)).toBe(true);
  screenReread('a/1', 'sig2');
  expect(dialogAnswered(task)).toBe(false);
  // the board says the agent went on: a new state is not the answered one
  markDialogAnswered(task, 'sig1');
  expect(dialogAnswered({ ...(task as object), status: 'working', state: 'working' } as never)).toBe(false);
  // the same menu long after the answer: it did not land
  markDialogAnswered(task, 'sig1');
  const now = Date.now();
  const spy = vi.spyOn(Date, 'now').mockReturnValue(now + 4000);
  screenReread('a/1', 'sig1');
  spy.mockRestore();
  expect(dialogAnswered(task)).toBe(false);
});
