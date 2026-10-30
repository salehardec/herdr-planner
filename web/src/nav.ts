/* Where the operator is, and how to go back.
   The rule of the whole page — «don't fly away from the place of action»:
   actions outside the waiting queue leave the operator where they are; Esc is exactly one step back,
   like the browser's «back»: close a panel → leave the focus → leave the nebula.
   A move the operator makes themself (navigator, search, a click on a thing,
   a task key, manual flight) flies there and puts the place they left on the stack. */
import { batch, signal } from '@preact/signals';
import {
  details, mode, panels, requests, selected, universe,
} from './store';
import type { CamState, World } from './world';
import type { Entity, Request } from './types';
import { api, refreshNow } from './api';
import { requestEntity } from './requests';
import { nextRequest, resolveRequest } from './request-queue';

let world: World | null = null;
export const setWorld = (w: World): void => { world = w; };
export const getWorld = (): World | null => world;

interface Frame {
  sel: Entity | null;
  cam: CamState | null;
  wait: boolean;
  todo: boolean;
  log: boolean;
  archive: boolean;
  focus: boolean;
  cursor: number;
  fromQueue: boolean;
  request: Request | null;
}

const stack: Frame[] = [];
export const depth = signal(0);
/** The cursor in the «Ждёт тебя» list (the index of the request under it). */
export const queueCursor = signal(0);
/** The place being looked at was opened from the «Ждёт тебя» list. */
export const fromQueue = signal(false);
export const queueRequest = signal<Request | null>(null);
let navigation = 0;
/** Additional surfaces (the phone) capture their location before the write starts. */
const answerNavigators = new Set<(r: Request, order: string[]) => (() => void) | undefined>();
export function onQueueAnswer(fn: (r: Request, order: string[]) => (() => void) | undefined): () => void {
  answerNavigators.add(fn);
  return () => { answerNavigators.delete(fn); };
}

/** Manual selection, including a link selected without flying, leaves queue traversal. */
export function leaveQueue(): void {
  navigation++;
  fromQueue.value = false;
  queueRequest.value = null;
}

function capture(): Frame {
  return {
    sel: selected.value,
    cam: world ? world.cameraState() : null,
    wait: panels.wait.value,
    todo: panels.todo.value,
    log: panels.log.value,
    archive: panels.archive.value,
    focus: panels.focus.value,
    cursor: queueCursor.value,
    fromQueue: fromQueue.value,
    request: queueRequest.value,
  };
}

export function pushFrame(): void {
  stack.push(capture());
  if (stack.length > 60) stack.shift();
  depth.value = stack.length;
}

export function clearStack(): void {
  stack.length = 0;
  depth.value = 0;
}

async function restore(f: Frame): Promise<void> {
  navigation++;
  batch(() => {
    selected.value = f.sel;
    panels.wait.value = f.wait;
    panels.todo.value = f.todo;
    panels.log.value = f.log;
    panels.archive.value = f.archive;
    panels.focus.value = f.focus;
    queueCursor.value = f.cursor;
    fromQueue.value = f.fromQueue;
    queueRequest.value = f.request;
  });
  if (!world) return;
  world.setSelection(f.sel);
  if (f.cam) await world.restoreCamera(f.cam);
}

/** One step back. False when there is nowhere to go back to. */
export async function back(): Promise<boolean> {
  const f = stack.pop();
  depth.value = stack.length;
  if (!f) return false;
  await restore(f);
  return true;
}

/** Goes to a thing: flies there and selects it. The place left is kept for Esc. */
export async function navigateTo(e: Entity, o: { push?: boolean } = {}): Promise<void> {
  if (o.push !== false) pushFrame();
  leaveQueue();
  batch(() => {
    selected.value = e;
    panels.focus.value = true;
    panels.archive.value = false;
    if (e.kind === 'universe') universe.value = e.id;
  });
  world?.setSelection(e);
  await world?.flyTo({ kind: 'entity', entity: e });
}

