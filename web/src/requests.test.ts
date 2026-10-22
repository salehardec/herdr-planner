import { describe, expect, it } from 'vitest';
import { collectRequests, parseOptions, requestType } from './requests';
import type { Task } from './types';

const task = (over: Partial<Task>): Task => ({
  id: 'a/1', repo: 'a', n: 1, tag: 'A-1', title: 't', state: 'blocked', kind: '', stage: '', msg: 'm', app: 'claude', progress: null, eta: null, pr: '', review: null, round: 0, queue: null,
  status: 'waiting', label: '', board_label: '', review_failed: false, merge_blocked: false, archived: false, actions: [], created: 0, description: '', worker_agent: '', model: '', session: '',
  worktree: '', exited_at: 0, key: 'A', subagents: [], subagents_note: '', ...over,
});

describe('answer options', () => {
  it('plain bullets are not answer options', () => {
    expect(parseOptions('Сделал:\n- обновил README\n- поправил тесты\n\nВсё ок? Это всё.')).toEqual([]);
    expect(parseOptions('Что делаем?\n- один\n- два')).toEqual([]);
  });
  it('numbered options after the question are answers', () => {
    expect(parseOptions('Что делаем?\n1) **Да**\n2) `Нет`')).toEqual([{ label: '1)', value: 'Да' }, { label: '2)', value: 'Нет' }]);
  });
});

describe('requests to the human', () => {
  it('names what a waiting task asks', () => {
    expect(requestType(task({ stage: 'pick' }))).toBe('pick');
    expect(requestType(task({ status: 'plan' }))).toBe('plan');
    expect(requestType(task({ status: 'ready' }))).toBe('approval');
    expect(requestType(task({ stage: 'stopped', state: 'blocked' }))).toBe('limit');
    expect(requestType(task({ kind: 'auth' }))).toBe('auth');
    expect(requestType(task({}))).toBe('question');
  });
  it('lists the urgent first and puts the Coordinator\'s dialog before them', () => {
    const list = collectRequests({
      tasks: [task({ id: 'a/2', n: 2, status: 'ready' }), task({ id: 'a/1', n: 1, status: 'failed', state: 'error' }), task({ id: 'a/3', n: 3, status: 'working', state: 'working' })],
      coordinator: { dialog: true, reply: 'May I?' } as never,
    });
    expect(list.map((r) => r.id)).toEqual(['coordinator', 'a/1', 'a/2']);
  });
});
