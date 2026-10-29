import { beforeEach, expect, it, vi } from 'vitest';
import { signal } from '@preact/signals';
import type { TurnData } from './types';

const calls = vi.hoisted(() => ({ api: vi.fn() }));
vi.mock('./api', () => ({ api: calls.api, post: vi.fn(), refreshNow: vi.fn() }));
vi.mock('./store', async () => {
  const { signal } = await import('@preact/signals');
  return { chat: signal({ turns: [], total: 0, available: null, loading: false, loadingLatest: false }) };
});
vi.mock('./attach', () => ({ makeShelf: () => ({ files: signal([]) }) }));
vi.mock('./chat-unread', () => ({ chatRead: signal(null), settleRead: vi.fn() }));
const turn = (n: number, q = `question ${n}`): TurnData => ({ n, at: 1, from: 'human', q, a: '' });

let saved: Map<string, string>;
beforeEach(async () => {
  vi.resetModules();
  calls.api.mockReset();
  const { chat } = await import('./store');
  chat.value = { ...chat.value, turns: [], total: 0, available: null, loading: false, loadingLatest: false };
  saved = new Map();
  vi.stubGlobal('localStorage', { getItem: (k: string) => saved.get(k) ?? null, setItem: (k: string, v: string) => saved.set(k, v), removeItem: (k: string) => saved.delete(k) });
});

it('reload covers an echo older than the latest page before leaving it in the waiting queue', async () => {
  saved.set('planner.chat.echoes', JSON.stringify([{ id: 'saved', text: 'mid-turn', base: 0, at: 1, attachments: [] }]));
  calls.api.mockImplementation(async (path: string) => path.includes('&before=')
    ? { turns: [turn(0), turn(1, 'mid-turn'), turn(2), turn(3), turn(4)] }
    : { available: true, total: 8, turns: [turn(5), turn(6), turn(7)] });
  const { loadLatest, pendingMessages } = await import('./chat');
  await loadLatest();
  expect(calls.api.mock.calls.map(([p]) => p)).toEqual(['/api/chat/history?limit=3', '/api/chat/history?limit=100&before=5']);
  expect(pendingMessages()).toEqual([]);
  expect(saved.has('planner.chat.echoes')).toBe(false);
  const { chat } = await import('./store');
  expect(chat.value.turns.map((t) => t.n)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
});

it('does not lose a transcript event arriving while the preceding history request is in flight', async () => {
  let finish!: (page: unknown) => void;
  calls.api.mockImplementationOnce(() => new Promise((r) => { finish = r; }))
    .mockResolvedValue({ available: true, total: 2, turns: [turn(0), turn(1)] });
  const { loadLatest } = await import('./chat');
  const first = loadLatest();
  await loadLatest(); // a second event before the first request completes
  finish({ available: true, total: 1, turns: [turn(0)] });
  await first;
  await Promise.resolve();
  const { chat } = await import('./store');
  expect(calls.api).toHaveBeenCalledTimes(2);
  expect(chat.value.turns.map((t) => t.n)).toEqual([0, 1]);
});
