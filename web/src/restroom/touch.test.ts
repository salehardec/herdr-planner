import { describe, expect, it, vi } from 'vitest';
import { Game } from './game';
import { bindPad, TouchPad } from './touch';

function setup() {
  const game = new Game(() => .5);
  game.enter();
  return { game, touch: new TouchPad(game) };
}

// Native pointer events without a DOM dependency. Capture and cancellation are the browser contract bindPad uses.
class Button extends EventTarget {
  held = new Set<number>();
  classes = new Set<string>();
  classList = { add: (c: string) => this.classes.add(c), remove: (c: string) => this.classes.delete(c) };
  setPointerCapture(id: number) { this.held.add(id); }
  hasPointerCapture(id: number) { return this.held.has(id); }
  releasePointerCapture(id: number) { this.held.delete(id); }
  pointer(type: string, id = 1, button = 0) {
    const e = Object.assign(new Event(type, { cancelable: true }), { pointerId: id, button });
    this.dispatchEvent(e);
    return e;
  }
}

describe('console touch input', () => {
  it('starts with A and holds movement and fire simultaneously; releasing one finger leaves the other held', () => {
    const { game, touch } = setup();
    touch.down(1, 'a', () => {});
    expect(game.mode).toBe('shoot');
    touch.down(2, 'left', () => {});
    const x = game.s!.x;
    game.advance(100);
    expect(game.s!.x).toBeLessThan(x);
    expect(game.s!.bul.length).toBeGreaterThan(0);
    touch.up(2);
    expect([...game.held]).toEqual(['a']);
    const stopped = game.s!.x;
    game.advance(100);
    expect(game.s!.x).toBe(stopped);
    touch.up(1);
    expect(game.held.size).toBe(0);
  });

  it('owns each finger, including two on one button, and clears every held state on interruption', () => {
    const { game, touch } = setup();
    const released = vi.fn();
    touch.down(1, 'down', released);
    touch.down(2, 'down', released);
    expect(game.sel).toBe(1);
    touch.up(1);
    expect(game.held.has('down')).toBe(true);
    touch.clear();
    expect(game.held.size).toBe(0);
    expect(released).toHaveBeenCalledTimes(2);
    touch.up(2);
    expect(released).toHaveBeenCalledTimes(2);
  });

  it('repeats a held direction in bricks but never repeats rotate, drop, pause or menu selection', () => {
    const { game, touch } = setup();
    touch.down(1, 'down', () => {});
    for (let i = 0; i < 12; i++) touch.advance(100);
    expect(game.sel).toBe(1);
    touch.clear();
    touch.down(2, 'a', () => {});
    expect(game.mode).toBe('bricks');
    touch.clear();
    touch.down(3, 'left', () => {});
    const x = game.b!.p.x;
    for (let i = 0; i < 4; i++) touch.advance(100);
    expect(game.b!.p.x).toBeLessThan(x);
    touch.clear();
    touch.down(4, 'up', () => {});
    const piece = JSON.stringify(game.b!.p.m);
    touch.down(5, 'start', () => {});
    for (let i = 0; i < 12; i++) touch.advance(100);
    expect(JSON.stringify(game.b!.p.m)).toBe(piece);
    expect(game.paused).toBe(true);
  });

  for (const end of ['pointerup', 'pointercancel', 'lostpointercapture']) {
    it(`captures a pointer and releases on ${end}, without page scrolling or a stuck button`, () => {
      const { game, touch } = setup();
      const button = new Button();
      const unbind = bindPad(button as unknown as HTMLButtonElement, 'left', touch);
      expect(button.pointer('pointerdown').defaultPrevented).toBe(true);
      expect(button.hasPointerCapture(1)).toBe(true);
      expect(button.classes.has('dn')).toBe(true);
      expect(game.held.has('left')).toBe(true);
      button.pointer('pointerleave');
      expect(game.held.has('left')).toBe(true);
      button.pointer(end);
      expect(game.held.size).toBe(0);
      expect(button.classes.size).toBe(0);
      expect(button.held.size).toBe(0);
      unbind();
    });
  }

  it('releases capture and pressed appearance on blur/exit cleanup, and ignores secondary mouse buttons', () => {
    const { game, touch } = setup();
    const button = new Button();
    const unbind = bindPad(button as unknown as HTMLButtonElement, 'a', touch);
    button.pointer('pointerdown', 1, 2);
    expect(game.mode).toBe('menu');
    button.pointer('pointerdown', 2);
    touch.clear();
    expect(button.classes.size).toBe(0);
    expect(button.held.size).toBe(0);
    button.pointer('pointerdown', 3);
    unbind();
    expect(game.held.size).toBe(0);
    button.pointer('pointerdown', 4);
    expect(game.held.size).toBe(0);
  });
});
