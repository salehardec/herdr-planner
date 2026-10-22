/* Modal-ish dialogs the Esc key must be able to close: add a galaxy, add a universe, the todo pop-overs, the Station,
   the ship's computer. The arrival notice is not modal: it closes with its own buttons, not with Esc. */
import { signal } from '@preact/signals';
import { closeStation, shipDlg, stationDlg } from './ui/station-api';
import { rememberDlg } from './ui/memory-api';
import { snap } from './store';
import type { SavedFile } from './attach';

export const addDialogs = {
  galaxy: signal(false),
  universe: signal<{ galaxy: string } | null>(null),
  firstRun: signal(false),
  popover: signal<{ id: string; repo: string; galaxy?: string } | null>(null),
  /** A todo item dropped on a planet: the window to correct it before it goes to the task's agent. */
  tell: signal<{ id: string; task: string; galaxy: string } | null>(null),
};

/** The todo item being corrected in a window before it leaves: its words and its files. */
export const todoDraft = { text: signal(''), files: signal<SavedFile[]>([]) };

function loadDraft(id: string): void {
  const it = snap.value?.todo.find((x) => x.id === id);
  todoDraft.text.value = it?.text ?? '';
  todoDraft.files.value = it?.attachments ?? [];
}

/** A todo item dropped on a planet: the window to correct it, then tell the task's agent. */
export function openTell(id: string, task: string, galaxy: string): void {
  loadDraft(id);
  addDialogs.tell.value = { id, task, galaxy };
}

/** A todo item dropped on a universe: the window for a new task (words, files, program, model). */
export function openNewTask(id: string, repo: string, galaxy: string): void {
  loadDraft(id);
  addDialogs.popover.value = { id, repo, galaxy };
}

export const dialogOpen = (): boolean =>
  addDialogs.galaxy.value || !!addDialogs.universe.value || addDialogs.firstRun.value || !!addDialogs.popover.value || !!addDialogs.tell.value
  || !!stationDlg.value || shipDlg.value || !!rememberDlg.value;

export function closeDialogs(): void {
  if (rememberDlg.value) { rememberDlg.value = null; return; }
  if (addDialogs.popover.value) { addDialogs.popover.value = null; return; }
  if (addDialogs.tell.value) { addDialogs.tell.value = null; return; }
  if (stationDlg.value) { closeStation(); return; }
  if (shipDlg.value) { shipDlg.value = false; return; }
  if (addDialogs.universe.value) { addDialogs.universe.value = null; return; }
  if (addDialogs.galaxy.value) { addDialogs.galaxy.value = false; return; }
  if (addDialogs.firstRun.value) { addDialogs.firstRun.value = false; }
}
