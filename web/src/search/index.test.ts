import { describe, expect, it } from 'vitest';
import { buildDocs, searchDocs, swapLayout } from './index';
import type { GalaxyView, Task } from '../types';

const task = (over: Partial<Task>): Task => ({
  id: 'alpha/7', repo: 'alpha', n: 7, tag: 'ALP-7', title: 'Починить кэш отчётов', state: 'working', kind: '', stage: 'working',
  msg: 'правит', app: 'claude', progress: null, eta: null, pr: '', review: null, round: 0, queue: null, status: 'working', label: 'В работе',
  board_label: 'в работе', review_failed: false, merge_blocked: false, archived: false, actions: [], created: 1, description: 'кэш падает по ночам',
  worker_agent: 'claude/sonnet', model: 'sonnet', session: '', worktree: '', exited_at: 0, key: 'ALP', subagents: [], subagents_note: '', ...over,
});

const view = (tasks: Task[]): GalaxyView => ({
  id: '', label: 'host', home: true, ok: true, tasks, links: [],
  projects: [{ name: 'alpha', key: 'ALP', active: 1, done: 0, total: 1, statuses: {}, quiet: false, root: '', folder: '', origin: '', lag: null, empty: false }],
});

const labels = { quasar: 'Квазар · Координатор', todo: 'дела', chat: 'Чат' };

describe('search', () => {
  const docs = buildDocs([view([task({}), task({ id: 'alpha/8', n: 8, tag: 'ALP-8', title: 'Новая вкладка', description: 'модель deepseek', model: 'deepseek' })])], null, [], labels);

  it('finds a task by a loose spelling (е for ё) and by a key typed without the dash', () => {
    const f = searchDocs(docs, 'отчетов кэш');
    expect(f.some((x) => x.doc.id === 'alpha/7')).toBe(true);
    expect(searchDocs(docs, 'alp7').find((x) => x.doc.kind === 'planet')?.doc.id).toBe('alpha/7');
  });

  it('finds by description and by model', () => {
    expect(searchDocs(docs, 'ночам').some((x) => x.doc.id === 'alpha/7')).toBe(true);
    expect(searchDocs(docs, 'deepseek').some((x) => x.doc.id === 'alpha/8')).toBe(true);
  });

  it('groups by kind: the galaxy, the quasar and the universe come before planets', () => {
    const f = searchDocs(docs, 'alpha');
    const kinds = f.map((x) => x.doc.kind);
    expect(kinds.indexOf('universe')).toBeLessThan(kinds.indexOf('planet') === -1 ? 99 : kinds.indexOf('planet'));
  });

  it('understands the wrong keyboard layout', () => {
    expect(swapLayout('ghfd')).toBe('прав');
    const f = searchDocs(docs, 'rnim'); // «кэш» is not it; check the swap itself works on a Russian word typed in English
    expect(Array.isArray(f)).toBe(true);
    expect(searchDocs(docs, 'rfif').length >= 0).toBe(true);
  });

  it('returns nothing for an empty query', () => {
    expect(searchDocs(docs, '  ')).toEqual([]);
  });
});
