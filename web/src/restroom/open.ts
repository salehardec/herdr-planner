/* Opening the room from the page: where the person is now (the place the room's «back» leads to) and what the page must
   let go of first. */
import { level, requests, selected, simple, taskOf, universe } from '../store';
import { navigateTo } from '../nav';
import { requestEntity } from '../requests';
import { baseKeys } from './notes';
import { enter, hooks, noteBase, noteLater, type Origin } from './state';

function here(): Origin {
  const sel = selected.value;
  if (sel?.kind === 'planet') return { kind: 'task', tag: taskOf(sel.id)?.tag ?? sel.id };
  if (sel?.kind === 'moon') { const id = sel.id.split('#')[0]; return id === 'quasar' ? { kind: 'quasar' } : { kind: 'task', tag: taskOf(id)?.tag ?? id }; }
  if (sel?.kind === 'quasar') return { kind: 'quasar' };
  if (sel?.kind === 'universe') return { kind: 'universe', name: sel.id };
  if (level.value === 'nebula' && universe.value) return { kind: 'universe', name: universe.value };
  return { kind: 'galaxy' };
}

/** `fromNav`: asked from the navigator, and that is where the person goes back to. `by`: the control that had the focus. */
export function openRestroom(by: HTMLElement | null = null, fromNav = false): void {
  // a mouse held by the ship (manual flight) is let go: the room is clicked
  if (document.pointerLockElement) document.exitPointerLock();
  enter(fromNav ? { kind: 'nav' } : here(), by && by !== document.body ? by : null);
  by?.blur();
}

hooks.calm = () => simple.value || matchMedia('(prefers-reduced-motion: reduce)').matches;
hooks.fly = (to) => { void navigateTo(requestEntity(to)); };
hooks.entered = () => { noteBase.value = baseKeys(requests.value); noteLater.value = new Set(); };
