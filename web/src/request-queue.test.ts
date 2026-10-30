import { beforeEach, expect, it } from 'vitest';
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
