/* Whether the page keeps the end of the Coordinator's chat in view (the pure part; the page is in ui/Chat.tsx).
   `stuck` is the page's follow flag, `pinned` is where the log stood the last time the page kept it at the end.

   A reader who moves the log — by a wheel, by «К последним», by a scroll of their own — must not be pulled back by a
   history that lands right then: the browser dispatches the scroll event of that move a frame later, and the turns may
   arrive in between. So the move is seen by the place of the log itself, not only by the event (PLN-109). */

/** How far the log may drift before it counts as moved by the reader: rounding and the scrollbar's own pixels. */
export const MOVED_PX = 2;

/** Keep pinning the end when the log has not moved since the last time it was pinned there. */
export function keepFollowing(stuck: boolean, pinned: number | null, top: number): boolean {
  if (!stuck) return false;
  // Never pinned yet (the first history of a page): followed, as it always was.
  if (pinned === null) return true;
  return Math.abs(top - pinned) <= MOVED_PX;
}
