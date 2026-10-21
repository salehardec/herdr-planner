/* What the operator can do to tasks, links and the Coordinator. Every action asks twice
   when it cannot be undone, goes to the machine the task lives on, and leaves the
   operator where they are. */
import { armed, details, galaxyId, panels, screens, snap, tellDraft, currentView, taskOf } from './store';
import { draftMessage, draftSent } from './chat';
import { endDrag } from './todo-drag';
import type { SavedFile } from './attach';
import { ApiError, api, post, refreshNow } from './api';
import { toast } from './toast';
import { t } from './i18n';
import { afterAnswered } from './nav';
import type { Entity, Task } from './types';

export const ACTION_LABEL: Record<string, string> = {
  approve: 'act.approve', anyway: 'act.anyway', retry: 'act.retry', review: 'act.review', stop: 'act.stop',
  delegate: 'act.delegate', archive: 'act.archive', restore: 'act.restore', answer: 'act.answer', fix: 'act.fix', finish: 'act.finish',
};

/** The machine an action goes to: the one the thing lives on. */
const machineOf = (galaxy?: string): string | undefined => {
  const g = galaxy ?? galaxyId.value;
  return g ? g : undefined;
};

export async function sendAction(action: string, extra: Record<string, unknown> = {}, galaxy?: string): Promise<any> {
  try {
    const r = await post('/api/action', { action, galaxy: machineOf(galaxy), ...extra });
    setTimeout(() => { void refreshNow(); }, 600);
    return r;
  } catch (e) {
    const err = e as ApiError;
    if (err.status === 403 && err.data && err.data.path) toast(`${err.data.error}: ${err.data.path}`, { icon: 'failed' });
    else toast(err.message, { icon: 'failed' });
    throw e;
  }
}

/** Two presses within 3.5 seconds fire the action (the second not sooner than 0.6 s after the first). */
export function confirmStep(key: string, fire: () => void | Promise<void>): void {
  const a = armed.value;
  if (!a || a.key !== key) {
    armed.value = { key, at: Date.now() };
    setTimeout(() => { if (armed.value?.key === key) armed.value = null; }, 3500);
    return;
  }
  if (Date.now() - a.at < 600) { toast(t('toast.pressAgain')); return; }
  armed.value = null;
  void fire();
}

const ANSWERING = new Set(['approve', 'anyway', 'retry', 'fix', 'finish', 'stop', 'review', 'delegate', 'pick']);

export function boardOk(): boolean {
  return snap.value?.board.ok !== false;
}

export function runTaskAction(cmd: string, task: Task, entity?: Entity): void {
  const galaxy = entity?.galaxy ?? galaxyId.value;
  if (!boardOk() && galaxy === '') { toast(t('toast.boardDown'), { icon: 'failed' }); return; }
  const done = async (msg: string): Promise<void> => {
    toast(msg);
    if (ANSWERING.has(cmd)) await afterAnswered(task.id);
  };
  if (cmd === 'approve' || cmd === 'anyway') {
    confirmStep(`${task.id}:${cmd}`, async () => {
      try {
        await sendAction(cmd, { id: task.id, confirm: true }, galaxy);
        await done(cmd === 'approve' ? t('toast.approved', { tag: task.tag }) : t('toast.anyway', { tag: task.tag }));
      } catch { /* the toast is shown */ }
    });
    return;
  }
  void (async () => {
    try {
      await sendAction(cmd, { id: task.id }, galaxy);
      await done(`${task.tag}: ${t(ACTION_LABEL[cmd] ?? cmd)}`);
    } catch { /* shown */ }
  })();
}

export async function tellTask(task: Task, text: string, galaxy?: string): Promise<boolean> {
  const body = text.trim();
  if (!body) return false;
  try {
    await sendAction('tell', { id: task.id, text: body }, galaxy);
    const d = { ...tellDraft.value };
    delete d[task.id];
    tellDraft.value = d;
    toast(t('toast.told', { tag: task.tag }));
    await afterAnswered(task.id);
    return true;
  } catch { return false; }
}

export async function pickAttempt(task: Task, letter: string): Promise<void> {
  try {
    await sendAction('pick', { id: task.id, letter });
    toast(t('toast.picked', { tag: task.tag, letter }));
    await afterAnswered(task.id);
  } catch { /* shown */ }
}

export async function openFiles(task: Task, kind: 'worktree' | 'reports'): Promise<void> {
  const local = ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(location.hostname);
  if (galaxyId.value !== '') { toast(t('toast.filesFar', { path: task.worktree || '—' })); return; }
  if (!local) {
    const d = details.value[task.id] ?? {};
    const p = kind === 'reports' ? d.reports_dir : d.worktree;
    toast(p ? t('toast.filesRemote', { path: p }) : t('toast.filesLater'));
    return;
  }
  try {
    const r = await sendAction('yazi', { id: task.id, kind });
    toast('yazi: ' + (r.path || t('toast.opened')));
  } catch { /* shown */ }
}

