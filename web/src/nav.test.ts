import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Signal } from '@preact/signals';
import type { Request, Task } from './types';
import { pendingRequests, resolvedRequests } from './request-queue';

vi.mock('./store', async () => {
  const { signal, computed } = await import('@preact/signals');
  const { pendingRequests } = await import('./request-queue');
  const rows = signal<Request[]>([]);
  return {
    rows, requests: computed(() => pendingRequests(rows.value)), details: signal({}), mode: signal('auto'),
    panels: Object.fromEntries(['wait', 'todo', 'log', 'archive', 'focus', 'chat'].map((k) => [k, signal(false)])),
    selected: signal(null), universe: signal(null), snap: signal({ board: { ok: true } }),
    armed: signal(null), screens: signal({}), tellDraft: signal({}), currentView: signal({ tasks: [], links: [] }), taskOf: vi.fn(),
  };
});
vi.mock('./api', () => ({ api: vi.fn(), post: vi.fn(), refreshNow: vi.fn() }));
vi.mock('./toast', () => ({ toast: vi.fn() }));
vi.mock('./i18n', () => ({ t: (key: string) => key }));
vi.mock('./chat', () => ({ draftMessage: vi.fn(), draftSent: {} }));
vi.mock('./todo-drag', () => ({ endDrag: vi.fn() }));

import * as store from './store';
import { post } from './api';
import { tellTask, answerMenu } from './actions';
import { clearStack, depth, fromQueue, navigateTo, onQueueAnswer, openRequest, prepareAnswer, queueRequest, setWorld, stepRequest } from './nav';

const rows = (store as unknown as { rows: Signal<Request[]> }).rows;
const row = (id: string, type = 'question'): Request => ({
  id, key: `${type}:${id}`, type, msg: 'Which one?', title: id, text: '', tag: id, repo: 'a', urgency: 0,
  task: type === 'memory' ? undefined : { id, tag: id, msg: 'Which one?', stage: '', status: 'waiting' } as Task,
});

beforeEach(() => {
  vi.clearAllMocks();
  setWorld(null as never);
  clearStack();
  resolvedRequests.value = new Map();
  rows.value = [row('a/1'), row('a/2'), row('a/3')];
  fromQueue.value = false;
  queueRequest.value = null;
  store.selected.value = null;
  vi.mocked(post).mockResolvedValue({ ok: true });
});

describe('working through the waiting queue', () => {
  it('opens the next request on success, without adding Back frames', async () => {
    await openRequest(0);
    const before = depth.value;
    expect(await tellTask(rows.value[0].task!, 'Keep it')).toBe(true);
    expect(store.selected.value?.id).toBe('a/2');
    expect(fromQueue.value).toBe(true);
    expect(depth.value).toBe(before);
    expect(pendingRequests(rows.value).map((r) => r.id)).toEqual(['a/2', 'a/3']);
  });

  it('keeps the request and queue intact when the board rejects the answer', async () => {
    await openRequest(0);
    vi.mocked(post).mockRejectedValue(new Error('Board unavailable'));
    expect(await tellTask(rows.value[0].task!, 'Keep it')).toBe(false);
    expect(store.selected.value?.id).toBe('a/1');
    expect(resolvedRequests.value.size).toBe(0);
  });

  it('shows an empty queue instead of the last task', async () => {
    rows.value = [row('a/1')];
    await openRequest(0);
    await prepareAnswer('a/1')();
    expect(store.selected.value).toBeNull();
    expect(queueRequest.value).toBeNull();
    expect(fromQueue.value).toBe(true);
    expect(store.panels.focus.value).toBe(true);
  });

  it('leaves a task opened manually selected', async () => {
    await navigateTo({ kind: 'planet', id: 'a/1' });
    await tellTask(rows.value[0].task!, 'Keep it');
    expect(store.selected.value?.id).toBe('a/1');
    expect(fromQueue.value).toBe(false);
  });

  it('uses the captured order when SSE removed the row before the response', async () => {
    await openRequest(1);
    const done = prepareAnswer('a/2');
    rows.value = [row('a/1'), row('a/3')];
    await done();
    expect(store.selected.value?.id).toBe('a/3');
  });

  it('does not consume a different question that arrived before the old response', async () => {
    await openRequest(0);
    const done = prepareAnswer('a/1');
    rows.value = [{ ...row('a/1'), msg: 'A new question' }, row('a/2')];
    await done();
    expect(store.requests.value.map((r) => r.id)).toEqual(['a/1', 'a/2']);
    expect(store.selected.value?.id).toBe('a/2');
  });

  it('does not move after manual navigation while the answer was in flight', async () => {
    await openRequest(0);
    const done = prepareAnswer('a/1');
    await navigateTo({ kind: 'planet', id: 'a/3' });
    await done();
    expect(store.selected.value?.id).toBe('a/3');
    expect(fromQueue.value).toBe(false);
  });

  it('resolves only the answered part of a task', async () => {
    rows.value = [row('a/1'), row('a/1', 'memory'), row('a/2')];
    await openRequest(0);
    await prepareAnswer('a/1')();
    expect(queueRequest.value?.key).toBe('memory:a/1');
    await prepareAnswer('a/1', 'memory')();
    expect(queueRequest.value?.key).toBe('question:a/2');
  });

  it('wraps to earlier remaining rows and handles the next/previous shortcuts', async () => {
    await openRequest(2);
    await prepareAnswer('a/3')();
    expect(store.selected.value?.id).toBe('a/1');
    await stepRequest(1);
    expect(store.selected.value?.id).toBe('a/2');
    await stepRequest(-1);
    expect(store.selected.value?.id).toBe('a/1');
  });

  it('advances after a successful agent menu answer but stays after rejection', async () => {
    await openRequest(0);
    vi.mocked(post).mockRejectedValueOnce(new Error('Old menu'));
    expect((await answerMenu('a/1', '', 'sig', { kind: 'pick', at: 0 })).ok).toBe(false);
    expect(store.selected.value?.id).toBe('a/1');
    expect((await answerMenu('a/1', '', 'sig', { kind: 'pick', at: 0 })).ok).toBe(true);
    expect(store.selected.value?.id).toBe('a/2');
  });

  it('captures a mobile surface before the request and calls it only on success', async () => {
    const moved = vi.fn();
    const capture = vi.fn(() => moved);
    const stop = onQueueAnswer(capture);
    vi.mocked(post).mockRejectedValueOnce(new Error('offline'));
    await tellTask(rows.value[0].task!, 'Keep it');
    expect(moved).not.toHaveBeenCalled();
    await tellTask(rows.value[0].task!, 'Keep it');
    expect(capture).toHaveBeenCalledTimes(2);
    expect(moved).toHaveBeenCalledTimes(1);
    stop();
  });
});
