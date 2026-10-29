import { describe, expect, it } from 'vitest';
import { Game, BH, BW } from './game';

/** A game whose dice are fixed. */
const seeded = (): Game => { let n = 0.37; return new Game(() => (n = (n * 9301 + 0.49297) % 1)); };
const frames = (g: Game, n: number): void => { for (let i = 0; i < n; i++) g.step(); };

describe('the console', () => {
  it('shows the menu when the room is entered the first time, and starts a game on Space', () => {
    const g = seeded();
    expect(g.mode).toBe('off');
    g.enter();
    expect(g.mode).toBe('menu');
    g.press('down');
    expect(g.sel).toBe(1);
    g.press('up');
    g.press('a');
    expect(g.mode).toBe('shoot');
    expect(g.s?.lives).toBe(3);
  });

  it('keeps the game when the room is entered again: same score, still on pause', () => {
    const g = seeded();
    g.enter();
    g.press('a');
    g.s!.score = 120; g.s!.wave = 3;
    g.pause(true); // the room is left
    g.enter();     // and entered again
    expect(g.mode).toBe('shoot');
    expect(g.s).toMatchObject({ score: 120, wave: 3 });
    expect(g.paused).toBe(true);
    const x = g.s!.x;
    g.held.add('left');
    frames(g, 10);
    expect(g.s!.x).toBe(x); // on pause nothing moves
    g.press('start');
    expect(g.paused).toBe(false);
  });

  it('moves and fires only while the buttons are held', () => {
    const g = seeded();
    g.enter(); g.press('a');
    const x = g.s!.x;
    g.down('left'); frames(g, 5); g.up('left');
    expect(g.s!.x).toBeLessThan(x);
    g.down('a'); frames(g, 1);
    expect(g.s!.bul.length).toBe(1);
    g.up('a');
  });

  it('«ИГРА ОКОНЧЕНА» waits for a new press: a held Space does not restart it', () => {
    const g = seeded();
    g.enter(); g.press('a');
    g.down('a'); // fire is held when the last life goes
    g.s!.score = 70;
    g.s!.lives = 0; g.s!.over = true;
    g.down('a', true); g.down('a', true); // the keyboard repeats the held key
    frames(g, 30);
    expect(g.s!.over).toBe(true);
    expect(g.s!.score).toBe(70);
    g.up('a');
    g.down('a'); // a new press
    expect(g.s!.over).toBe(false);
    expect(g.s!.score).toBe(0);
  });

  it('M goes back to the menu and P toggles the pause only inside a game', () => {
    const g = seeded();
    g.enter();
    g.press('start');
    expect(g.paused).toBe(false); // nothing to pause in the menu
    g.press('a'); g.press('start');
    expect(g.paused).toBe(true);
    g.press('select');
    expect(g.mode).toBe('menu');
    expect(g.paused).toBe(false);
  });

  it('plays the shooter: the aliens come down and a lost life is counted', () => {
    const g = seeded();
    g.enter(); g.press('a');
    const y0 = g.s!.al[0].y;
    frames(g, 60 * 60); // a minute of nobody firing
    expect(g.s!.al.some((a) => a.y > y0) || g.s!.over).toBe(true);
    expect(g.s!.lives).toBeLessThan(3);
  });

  it('plays the bricks: a piece falls, turns, drops and locks', () => {
    const g = seeded();
    g.enter(); g.press('down'); g.press('a');
    expect(g.mode).toBe('bricks');
    const b = g.b!;
    expect(b.grid.length).toBe(BH);
    g.press('left'); g.press('up');
    g.press('a'); // hard drop
    expect(b.grid.flat().filter(Boolean).length).toBe(4);
    expect(b.grid[BH - 1].some(Boolean)).toBe(true);
    expect(b.grid[0].length).toBe(BW);
  });

  it('repeats left, right and down in the bricks, but not the drop or the pause', () => {
    const g = seeded();
    g.enter(); g.press('down'); g.press('a');
    const x = g.b!.p.x;
    g.down('left'); g.down('left', true);
    expect(g.b!.p.x).toBe(x - 2);
    g.down('start'); g.down('start', true);
    expect(g.paused).toBe(true); // pressed once, not twice
  });

  it('a stack to the top ends the bricks and a new press starts again', () => {
    const g = seeded();
    g.enter(); g.press('down'); g.press('a');
    for (let i = 0; i < 40 && !g.b!.over; i++) g.press('a');
    expect(g.b!.over).toBe(true);
    g.press('a');
    expect(g.b!.over).toBe(false);
    expect(g.b!.score).toBe(0);
  });

  it('long gaps between frames are cut, so a hidden tab does not run the game for minutes', () => {
    const g = seeded();
    g.enter(); g.press('a');
    const al = g.s!.al[0].x;
    g.advance(10 * 60 * 1000);
    expect(Math.abs(g.s!.al[0].x - al)).toBeLessThan(10);
  });
});
