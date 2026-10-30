/* Keep an answered row out of the queue while the board catches up. A new question at the same task is a new row. */
import { signal } from '@preact/signals';
import type { Request } from './types';

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
