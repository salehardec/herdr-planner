/* The keys of the rest room. Letters are read by physical key (`code`), like everywhere on the page, so «Щ» of the Russian
   layout opens the room as well as O. Which keys are taken elsewhere is kept in keys.test.ts: this one must stay free. */
import type { Pad } from './game';

/** Opens the room (the room's own keys ignore it: inside, it is just a letter). */
export const ROOM_KEY = 'KeyO';

const PADS: Record<string, Pad> = {
  ArrowLeft: 'left', KeyA: 'left', ArrowRight: 'right', KeyD: 'right', ArrowUp: 'up', ArrowDown: 'down',
  Space: 'a', KeyX: 'b', Enter: 'ok', NumpadEnter: 'ok', KeyP: 'start', KeyM: 'select',
};

export const padOf = (code: string): Pad | null => PADS[code] ?? null;

/** Keys the page must not scroll or click with while the room is open. */
export const swallowed = (code: string): boolean => code in PADS;

export interface KeyLike { code: string; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean; repeat?: boolean }

/** The key that opens the room: O alone, from anywhere but a text field (the caller has checked that). */
export const opensRoom = (e: KeyLike): boolean => e.code === ROOM_KEY && !e.ctrlKey && !e.metaKey && !e.altKey && !e.repeat;

/** Buttons of the panels (wallpapers, notices, exit) that Space and Enter press; anywhere else those keys go to the game. */
export const PANEL_BUTTONS = '#rm-wp button, #rm-notes button, #rm-exit button';
