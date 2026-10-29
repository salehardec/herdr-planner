/* The screen of the console: what the games look like. Pixels of a 240×224 canvas; the words come from the dictionary. */
import { t } from '../i18n';
import { BH, BW, GAMES, H, W, type Game } from './game';

const PAL = { bg: '#0b1a12', fg: '#9bf0b0', dim: '#3f7a52', hi: '#f6e27a', red: '#ff6b6b' };
const ALIEN_INK = [PAL.fg, '#7ad8f0', '#f0a07a', '#d88af0'];
const BRICK_INK = ['', '#9bf0b0', '#f6e27a', '#d88af0', '#7ad8f0', '#f0a07a', '#ff6b6b', '#7aa8f0'];

const SHIP = ['...#...', '..###..', '.#####.', '##.#.##', '#######'];
const ALIEN = [
  ['.#...#.', '..###..', '.#####.', '##.#.##', '#.#.#.#'], ['#.###.#', '#######', '##.#.##', '.#####.', '.#...#.'],
  ['..###..', '.#####.', '#.#.#.#', '#######', '.#.#.#.'], ['.##.##.', '#######', '#.###.#', '.#...#.', '#.....#'],
];

type Ctx = CanvasRenderingContext2D;

function txt(g: Ctx, s: string, x: number, y: number, c = PAL.fg, sz = 8, al: CanvasTextAlign = 'center'): void {
  g.fillStyle = c; g.font = `bold ${sz}px monospace`; g.textAlign = al; g.fillText(s, x, y);
}

function spr(g: Ctx, p: string[], x: number, y: number, c: string, s = 2): void {
  g.fillStyle = c;
  p.forEach((row, j) => [...row].forEach((ch, i) => { if (ch === '#') g.fillRect(Math.round(x + (i - 3.5) * s), Math.round(y + j * s), s, s); }));
}

function shooter(g: Ctx, game: Game, now: number): void {
  const S = game.s!;
  txt(g, `${t('restroom.game.score')} ${String(S.score).padStart(5, '0')}`, 6, 10, PAL.fg, 8, 'left');
  txt(g, `${t('restroom.game.wave')} ${S.wave}`, W / 2, 10, PAL.dim);
  for (let i = 0; i < S.lives; i++) spr(g, SHIP, W - 12 - i * 14, 3, PAL.hi, 1.2);
  const fr = ((now / 400) | 0) % 2;
  S.al.forEach((a) => { if (a.alive) spr(g, fr && a.t % 2 ? ALIEN[(a.t + 1) % 4] : ALIEN[a.t], a.x, a.y, ALIEN_INK[a.t]); });
  if (!(S.inv && (S.inv >> 2) % 2)) spr(g, SHIP, S.x, H - 20, PAL.hi);
  g.fillStyle = PAL.hi; S.bul.forEach((b) => g.fillRect(b.x - 1, b.y, 2, 5));
  g.fillStyle = PAL.red; S.ebul.forEach((b) => g.fillRect(b.x - 1, b.y, 2, 4));
  S.boom.forEach((b) => {
    g.fillStyle = b.t % 4 < 2 ? PAL.hi : PAL.red;
    for (let k = 0; k < 6; k++) { const an = (k / 6) * 6.28; g.fillRect(b.x + Math.cos(an) * (16 - b.t), b.y + Math.sin(an) * (16 - b.t), 2, 2); }
  });
  g.fillStyle = PAL.dim; g.fillRect(0, H - 6, W, 1);
  if (S.over) {
    g.fillStyle = 'rgba(0,0,0,.6)'; g.fillRect(0, 0, W, H);
    txt(g, t('restroom.game.over'), W / 2, 80, PAL.red, 12);
    txt(g, `${t('restroom.game.score.low')} ${S.score}`, W / 2, 98);
    txt(g, t('restroom.game.again'), W / 2, 120, PAL.dim);
  }
}

function bricks(g: Ctx, game: Game): void {
  const B = game.b!;
  const CS = 9;
  const ox = (W - BW * CS) / 2 - 30;
  const oy = 8;
  g.strokeStyle = PAL.dim; g.strokeRect(ox - 1.5, oy - 1.5, BW * CS + 3, BH * CS + 3);
  const cell = (x: number, y: number, c: number): void => { g.fillStyle = BRICK_INK[c]; g.fillRect(ox + x * CS, oy + y * CS, CS - 1, CS - 1); };
  B.grid.forEach((r, y) => r.forEach((v, x) => { if (v) cell(x, y, v); }));
  B.p.m.forEach((r, j) => r.forEach((v, i) => { if (v) cell(B.p.x + i, B.p.y + j, B.p.c); }));
  const tx = ox + BW * CS + 14;
  txt(g, t('restroom.game.bricks'), tx, 20, PAL.hi, 8, 'left');
  txt(g, `${t('restroom.game.score.low')} ${B.score}`, tx, 40, PAL.fg, 8, 'left');
  txt(g, `${t('restroom.game.lines')} ${B.lines}`, tx, 54, PAL.fg, 8, 'left');
  txt(g, t('restroom.game.rotate'), tx, 90, PAL.dim, 7, 'left');
  txt(g, t('restroom.game.drop'), tx, 102, PAL.dim, 7, 'left');
  if (B.over) {
    g.fillStyle = 'rgba(0,0,0,.6)'; g.fillRect(0, 0, W, H);
    txt(g, t('restroom.game.stacked'), W / 2, 84, PAL.red, 12);
    txt(g, t('restroom.game.again'), W / 2, 104, PAL.dim);
  }
}

function menu(g: Ctx, game: Game): void {
  txt(g, t('restroom.game.title'), W / 2, 30, PAL.hi, 12);
  for (let i = 0; i < 3; i++) spr(g, ALIEN[i], W / 2 - 40 + i * 40, 44, PAL.dim);
  const items = [
    { n: t('restroom.game.shoot'), d: t('restroom.game.shoot.d') },
    { n: t('restroom.game.bricks'), d: t('restroom.game.bricks.d') },
  ].slice(0, GAMES);
  items.forEach((it, i) => {
    const y = 108 + i * 30;
    const s = i === game.sel;
    if (s) { g.fillStyle = PAL.dim; g.fillRect(30, y - 11, W - 60, 22); }
    txt(g, (s ? '▶ ' : '  ') + it.n, W / 2, y - 1, s ? PAL.hi : PAL.fg, 9);
    txt(g, it.d, W / 2, y + 8, s ? PAL.fg : PAL.dim, 7);
  });
  txt(g, t('restroom.game.menuHint'), W / 2, H - 10, PAL.dim, 7);
}

/** One frame of the screen. */
export function drawScreen(g: Ctx, game: Game, now: number): void {
  g.fillStyle = PAL.bg; g.fillRect(0, 0, W, H);
  if (game.mode === 'menu') menu(g, game);
  else if (game.mode === 'shoot' && game.s) shooter(g, game, now);
  else if (game.mode === 'bricks' && game.b) bricks(g, game);
  if (game.paused) {
    g.fillStyle = 'rgba(0,0,0,.55)'; g.fillRect(0, 0, W, H);
    txt(g, t('restroom.game.paused'), W / 2, H / 2, PAL.hi, 14);
    txt(g, t('restroom.game.resume'), W / 2, H / 2 + 16, PAL.fg, 7);
  }
  g.fillStyle = 'rgba(0,0,0,.12)';
  for (let y = 0; y < H; y += 2) g.fillRect(0, y, W, 1);
}
