/* The console of the rest room («Звёздный Досуг»): the menu and two games. No DOM here: the logic runs on a fixed step of
   60 Hz (`step`) and the pad is a set of buttons (`down` / `up`), so the whole thing can be played from a test.
   A game that is left stays where it is: `enter` starts the menu only when nothing was started yet. */

export const W = 240;
export const H = 224;

export type Mode = 'off' | 'menu' | 'shoot' | 'bricks';
/** The buttons of the console: the keyboard and the pad on the shell both end in these. */
export type Pad = 'left' | 'right' | 'up' | 'down' | 'a' | 'b' | 'ok' | 'start' | 'select';

export interface Alien { x: number; y: number; t: number; alive: boolean }
interface Shot { x: number; y: number }
interface Boom { x: number; y: number; t: number }
export interface Shooter {
  x: number; lives: number; score: number; wave: number;
  bul: Shot[]; ebul: Shot[]; inv: number; cool: number; over: boolean; boom: Boom[];
  al: Alien[]; dir: number; sp: number;
}

export const PIECES = [
  [[1, 1, 1, 1]], [[1, 1], [1, 1]], [[0, 1, 0], [1, 1, 1]], [[1, 0, 0], [1, 1, 1]], [[0, 0, 1], [1, 1, 1]], [[1, 1, 0], [0, 1, 1]], [[0, 1, 1], [1, 1, 0]],
];
export const BW = 10;
export const BH = 23;
export interface Piece { m: number[][]; x: number; y: number; c: number }
export interface Bricks { grid: number[][]; score: number; lines: number; t: number; over: boolean; p: Piece }

/** How many games the menu offers (the names are in the dictionary). */
export const GAMES = 2;

const FRAME = 1000 / 60;

export class Game {
  mode: Mode = 'off';
  sel = 0;
  paused = false;
  s: Shooter | null = null;
  b: Bricks | null = null;
  readonly held = new Set<Pad>();
  private acc = 0;

  constructor(private readonly rnd: () => number = Math.random) {}

  /** The room is entered: the menu, unless a game was started before (it is on pause and waits). */
  enter(): void { if (this.mode === 'off') this.menu(); }

  menu(): void { this.mode = 'menu'; this.paused = false; }

  pause(v?: boolean): void {
    if (this.mode === 'shoot' || this.mode === 'bricks') this.paused = v ?? !this.paused;
  }

  /** A button goes down. A held button repeats only where it means «again»: the pieces move and fall faster. */
  down(pad: Pad, repeat = false): void {
    if (!repeat || (this.mode === 'bricks' && (pad === 'left' || pad === 'right' || pad === 'down'))) this.press(pad);
    this.held.add(pad);
  }

  up(pad: Pad): void { this.held.delete(pad); }

  releaseAll(): void { this.held.clear(); }

  press(pad: Pad): void {
    if (this.mode === 'menu') {
      if (pad === 'up' || pad === 'down') this.sel = (this.sel + 1) % GAMES;
      if (pad === 'a' || pad === 'ok') this.start(this.sel);
      return;
    }
    if (this.mode === 'off') return;
    if (pad === 'start') { this.pause(); return; }
    if (pad === 'select') { this.menu(); return; }
    if (this.paused) return;
    // a game over waits for a new press: a held button does not get here (it repeats), so the score can be read
    if (this.mode === 'shoot' && this.s?.over && (pad === 'a' || pad === 'ok')) this.shootNew();
    if (this.mode === 'bricks') this.bricksKey(pad);
  }

  start(i: number): void {
    if (i === 0) { this.shootNew(); this.mode = 'shoot'; } else { this.bricksNew(); this.mode = 'bricks'; }
    this.paused = false;
  }

  /** Real time in, steps of 1/60 s out; a long gap (a hidden tab) is cut to a tenth of a second. */
  advance(ms: number): void {
    this.acc += Math.min(100, ms);
    while (this.acc >= FRAME) { this.acc -= FRAME; this.step(); }
  }

  step(): void {
    if (this.paused) return;
    if (this.mode === 'shoot') this.shootStep();
    else if (this.mode === 'bricks') this.bricksStep();
  }

  // ── Звёздный заслон
  shootNew(): void {
    this.s = {
      x: W / 2, lives: 3, score: 0, wave: 1, bul: [], ebul: [], inv: 0, cool: 0, over: false, boom: [], al: [], dir: 1, sp: 0,
    };
    this.wave();
  }

  private wave(): void {
    const S = this.s!;
    S.al = [];
    for (let r = 0; r < 4; r++) for (let c = 0; c < 8; c++) S.al.push({ x: 30 + c * 24, y: 22 + r * 16, t: r, alive: true });
    S.dir = 1;
    S.sp = 0.25 + S.wave * 0.08;
  }

