/* A todo card being dragged: from the list into the world (a planet, a nebula, the Quasar), into the chat,
   or inside the list. The chat is the drop place at the bottom: it opens by itself while a card is in the air
   and folds back if nothing was dropped into it. */
import { signal } from '@preact/signals';
import { panels } from './store';

/** The id of the item being dragged. */
export const dragTodo = signal<string | null>(null);
export const overId = signal<string | null>(null);

let chatOpenedByDrag = false;
let timer = 0;

export function startDrag(id: string): void {
  dragTodo.value = id;
  window.clearTimeout(timer);
  // Not inside dragstart itself: a page that changes while the drag begins may lose it.
  timer = window.setTimeout(() => {
    if (dragTodo.value === id && !panels.chat.value) { panels.chat.value = true; chatOpenedByDrag = true; }
  }, 0);
}

/** The card is down (dropped or let go). `keepChat`: it was dropped into the chat, which stays open. */
export function endDrag(keepChat = false): void {
  window.clearTimeout(timer);
  dragTodo.value = null;
  overId.value = null;
  if (chatOpenedByDrag && !keepChat) panels.chat.value = false;
  chatOpenedByDrag = false;
}
