/* The rest room's state: whether it is open, where the person came from, the wallpaper, the game. The room is a layer
   over the world (the world is not unloaded and the ship stands still), so entering it only remembers the place left
   and leaving brings the person back to exactly that place. The game outlives the visit: it waits on pause. */
import { signal } from '@preact/signals';
import { Game } from './game';
import { TouchPad } from './touch';
import { WALLS, WALL_DEFAULT } from './walls';

export const game = new Game();
export const touch = new TouchPad(game);

export const roomOpen = signal(false);

/** The place the person came from. Words for it are in the dictionary (restroom.back.*, restroom.where.*). */
export type Origin =
  | { kind: 'mobile' }
  | { kind: 'nav' }
  | { kind: 'galaxy' }
  | { kind: 'quasar' }
  | { kind: 'task'; tag: string }
  | { kind: 'universe'; name: string };

export const origin = signal<Origin>({ kind: 'galaxy' });

/** What the airlock says while it is shut. */
export type IrisLabel = { kind: 'in' } | { kind: 'back'; from: Origin } | { kind: 'fly'; tag: string };
/** The iris of the airlock: shut with a label while the scene changes behind it. */
export const iris = signal<{ shut: boolean; label: IrisLabel } | null>(null);

const KEY = 'herdr-planner.restroom.';
const read = (k: string): string | null => { try { return localStorage.getItem(KEY + k); } catch { return null; } };
const write = (k: string, v: string): boolean => { try { localStorage.setItem(KEY + k, v); return true; } catch { return false; } };

/** The person's own picture (a data: URL) when there is one. */
export const customWall = signal<string | null>(read('custom'));
const known = (id: string | null): string => (id === 'custom' && customWall.value ? 'custom' : WALLS.find((w) => w.id === id)?.id ?? WALL_DEFAULT);
export const wallId = signal<string>(known(read('wall')));

export function setWall(id: string): void {
  wallId.value = known(id);
  write('wall', wallId.value);
}

/** Takes the person's picture. A picture too big for the browser's storage works until the page is closed; false says so. */
export function setCustomWall(url: string): boolean {
  customWall.value = url;
  const kept = write('custom', url);
  setWall('custom');
  return kept;
}

/** The requests that were there when the room was entered, and the ones sent away with «Позже»: neither is a notice. */
export const noteBase = signal<ReadonlySet<string>>(new Set());
export const noteLater = signal<ReadonlySet<string>>(new Set());

/** Things the page wires in: it knows the world and the preferences, this module does not. */
export const hooks = {
  /** Calm mode or «reduce motion»: no airlock, no slide-in. */
  calm: (): boolean => typeof matchMedia !== 'function' || matchMedia('(prefers-reduced-motion: reduce)').matches,
  /** «Лететь»: out of the room and to the thing that asks. */
  fly: (_to: { type: string; id: string }): void => {},
  /** The room is about to open: the requests that wait now are not news. */
  entered: (): void => {},
};

let busy = false;
let opener: HTMLElement | null = null;
const timers: ReturnType<typeof setTimeout>[] = [];

/** The airlock: the doors close with a label, `mid` runs behind them, the doors open. */
function gate(label: IrisLabel, mid: () => void): void {
  if (hooks.calm()) { mid(); return; }
  busy = true;
  iris.value = { shut: true, label };
  timers.push(setTimeout(() => {
    mid();
    timers.push(setTimeout(() => { iris.value = { shut: false, label }; busy = false; }, 220));
  }, 480));
}

export const isBusy = (): boolean => busy;

export function enter(from: Origin, by: HTMLElement | null = null): void {
  if (roomOpen.value || busy) return;
  origin.value = from;
  opener = by;
  touch.clear();
  game.releaseAll();
  gate({ kind: 'in' }, () => {
    hooks.entered();
    roomOpen.value = true;
    game.enter();
  });
}

/** Out of the room: to the place it was entered from, or, with `to`, to the thing a notice is about. The game is paused. */
export function leave(to?: { type: string; id: string; tag: string }): void {
  if (!roomOpen.value || busy) return;
  game.pause(true);
  touch.clear();
  game.releaseAll();
  gate(to ? { kind: 'fly', tag: to.tag } : { kind: 'back', from: origin.value }, () => {
    roomOpen.value = false;
    const back = opener;
    opener = null;
    if (to) hooks.fly(to);
    else if (back && back.isConnected) back.focus({ preventScroll: true });
  });
}

/** For tests: forget timers and the open state. */
export function reset(): void {
  touch.clear();
  timers.forEach(clearTimeout);
  timers.length = 0;
  busy = false;
  opener = null;
  iris.value = null;
  roomOpen.value = false;
}
