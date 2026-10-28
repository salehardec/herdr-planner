/* The phone's board: the tasks in three heaps, in the order a person who is away wants them. */
import type { Task } from '../types';

export interface Heap {
  key: 'attention' | 'working' | 'queued' | 'done';
  tasks: Task[];
}

/** How many finished tasks the board shows: the rest are on the computer. */
export const DONE_SHOWN = 12;

const ATTENTION = new Set(['waiting', 'failed', 'ready', 'plan']);

/** Tasks that need the human first, then those at work (and in review), then the queue, then what was done lately. */
export function heaps(tasks: Task[]): Heap[] {
  const live = tasks.filter((x) => !x.archived);
  const byTime = (a: Task, b: Task): number => (b.exited_at || b.created) - (a.exited_at || a.created) || b.n - a.n;
  const out: Heap[] = [
    { key: 'attention', tasks: live.filter((x) => ATTENTION.has(x.status)).sort((a, b) => a.n - b.n) },
    { key: 'working', tasks: live.filter((x) => x.status === 'working' || x.status === 'review').sort((a, b) => a.n - b.n) },
    { key: 'queued', tasks: live.filter((x) => x.status === 'queued').sort((a, b) => (a.queue ?? 1e9) - (b.queue ?? 1e9) || a.n - b.n) },
    { key: 'done', tasks: live.filter((x) => x.status === 'done').sort(byTime).slice(0, DONE_SHOWN) },
  ];
  return out.filter((h) => h.tasks.length > 0);
}

/** A task's progress as a share 0..100 for a bar; null when the board has none. */
export const shareOf = (task: Task): number | null => (task.progress === null || task.progress === undefined ? null : Math.max(0, Math.min(100, task.progress)));
