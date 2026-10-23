/* Modal-ish dialogs the Esc key must be able to close: add a universe, the todo pop-overs, the Station,
   the ship's computer. The arrival notice is not modal: it closes with its own buttons, not with Esc. */
import { signal } from '@preact/signals';
import { closeStation, shipDlg, stationDlg } from './ui/station-api';
import { memoryWin, rememberDlg } from './ui/memory-api';
import { snap } from './store';
import type { SavedFile } from './attach';

export const addDialogs = {
  universe: signal(false),
  runner: signal(false),
  firstRun: signal(false),
  popover: signal<{ id: string; repo: string } | null>(null),
  /** A todo item dropped on a planet: the window to correct it before it goes to the task's agent. */
  tell: signal<{ id: string; task: string } | null>(null),
  /** The agent's result, the description or the plan in a window over the page, with the task's actions. */
  summary: signal<{ task: string; part: 'closing' | 'description' | 'plan' } | null>(null),
  /** An agent's terminal screen in a window over the page; `reviewer` reads the `<agent>-r` pane. */
  screen: signal<{ task: string; role?: 'reviewer' } | null>(null),
  /** The log tail of one container of a shipyard. */
  ctlLogs: signal<{ runner: string; id: string } | null>(null),
  /** «Было → стало» of stop / restart / delete of one container; `scope` is the task whose Focus it was asked from. */
  ctlAct: signal<{ runner: string; id: string; act: 'stop' | 'restart' | 'delete'; scope: string } | null>(null),
};

/** The todo item being corrected in a window before it leaves: its words and its files. */
export const todoDraft = { text: signal(''), files: signal<SavedFile[]>([]) };

function loadDraft(id: string): void {
  const it = snap.value?.todo.find((x) => x.id === id);
  todoDraft.text.value = it?.text ?? '';
  todoDraft.files.value = it?.attachments ?? [];
}

/** A todo item dropped on a planet: the window to correct it, then tell the task's agent. */
export function openTell(id: string, task: string): void {
  loadDraft(id);
  addDialogs.tell.value = { id, task };
}

/** A todo item dropped on a universe: the window for a new task (words, files, program, model). */
export function openNewTask(id: string, repo: string): void {
  loadDraft(id);
  addDialogs.popover.value = { id, repo };
}

export const dialogOpen = (): boolean =>
  addDialogs.universe.value || addDialogs.runner.value || addDialogs.firstRun.value || !!addDialogs.popover.value || !!addDialogs.tell.value
  || !!addDialogs.summary.value || !!addDialogs.screen.value || !!addDialogs.ctlLogs.value || !!addDialogs.ctlAct.value || !!stationDlg.value || shipDlg.value || !!rememberDlg.value || !!memoryWin.value;

export function closeDialogs(): void {
  if (addDialogs.ctlAct.value) { addDialogs.ctlAct.value = null; return; }
  if (addDialogs.ctlLogs.value) { addDialogs.ctlLogs.value = null; return; }
  if (addDialogs.screen.value) { addDialogs.screen.value = null; return; }
  if (memoryWin.value) { memoryWin.value = null; return; }
  if (addDialogs.summary.value) { addDialogs.summary.value = null; return; }
  if (rememberDlg.value) { rememberDlg.value = null; return; }
  if (addDialogs.popover.value) { addDialogs.popover.value = null; return; }
  if (addDialogs.tell.value) { addDialogs.tell.value = null; return; }
  if (stationDlg.value) { closeStation(); return; }
  if (shipDlg.value) { shipDlg.value = false; return; }
  if (addDialogs.runner.value) { addDialogs.runner.value = false; return; }
  if (addDialogs.universe.value) { addDialogs.universe.value = false; return; }
  if (addDialogs.firstRun.value) { addDialogs.firstRun.value = false; }
}
