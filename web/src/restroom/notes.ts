/* Notices in the rest room: the requests of «Ждёт тебя» that came after the room was entered (what was waiting before is
   the person's reason to rest, not news), minus the ones sent away with «Позже». */

interface Asks { key: string; type: string }

/** Requests that are worth a card in the room: a task's or the Coordinator's own, not the options list or a memory proposal. */
export const noticeable = (r: Asks): boolean => r.type !== 'coordinator-options' && r.type !== 'memory';

export function freshNotes<T extends Asks>(base: ReadonlySet<string>, later: ReadonlySet<string>, list: readonly T[]): T[] {
  return list.filter((r) => noticeable(r) && !base.has(r.key) && !later.has(r.key));
}

/** The keys that are already there when the room is entered. */
export const baseKeys = (list: readonly Asks[]): Set<string> => new Set(list.map((r) => r.key));
