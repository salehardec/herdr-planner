/* The agent's closing note (`closing.md`) of a task: loaded once per state of the task and kept for the
   Focus and for the window that shows it on the whole page. */
import { signal } from '@preact/signals';
import { api } from './api';
import type { Task } from './types';

export interface Closing {
  text: string;
  truncated: boolean;
  key: string;
}

export const closing = signal<Record<string, Closing>>({});

export async function loadClosing(task: Task): Promise<void> {
  const key = `${task.status}|${task.msg}|${task.exited_at}`;
  if (closing.value[task.id]?.key === key) return;
  try {
    const r = await api<{ text: string; truncated: boolean }>('/api/task/closing?id=' + encodeURIComponent(task.id));
    closing.value = { ...closing.value, [task.id]: { text: r.text, truncated: r.truncated, key } };
  } catch { /* none */ }
}
