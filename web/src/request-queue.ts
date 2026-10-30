/* Keep an answered row out of the queue while the board catches up. A new question at the same task is a new row. */
import { signal } from '@preact/signals';
import type { Request, Task } from './types';

export const resolvedRequests = signal<ReadonlyMap<string, string>>(new Map());
const signature = (r: Request): string => JSON.stringify([r.key, r.msg, r.task?.stage, r.task?.state, r.task?.round, r.task?.session]);

export function resolveRequest(r: Request): void {
  resolvedRequests.value = new Map(resolvedRequests.value).set(r.key, signature(r));
}

export function restoreRequest(key: string): void {
  const next = new Map(resolvedRequests.value);
  next.delete(key);
  resolvedRequests.value = next;
}

export const pendingRequests = (all: Request[]): Request[] => all.filter((r) => resolvedRequests.value.get(r.key) !== signature(r));

/** Called on a fresh projection: after a row changes or disappears, it may be asked again. */
export function forgetChangedRequests(all: Request[]): void {
  const old = resolvedRequests.peek();
  const next = new Map([...old].filter(([key, sig]) => all.some((r) => r.key === key && signature(r) === sig)));
  if (next.size !== old.size) resolvedRequests.value = next;
}

/** Next in the order the person saw when they answered; wrap to earlier unhandled rows at the end. */
export function nextRequest(order: string[], key: string, pending: Request[]): Request | undefined {
  const at = order.indexOf(key);
  const keys = [...order.slice(at + 1), ...order.slice(0, Math.max(0, at))];
  return keys.map((k) => pending.find((r) => r.key === k)).find(Boolean) ?? pending[0];
}

/* A dialog answered through the planner: the keys went to the agent, but the board still says «waiting» until the agent's
   hook reports. Until then (or until the screen shows another menu) the task does not look as if it still waits for the human. */

/** How long an answer hides the dialog at most: the board has caught up by then, or the answer did not land and the screen says so. */
const ANSWERED_MS = 30000;
/** The same menu still on the screen this long after the answer means it did not land. */
export const RELOOK_MS = 3000;

interface Answered { sig: string; menu: string; at: number }
export const answeredDialogs = signal<ReadonlyMap<string, Answered>>(new Map());
const taskSignature = (t: Task): string => JSON.stringify([t.status, t.state, t.kind, t.stage, t.msg, t.round, t.session]);

export function markDialogAnswered(task: Task, menuSig: string): void {
  const at = Date.now();
  answeredDialogs.value = new Map(answeredDialogs.value).set(task.id, { sig: taskSignature(task), menu: menuSig, at });
  setTimeout(() => { if (answeredDialogs.peek().get(task.id)?.at === at) clearDialogAnswered(task.id); }, ANSWERED_MS + 50);
}

export function clearDialogAnswered(id: string): void {
  if (!answeredDialogs.peek().has(id)) return;
  const next = new Map(answeredDialogs.peek());
  next.delete(id);
  answeredDialogs.value = next;
}

/** The task's dialog was answered here and the board has not moved since. */
export function dialogAnswered(task: Task): boolean {
  const a = answeredDialogs.value.get(task.id);
  return !!a && a.sig === taskSignature(task);
}

/** The screen was read again: another menu, or the same one after the answer had time to land, means the dialog is back. */
export function screenReread(id: string, menuSig: string | undefined): void {
  const a = answeredDialogs.peek().get(id);
  if (!a || !menuSig) return;
  if (menuSig !== a.menu || Date.now() - a.at > RELOOK_MS) clearDialogAnswered(id);
}
