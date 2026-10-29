/* The room's keyboard. While the room is open it owns every key: the game gets its buttons, Esc leaves, and nothing reaches
   the page behind (P would otherwise switch the flight, N close the navigator). It listens first (capture) and stops the
   others; a key-up is let through so that the ship never keeps a key that went down before the room opened. */
import { game, leave, roomOpen } from './state';
import { PANEL_BUTTONS, padOf } from './keymap';

function down(e: KeyboardEvent): void {
  if (!roomOpen.value) return;
  e.stopImmediatePropagation();
  if (e.code === 'Escape') { e.preventDefault(); leave(); return; }
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const pad = padOf(e.code);
  if (!pad) return;
  // Space and Enter on a button of a panel (wallpaper, notice, exit) press that button; anywhere else they are the game's
  if ((e.code === 'Space' || e.code === 'Enter') && (e.target as Element | null)?.closest?.(PANEL_BUTTONS)) return;
  e.preventDefault();
  game.down(pad, e.repeat);
}

function up(e: KeyboardEvent): void {
  const pad = padOf(e.code);
  if (pad) game.up(pad);
}

/** The window lost the screen: the game waits and no button stays held. */
function away(): void {
  game.releaseAll();
  game.pause(true);
}

export function installRoomInput(): void {
  addEventListener('keydown', down, true);
  addEventListener('keyup', up, true);
  addEventListener('blur', away);
  document.addEventListener('visibilitychange', () => { if (document.hidden) away(); });
  // A click on a button of the room leaves no focus on it, so that Space and Enter go on to the game; Tab keeps its focus.
  document.addEventListener('click', (e) => {
    const b = (e.target as Element | null)?.closest?.('#rm button');
    if (b && e.detail > 0 && !b.closest('#rm-console')) (b as HTMLElement).blur();
  });
}
