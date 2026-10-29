/* Pointer ownership belongs to the pad, not the browser's click synthesis. Each finger releases only its own button;
   capture keeps holds working outside the hit area. Repeats use the game's frame clock, including in «Кирпичики». */
import type { Game, Pad } from './game';

export class TouchPad {
  private pointers = new Map<number, { pad: Pad; ms: number; next: number; released: () => void }>();
  constructor(private readonly game: Game) {}

  down(id: number, pad: Pad, released: () => void): void {
    if (this.pointers.has(id)) return;
    const held = [...this.pointers.values()].some((p) => p.pad === pad);
    this.pointers.set(id, { pad, ms: 0, next: 320, released });
    if (!held) this.game.down(pad);
  }

  up(id: number): void {
    const p = this.pointers.get(id);
    if (!p) return;
    this.pointers.delete(id);
    if (![...this.pointers.values()].some((other) => other.pad === p.pad)) this.game.up(p.pad);
    p.released();
  }

  clear(): void { for (const id of this.pointers.keys()) this.up(id); }

  advance(ms: number): void {
    const repeated = new Set<Pad>();
    for (const p of this.pointers.values()) {
      p.ms += Math.min(ms, 100);
      if (p.ms < p.next) continue;
      p.next = p.ms + 80;
      if (!repeated.has(p.pad) && (p.pad === 'left' || p.pad === 'right' || p.pad === 'down')) {
        this.game.down(p.pad, true);
        repeated.add(p.pad);
      }
    }
  }
}

export function bindPad(button: HTMLButtonElement, pad: Pad, touch: TouchPad): () => void {
  const ids = new Set<number>();
  const down = (e: PointerEvent): void => {
    if (e.button !== 0) return;
    e.preventDefault();
    button.setPointerCapture(e.pointerId);
    ids.add(e.pointerId);
    button.classList.add('dn');
    touch.down(e.pointerId, pad, () => {
      ids.delete(e.pointerId);
      if (!ids.size) button.classList.remove('dn');
      if (button.hasPointerCapture(e.pointerId)) button.releasePointerCapture(e.pointerId);
    });
  };
  const up = (e: PointerEvent): void => {
    e.preventDefault();
    if (ids.has(e.pointerId)) touch.up(e.pointerId);
  };
  button.addEventListener('pointerdown', down);
  for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) button.addEventListener(name, up as EventListener);
  return () => {
    for (const id of ids) touch.up(id);
    button.removeEventListener('pointerdown', down);
    for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) button.removeEventListener(name, up as EventListener);
  };
}