export async function archiveAllDone(): Promise<void> {
  try {
    const r = await sendAction('archive-done', { scope: 'all' });
    toast(t('toast.archivedN', { n: r.archived || 0 }));
  } catch { /* shown */ }
}

/* ───────── agent screen and files on another machine ───────── */

import { signal } from '@preact/signals';
import { onProgress } from './api';

export interface YaziState {
  present: boolean; path: string | null; os: string; arch: string; has_file: boolean; has_chafa: boolean;
  managers: { id: string; command: string; sudo: boolean; available: boolean; why_not: string | null }[];
  binary_url: string | null; notes: string[];
}
export const yaziDlg = signal<{ galaxy: string; task: Task; state: YaziState | null; busy: boolean; log: string[]; err: string; done: string } | null>(null);

export async function farScreen(task: Task): Promise<void> {
  try {
    const r = await post('/api/far/files', { galaxy: galaxyId.value, task: task.id, op: 'screen' });
    toast(t('far.screenOpened', { agent: r.agent ?? '' }));
  } catch (e) { toast((e as Error).message, { icon: 'failed' }); }
}

/** yazi of a task on another machine: opened if it is there, else the install dialog offers it. */
export async function farYazi(task: Task): Promise<void> {
  const galaxy = galaxyId.value;
  try {
    const c = await post<{ state: YaziState }>('/api/far/files', { galaxy, task: task.id, op: 'check' });
    if (c.state.present) {
      const r = await post('/api/far/files', { galaxy, task: task.id, op: 'yazi' });
      toast(t('far.yaziOpened', { path: r.path ?? '' }));
    } else {
      yaziDlg.value = { galaxy, task, state: c.state, busy: false, log: [], err: '', done: '' };
    }
  } catch (e) { toast((e as Error).message, { icon: 'failed' }); }
}

export function installYazi(method: string): void {
  const d = yaziDlg.value;
  if (!d) return;
  if (method === 'pacman') {
    void post('/api/far/files', { galaxy: d.galaxy, op: 'install-terminal', method }).then(() => {
      toast(t('far.terminalOpened'));
    }).catch((e: Error) => { yaziDlg.value = { ...d, err: e.message }; });
    return;
  }
  confirmStep(`yazi:${method}`, async () => {
    yaziDlg.value = { ...d, busy: true, log: [], err: '' };
    try {
      const r = await post<{ job: string }>('/api/far/files', { galaxy: d.galaxy, op: 'install', method, confirm: true });
      const off = onProgress((p) => {
        if (p.job !== r.job) return;
        const cur = yaziDlg.value;
        if (!cur) { off(); return; }
        if (p.steps?.[0]) yaziDlg.value = { ...cur, log: [...cur.log, p.steps[0].label] };
        if (p.done) {
          off();
          yaziDlg.value = { ...(yaziDlg.value ?? cur), busy: false, err: p.ok ? '' : p.error, done: p.ok ? p.result?.path ?? '' : '' };
          if (p.ok) void farYazi(d.task).then(() => { if (yaziDlg.value?.done) yaziDlg.value = null; });
        }
      });
    } catch (e) { yaziDlg.value = { ...d, busy: false, err: (e as Error).message }; }
  });
}

/* ───────── agent screens (herdr) ───────── */

export async function fetchScreen(target: string, role = ''): Promise<void> {
  const cur = screens.value[target];
  if (cur?.busy) return;
  screens.value = { ...screens.value, [target]: { ...(cur ?? {}), busy: true } };
  try {
    const r = await api('/api/coordinator/screen?target=' + encodeURIComponent(target) + (role ? '&role=' + encodeURIComponent(role) : ''));
    screens.value = { ...screens.value, [target]: { at: Date.now(), ...r, busy: false } };
  } catch (e) {
    screens.value = { ...screens.value, [target]: { error: (e as Error).message, busy: false } };
  }
}

export async function sendKey(target: string, key: string, role = ''): Promise<void> {
  try {
    await sendAction('keys', { target, keys: [key], role }, '');
    setTimeout(() => { void fetchScreen(target, role); }, 500);
    toast(t('toast.key', { key }));
  } catch { /* shown */ }
}

/* ───────── links (`after`) ───────── */

let linkQueue: Promise<unknown> = Promise.resolve();
function linkOp<T>(fn: () => Promise<T>): Promise<T> {
  const p = linkQueue.then(fn, fn);
  linkQueue = p.catch(() => {});
  return p;
}

const tagOf = (id: string, galaxy: string): string => taskOf(id, galaxy)?.tag ?? id;

