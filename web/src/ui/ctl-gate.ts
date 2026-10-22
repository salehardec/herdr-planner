/* Who may be stopped, restarted or removed from a screen: the same rule the server checks again (src/runners_ctl.rs).
   `scope` is the task whose Focus the buttons are in; the shipyard's own screen has none. */
import type { RunnerContainer } from '../types';

export type CtlAct = 'stop' | 'restart' | 'delete';

const UP = new Set(['running', 'restarting', 'paused']);

export const isUp = (c: RunnerContainer): boolean => UP.has(c.state);

/** The dictionary key of the reason a button is locked, or '' when the action is allowed. */
export function ctlGate(c: RunnerContainer, act: CtlAct, scope: string): string {
  if (c.class === 'foreign') return 'ctl.noForeign';
  if (c.class === 'unsure') return 'ctl.noUnsure';
  if (c.class === 'active' && !(scope && c.task?.id === scope)) return 'ctl.noActive';
  if (act === 'stop' && !isUp(c)) return 'ctl.alreadyStopped';
  if (act === 'delete' && isUp(c)) return 'ctl.stopFirst';
  return '';
}

/** Logs: every container of a board task, never a foreign one. */
export function logsGate(c: RunnerContainer): string {
  return c.class === 'foreign' ? 'ctl.noForeign' : '';
}

/** A shell inside the container: the same rule as the server (src/runners_ctl.rs::shell_gate) — a task's container,
    never a foreign one, a shared cache or an unsure one; only a running container can be entered. */
export function shellGate(c: RunnerContainer): string {
  if (c.class === 'foreign') return 'ctl.shellForeign';
  if (c.class === 'shared') return 'ctl.shellShared';
  if (c.class === 'unsure') return 'ctl.shellUnsure';
  if (c.state !== 'running') return 'ctl.shellStopped';
  return '';
}

/** Escape sequences of a terminal (colours, cursor) are not shown in a log. */
export function stripAnsi(s: string): string {
  return s.replace(new RegExp(`${String.fromCharCode(27)}\\[[0-9;?]*[ -/]*[@-~]`, 'g'), '');
}

/** The lines that hold `q` (any case), with the total; all of them when `q` is empty. */
export function grep(text: string, q: string): { lines: string[]; total: number } {
  const all = text === '' ? [] : text.replace(/\n$/, '').split('\n');
  const needle = q.trim().toLowerCase();
  return { lines: needle ? all.filter((l) => l.toLowerCase().includes(needle)) : all, total: all.length };
}