/** Back to the whole Galaxy (the camera pulls out); the selection is dropped. */
export async function goOverview(o: { push?: boolean } = {}): Promise<void> {
  if (o.push !== false) pushFrame();
  leaveQueue();
  batch(() => { selected.value = null; });
  world?.setSelection(null);
  await world?.flyTo({ kind: 'overview' });
}

/** Home: the Galaxy as a whole. */
export async function goHome(): Promise<void> {
  pushFrame();
  leaveQueue();
  batch(() => { selected.value = null; });
  world?.setSelection(null);
  await world?.flyTo({ kind: 'overview' });
}

/** Open a queue row. Automatic moves replace it without growing the Back stack. */
export async function openRequest(index: number, push = true): Promise<void> {
  const r = requests.value.filter((x) => x.type !== 'coordinator-options')[index];
  if (!r) return;
  if (push && !fromQueue.value) pushFrame();
  queueCursor.value = index;
  const flight = navigateTo(requestEntity(r), { push: false });
  fromQueue.value = true;
  queueRequest.value = r;
  await flight;
}

export async function stepRequest(delta: number): Promise<void> {
  const list = requests.value.filter((r) => r.type !== 'coordinator-options');
  if (!list.length) return;
  const at = list.findIndex((r) => r.key === queueRequest.value?.key);
  await openRequest((at + delta + list.length) % list.length);
}

/** Capture before the write: SSE may remove the row before its HTTP response arrives. Call only on success. */
export function prepareAnswer(id: string, kind: 'task' | 'memory' | 'coordinator' | 'runner' = 'task'): () => Promise<void> {
  const list = requests.value;
  const here = queueRequest.value;
  const r = kind === 'runner'
    // A yard may have several alarms: the one being worked on, else the first.
    ? (here?.type === 'runner-alert' && here.id === id ? list.find((r) => r.key === here.key) : undefined) ?? list.find((r) => r.type === 'runner-alert' && r.id === id)
    : kind === 'coordinator' ? list.find((r) => r.type === 'coordinator-dialog')
    : list.find((r) => r.id === id && (kind === 'memory' ? r.type === 'memory' : r.type !== 'memory'));
  const order = list.map((r) => r.key);
  const origin = navigation;
  const advance = r && fromQueue.value && queueRequest.value?.id === id;
  const extra = r ? [...answerNavigators].map((fn) => fn(r, order)).filter((fn) => !!fn) : [];
  let complete = false;
  return async () => {
    if (!r || complete) return;
    complete = true;
    const present = (): boolean => requests.value.some((current) => current.key === r.key);
    // An alarm stays until the yard says it is gone: read the board again (a read in flight may predate the action),
    // and if it is still there, the action did not clear it: no move.
    if (kind === 'runner') {
      for (let i = 0; i < 4 && present(); i++) {
        if (i) await new Promise((resolve) => setTimeout(resolve, 700));
        await refreshNow();
      }
      if (present()) return;
    }
    if (present()) resolveRequest(r);
    extra.forEach((fn) => fn?.());
    // A late response must not move a person who opened something else meanwhile.
    if (!advance || navigation !== origin || !fromQueue.value) return;
    const pending = requests.value.filter((r) => r.type !== 'coordinator-options');
    const next = nextRequest(order, r.key, pending);
    if (next) await openRequest(pending.findIndex((r) => r.key === next.key), false);
    else {
      navigation++;
      batch(() => {
        selected.value = null;
        queueRequest.value = null;
        panels.focus.value = true;
      });
      world?.setSelection(null);
    }
  };
}

/* ───────── details of a task ───────── */

const loading = new Set<string>();

export async function fetchDetails(id: string, force = false): Promise<void> {
  if (!id || loading.has(id) || (!force && details.value[id])) return;
  loading.add(id);
  try {
    const r = await api<{ details: any }>('/api/details?id=' + encodeURIComponent(id));
    details.value = { ...details.value, [id]: r.details };
  } catch (e) {
    details.value = { ...details.value, [id]: { error: (e as Error).message } };
  } finally {
    loading.delete(id);
  }
}

export const modeNow = (): 'auto' | 'manual' => mode.value;
