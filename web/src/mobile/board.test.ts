import { describe, expect, it } from 'vitest';
import { DONE_SHOWN, heaps, shareOf } from './board';
import type { Task } from '../types';

const task = (n: number, status: Task['status'], over: Partial<Task> = {}): Task => ({
  id: `a/${n}`, repo: 'a', n, tag: `A-${n}`, title: '', state: '', kind: '', stage: '', msg: '', app: '', progress: null, eta: null, pr: '',
  review: null, round: 0, queue: null, status, label: '', board_label: '', review_failed: false, merge_blocked: false, archived: false,
  actions: [], created: n, description: '', worker_agent: '', model: '', session: '', worktree: '', exited_at: 0, key: '', subagents: [],
  subagents_note: '', seat: null, ...over,
});

describe('the phone\'s board', () => {
  it('puts the tasks that need the human first, then the work, the queue and the last done', () => {
    const list = heaps([task(1, 'done'), task(2, 'queued'), task(3, 'working'), task(4, 'review'), task(5, 'waiting'), task(6, 'plan')]);
    expect(list.map((h) => h.key)).toEqual(['attention', 'working', 'queued', 'done']);
    expect(list[0].tasks.map((x) => x.n)).toEqual([5, 6]);
    expect(list[1].tasks.map((x) => x.n)).toEqual([3, 4]);
  });

  it('leaves out an empty heap and the archived', () => {
    expect(heaps([task(1, 'working'), task(2, 'done', { archived: true })]).map((h) => h.key)).toEqual(['working']);
    expect(heaps([])).toEqual([]);
  });

  it('keeps the queue in the board\'s order and shows only the latest finished', () => {
    const q = heaps([task(1, 'queued', { queue: 2 }), task(2, 'queued', { queue: 1 })])[0];
    expect(q.tasks.map((x) => x.n)).toEqual([2, 1]);
    const many = Array.from({ length: DONE_SHOWN + 5 }, (_, i) => task(i + 1, 'done', { exited_at: 1000 + i }));
    const done = heaps(many)[0].tasks;
    expect(done).toHaveLength(DONE_SHOWN);
    expect(done[0].n).toBe(DONE_SHOWN + 5);
  });

  it('a share is clamped to a bar', () => {
    expect(shareOf(task(1, 'working', { progress: 64 }))).toBe(64);
    expect(shareOf(task(1, 'working', { progress: 250 }))).toBe(100);
    expect(shareOf(task(1, 'working'))).toBeNull();
  });
});
