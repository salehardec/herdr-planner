/* The part of the window the panels leave free: the navigator on the left, the focus card on the right, the chat or the todos over
   the middle. The world frames a nebula there (see world/framing.ts), so that it is not hidden under a panel.
   Where a panel stands is read from its layout (offsets, not the box that a slide-in still moves), and whether it is open from the
   page's state, so the answer is right in the same moment a panel is opened and the camera is sent. */
import { panels } from './store';
import { rootPx } from './scale';
import { freeRect, type Rect } from './world/framing';

interface Box { l: number; t: number; r: number; b: number }

function boxOf(el: HTMLElement | null): Box | null {
  if (!el || !el.offsetParent) return null;
  let l = 0, t = 0;
  for (let e: HTMLElement | null = el; e && e.id !== 'win'; e = e.offsetParent as HTMLElement | null) { l += e.offsetLeft; t += e.offsetTop; }
  return { l, t, r: l + el.offsetWidth, b: t + el.offsetHeight };
}

export function freeArea(): Rect | null {
  const win = document.getElementById('win');
  const scene = document.getElementById('scene');
  if (!win || !scene) return null;
  const W = scene.clientWidth || innerWidth;
  const H = scene.clientHeight || innerHeight;
  const gap = 0.5 * rootPx();
  const root = document.documentElement.classList;
  const wide = win.classList.contains('chat-wide') && !root.contains('bp1000');
  const chatOpen = panels.chat.value;
  const todoOpen = panels.todo.value;
  const ins = { l: 0, t: 0, r: 0, b: 0 };
  const topBar = boxOf(document.getElementById('top'));
  if (topBar) ins.t = topBar.b;

  const side = (el: string): void => {
    const b = boxOf(document.getElementById(el));
    if (!b) return;
    if (b.r - b.l > W * 0.6) { // a sheet over the whole width (narrow windows): above or below the picture
      if ((b.t + b.b) / 2 > H / 2) ins.b = Math.max(ins.b, H - b.t + gap); else ins.t = Math.max(ins.t, b.b + gap);
    } else if ((b.l + b.r) / 2 < W / 2) ins.l = Math.max(ins.l, b.r + gap);
    else ins.r = Math.max(ins.r, W - b.l + gap);
  };
  const middle = (el: string): void => {
    const b = boxOf(document.getElementById(el));
    if (!b) return;
    if ((b.t + b.b) / 2 > H / 2) ins.b = Math.max(ins.b, H - b.t + gap); else ins.t = Math.max(ins.t, b.b + gap);
  };

  if (panels.nav.value) side('nav');
  // The focus card steps aside for a chat or todos that have no room beside it (chatwide.css).
  const aside = win.classList.contains('chat-tight') && (chatOpen || todoOpen) || (wide && todoOpen && win.classList.contains('wide-tight'));
  if (panels.focus.value && !aside) side('focus');
  if (!wide) {
    if (chatOpen) middle('chat');
    if (todoOpen) middle('todo');
  }
  return freeRect(W, H, ins);
}
