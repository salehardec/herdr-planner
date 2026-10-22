/* Where the operator is, and how to go back.
   The rule of the whole page — «don't fly away from the place of action»:
   an action leaves the operator where they are; Esc is exactly one step back,
   like the browser's «back»: close a panel → leave the focus → leave the nebula.
   A move the operator makes themself (navigator, search, a click on a thing,
   a task key, manual flight) flies there and puts the place they left on the stack. */
import { batch, signal } from '@preact/signals';
import {
  answered, details, mode, panels, requests, selected, universe,
} from './store';
import type { CamState, World } from './world';
import type { Entity } from './types';
import { api } from './api';

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
}

const stack: Frame[] = [];
export const depth = signal(0);
/** The cursor in the «Ждёт тебя» list (the index of the request under it). */
export const queueCursor = signal(0);
/** The place being looked at was opened from the «Ждёт тебя» list. */
export const fromQueue = signal(false);

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
  batch(() => {
    selected.value = f.sel;
    panels.wait.value = f.wait;
    panels.todo.value = f.todo;
    panels.log.value = f.log;
    panels.archive.value = f.archive;
    panels.focus.value = f.focus;
    queueCursor.value = f.cursor;
    fromQueue.value = f.fromQueue;
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
  if (!world) return;
  if (o.push !== false) pushFrame();
  fromQueue.value = false;
  batch(() => {
    selected.value = e;
    panels.focus.value = true;
    panels.archive.value = false;
    if (e.kind === 'universe') universe.value = e.id;
  });
  world.setSelection(e);
  await world.flyTo({ kind: 'entity', entity: e });
}

/** Back to the whole Galaxy (the camera pulls out); the selection is dropped. */
export async function goOverview(o: { push?: boolean } = {}): Promise<void> {
  if (!world) return;
  if (o.push !== false) pushFrame();
  fromQueue.value = false;
  batch(() => { selected.value = null; });
  world.setSelection(null);
  await world.flyTo({ kind: 'overview' });
}

/** Home: the Galaxy as a whole. */
export async function goHome(): Promise<void> {
  if (!world) return;
  pushFrame();
  fromQueue.value = false;
  batch(() => { selected.value = null; });
  world.setSelection(null);
  await world.flyTo({ kind: 'overview' });
}

/** From the «Ждёт тебя» list into a request: the list and the camera are kept for the way back. */
export async function openRequest(index: number): Promise<void> {
  const r = requests.value.filter((x) => x.type !== 'coordinator-options')[index];
  if (!r) return;
  queueCursor.value = index;
  pushFrame();
  const entity: Entity = r.type === 'runner-alert'
    ? { kind: 'runner', id: r.id }
    : r.id === 'coordinator'
      ? { kind: 'quasar', id: 'quasar' }
      : { kind: 'planet', id: r.id };
  await navigateTo(entity, { push: false });
  fromQueue.value = true;
}

/** After an action that answers a request: out of it, back to the list with the cursor on the next one. */
export async function afterAnswered(id: string): Promise<void> {
  const set = new Set(answered.value);
  set.add(id);
  answered.value = set;
  setTimeout(() => {
    const s = new Set(answered.value);
    s.delete(id);
    answered.value = s;
  }, 5000);
  if (!fromQueue.value) return;
  const list = requests.value.filter((x) => x.type !== 'coordinator-options');
  const here = list.findIndex((r) => r.id === id);
  let next = here;
  for (let i = 1; i <= list.length; i++) {
    const k = (here + i) % list.length;
    if (!set.has(list[k].id) && k !== here) { next = k; break; }
  }
  await back();
  queueCursor.value = Math.max(0, next);
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