  private shootStep(): void {
    const S = this.s!;
    if (S.over) return;
    const h = this.held;
    if (h.has('left')) S.x = Math.max(8, S.x - 2.2);
    if (h.has('right')) S.x = Math.min(W - 8, S.x + 2.2);
    if (S.cool > 0) S.cool--;
    if (h.has('a') && S.cool === 0) { S.bul.push({ x: S.x, y: H - 22 }); S.cool = 14; }
    S.bul.forEach((b) => { b.y -= 4; });
    S.bul = S.bul.filter((b) => b.y > 0);
    const live = S.al.filter((a) => a.alive);
    let edge = false;
    live.forEach((a) => { a.x += S.dir * S.sp * (1 + (32 - live.length) / 16); if (a.x < 10 || a.x > W - 10) edge = true; });
    if (edge) { S.dir *= -1; live.forEach((a) => { a.y += 6; }); }
    if (this.rnd() < 0.02 + S.wave * 0.004 && live.length) {
      const a = live[(this.rnd() * live.length) | 0];
      S.ebul.push({ x: a.x, y: a.y + 10 });
    }
    S.ebul.forEach((b) => { b.y += 1.8 + S.wave * 0.2; });
    S.ebul = S.ebul.filter((b) => b.y < H);
    for (const b of S.bul) {
      for (const a of live) {
        if (a.alive && Math.abs(b.x - a.x) < 8 && Math.abs(b.y - a.y - 5) < 6) {
          a.alive = false; b.y = -9; S.score += (4 - a.t) * 10; S.boom.push({ x: a.x, y: a.y + 5, t: 12 });
        }
      }
    }
    if (S.inv > 0) S.inv--;
    for (const b of S.ebul) {
      if (S.inv === 0 && Math.abs(b.x - S.x) < 7 && b.y > H - 22 && b.y < H - 10) {
        b.y = H + 9; S.lives--; S.inv = 90; S.boom.push({ x: S.x, y: H - 16, t: 16 });
        if (S.lives <= 0) S.over = true;
      }
    }
    if (live.some((a) => a.alive && a.y > H - 30)) { S.lives = 0; S.over = true; }
    if (!S.al.some((a) => a.alive)) { S.wave++; this.wave(); }
    S.boom.forEach((b) => { b.t--; });
    S.boom = S.boom.filter((b) => b.t > 0);
  }

  // ── Кирпичики
  bricksNew(): void {
    this.b = { grid: Array.from({ length: BH }, () => Array<number>(BW).fill(0)), score: 0, lines: 0, t: 0, over: false, p: { m: [[1]], x: 0, y: 0, c: 1 } };
    this.spawn();
  }

  private spawn(): void {
    const B = this.b!;
    const i = (this.rnd() * PIECES.length) | 0;
    B.p = { m: PIECES[i].map((r) => [...r]), x: 3, y: 0, c: i + 1 };
    if (this.hit(B.p.m, B.p.x, B.p.y)) B.over = true;
  }

  private hit(m: number[][], x: number, y: number): boolean {
    const B = this.b!;
    return m.some((r, j) => r.some((v, i) => v && (x + i < 0 || x + i >= BW || y + j >= BH || B.grid[y + j]?.[x + i])));
  }

  private lock(): void {
    const B = this.b!;
    B.p.m.forEach((r, j) => r.forEach((v, i) => { if (v) B.grid[B.p.y + j][B.p.x + i] = B.p.c; }));
    let n = 0;
    B.grid = B.grid.filter((r) => { if (r.every(Boolean)) { n++; return false; } return true; });
    while (B.grid.length < BH) B.grid.unshift(Array<number>(BW).fill(0));
    B.lines += n;
    B.score += [0, 100, 300, 500, 800][n];
    this.spawn();
  }

  private bricksKey(pad: Pad): void {
    const B = this.b!;
    if (B.over) { if (pad === 'a' || pad === 'ok') this.bricksNew(); return; }
    const p = B.p;
    if (pad === 'left' && !this.hit(p.m, p.x - 1, p.y)) p.x--;
    if (pad === 'right' && !this.hit(p.m, p.x + 1, p.y)) p.x++;
    if (pad === 'down' && !this.hit(p.m, p.x, p.y + 1)) p.y++;
    if (pad === 'up') {
      const m = p.m[0].map((_, i) => p.m.map((r) => r[i]).reverse());
      if (!this.hit(m, p.x, p.y)) p.m = m;
    }
    if (pad === 'a') { while (!this.hit(p.m, p.x, p.y + 1)) p.y++; this.lock(); }
  }

  private bricksStep(): void {
    const B = this.b!;
    if (B.over) return;
    if (++B.t > Math.max(8, 34 - B.lines)) {
      B.t = 0;
      if (this.hit(B.p.m, B.p.x, B.p.y + 1)) this.lock(); else B.p.y++;
    }
  }
}