async function saveAfter(id: string, refs: string[], undoRefs: string[] | null, galaxy: string): Promise<boolean> {
  try {
    await post('/api/action', { action: 'after', id, after: refs, galaxy: machineOf(galaxy) });
    setTimeout(() => { void refreshNow(); }, 500);
    toast(
      `${tagOf(id, galaxy)}: ${refs.length ? t('toast.afterSet', { tags: refs.map((r) => tagOf(r, galaxy)).join(', ') }) : t('toast.afterCleared')}`,
      { icon: 'link', timeout: 12000, label: t('act.undo'), action: undoRefs ? () => { void saveAfter(id, undoRefs, null, galaxy); } : undefined },
    );
    return true;
  } catch (e) {
    toast((e as Error).message, { icon: 'failed', timeout: 8000 });
    return false;
  }
}

export function connectTasks(from: string, to: string, galaxy: string): Promise<void> {
  return linkOp(async () => {
    await refreshNow();
    const view = galaxy === galaxyId.value ? currentView.value : null;
    const a = taskOf(from, galaxy);
    if (!a) { toast(t('toast.linkNoTask'), { icon: 'failed' }); return; }
    const current = (view?.links ?? []).filter((l) => l.to === from).map((l) => l.from);
    if (current.includes(to)) { toast(t('toast.linkExists', { a: a.tag, b: tagOf(to, galaxy) })); return; }
    await saveAfter(from, [...current, to], current, galaxy);
    await refreshNow();
  });
}

export function removeLink(waiting: string, from: string, galaxy: string): Promise<void> {
  return linkOp(async () => {
    await refreshNow();
    const view = galaxy === galaxyId.value ? currentView.value : null;
    const all = (view?.links ?? []).filter((l) => l.to === waiting).map((l) => l.from);
    await saveAfter(waiting, all.filter((x) => x !== from), all, galaxy);
    await refreshNow();
  });
}

/* ───────── the Coordinator's memory ───────── */

export function compactCoordinator(): void {
  confirmStep('coordinator:compact', async () => {
    try {
      await sendAction('coordinator-compact', { confirm: true }, '');
      toast(t('toast.compacting'));
    } catch { /* shown */ }
  });
}

export function newCoordinator(): void {
  confirmStep('coordinator:new', async () => {
    try {
      await sendAction('coordinator-new', { confirm: true }, '');
      toast(t('toast.newCoordinator'));
    } catch { /* shown */ }
  });
}

/* ───────── the list of todos ───────── */


async function todoCall(action: string, extra: Record<string, unknown>): Promise<any | null> {
  try {
    const r = await post('/api/action', { action, ...extra });
    if (r.todo && snap.value) snap.value = { ...snap.value, todo: r.todo };
    return r;
  } catch (e) {
    toast((e as Error).message, { icon: 'failed' });
    return null;
  }
}

export const todoAdd = (text: string, attachments: SavedFile[] = []): Promise<any> => todoCall('todo-add', { text, attachments });
/** Changes the text and/or the files (the whole set) of an item. */
export const todoUpdate = (id: string, patch: { text?: string; attachments?: SavedFile[] }): Promise<any> => todoCall('todo-update', { id, ...patch });
export const todoRemove = (id: string): Promise<any> => todoCall('todo-remove', { id });
export const todoReorder = (ids: string[]): Promise<any> => todoCall('todo-reorder', { ids });
export const todoAssign = (id: string, to: string): Promise<any> => todoCall('todo-assign', { id, to });

export async function todoSend(id: string): Promise<void> {
  const r = await todoCall('todo-send', { id });
  if (r) toast(t('toast.todoSent'));
}

export async function todoToTask(id: string, repo: string, agent: string): Promise<boolean> {
  const r = await todoCall('todo-create', { id, repo, agent: agent || undefined });
  if (r) { toast(t('toast.todoCreated', { repo }), { icon: 'universe' }); setTimeout(() => { void refreshNow(); }, 600); }
  return !!r;
}

/** Dropping an item on a thing: a planet asks what to tell its agent (a window to correct it first), the Quasar
    gets the message at once, a nebula asks where and by whom the new task is made. */
export async function todoDropOn(id: string, target: Entity): Promise<'sent' | 'asked' | 'tell' | 'none'> {
  if (target.kind === 'planet') return 'tell';
  if (target.kind === 'quasar') {
    await todoAssign(id, 'quasar');
    await todoSend(id);
    return 'sent';
  }
  if (target.kind === 'universe') return 'asked';
  return 'none';
}

/** The item becomes a draft of the message to the Coordinator, with its files, in the line. It stays in the list
    (marked) until the message is sent: a reload or a change of mind loses nothing. */
export async function todoToDraft(id: string): Promise<void> {
  const it = snap.value?.todo.find((x) => x.id === id);
  panels.chat.value = true;
  endDrag(true);
  if (!it) return;
  if (!draftMessage(id, it.text, it.attachments ?? [])) { toast(t('todo.alreadyDraft')); return; }
  toast(t('todo.draftNote'));
}

// A message sent from the line takes its todo items with it.
draftSent.fn = (ids) => {
  const have = new Set((snap.value?.todo ?? []).map((x) => x.id));
  ids.filter((id) => have.has(id)).forEach((id) => { void todoRemove(id); });
};
