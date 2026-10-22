/* The colours of a terminal screen: `herdr agent read --format ansi` sends SGR sequences, and the
   window over the page draws them as spans. Only colour and emphasis are taken; anything else in a
   sequence (a cursor move, a window title) is dropped, and nothing from the screen becomes markup. */

export interface AnsiSpan {
  text: string;
  /** A CSS colour (a theme token for the sixteen terminal colours, a hex value for the rest). */
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
}

/** The sixteen ANSI colours in the page's own theme, so a screen reads in a light theme too. */
const FG: Record<number, string> = {
  30: 'var(--c-muted)', 31: 'var(--c-red)', 32: 'var(--c-green)', 33: 'var(--c-yellow)',
  34: 'var(--c-blue)', 35: 'var(--c-magenta)', 36: 'var(--c-cyan)', 37: 'var(--c-fg)',
};
const FG_BRIGHT: Record<number, string> = {
  90: 'var(--c-fg-dim)', 91: 'var(--c-red)', 92: 'var(--c-green)', 93: 'var(--c-yellow)',
  94: 'var(--c-blue)', 95: 'var(--c-magenta)', 96: 'var(--c-cyan)', 97: 'var(--c-fg-bright)',
};
const BG: Record<number, string> = {
  40: 'var(--c-bg-darker)', 41: 'var(--c-red)', 42: 'var(--c-green)', 43: 'var(--c-yellow)',
  44: 'var(--c-blue)', 45: 'var(--c-magenta)', 46: 'var(--c-cyan)', 47: 'var(--c-fg-dim)',
  100: 'var(--c-bg-dark)', 101: 'var(--c-red)', 102: 'var(--c-green)', 103: 'var(--c-yellow)',
  104: 'var(--c-blue)', 105: 'var(--c-magenta)', 106: 'var(--c-cyan)', 107: 'var(--c-fg)',
};

/** The xterm palette beyond the sixteen named colours. */
const XTERM = [
  '#000000', '#cd0000', '#00cd00', '#cdcd00', '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00', '#5c5cff', '#ff00ff', '#00ffff', '#ffffff',
];

function xterm256(n: number): string | undefined {
  if (!Number.isInteger(n) || n < 0 || n > 255) return undefined;
  if (n < 16) return XTERM[n];
  if (n < 232) {
    const i = n - 16;
    const steps = [0, 95, 135, 175, 215, 255];
    const hex = (v: number): string => v.toString(16).padStart(2, '0');
    return `#${hex(steps[Math.floor(i / 36)])}${hex(steps[Math.floor(i / 6) % 6])}${hex(steps[i % 6])}`;
  }
  const v = 8 + (n - 232) * 10;
  const hex = v.toString(16).padStart(2, '0');
  return `#${hex}${hex}${hex}`;
}

function trueColor(r: number, g: number, b: number): string | undefined {
  if (![r, g, b].every((v) => Number.isInteger(v) && v >= 0 && v <= 255)) return undefined;
  return `#${[r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('')}`;
}

interface Sgr {
  fg?: string; bg?: string; bold?: boolean; dim?: boolean; italic?: boolean; underline?: boolean; inverse?: boolean;
}

function applyCodes(state: Sgr, codes: number[]): void {
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) { for (const k of Object.keys(state) as (keyof Sgr)[]) delete state[k]; }
    else if (c === 1) state.bold = true;
    else if (c === 2) state.dim = true;
    else if (c === 3) state.italic = true;
    else if (c === 4) state.underline = true;
    else if (c === 7) state.inverse = true;
    else if (c === 22) { delete state.bold; delete state.dim; }
    else if (c === 23) delete state.italic;
    else if (c === 24) delete state.underline;
    else if (c === 27) delete state.inverse;
    else if (c === 39) delete state.fg;
    else if (c === 49) delete state.bg;
    else if (FG[c]) state.fg = FG[c];
    else if (FG_BRIGHT[c]) state.fg = FG_BRIGHT[c];
    else if (BG[c]) state.bg = BG[c];
    else if (c === 38 || c === 48) {
      const into = c === 38 ? 'fg' : 'bg';
      if (codes[i + 1] === 5) {
        const color = xterm256(codes[i + 2]);
        if (color) state[into] = color;
        i += 2;
      } else if (codes[i + 1] === 2) {
        const color = trueColor(codes[i + 2], codes[i + 3], codes[i + 4]);
        if (color) state[into] = color;
        i += 4;
      }
    }
  }
}

/**
 * The screen as spans of text with what colours them. `\r` and the other control characters a
 * terminal would act on are dropped, except the line breaks and tabs a `<pre>` shows.
 */
export function ansiSpans(src: string): AnsiSpan[] {
  const out: AnsiSpan[] = [];
  const state: Sgr = {};
  let text = '';
  const flush = (): void => {
    if (!text) return;
    out.push({ text, ...state });
    text = '';
  };
  const s = src.replace(/\r\n?/g, '\n');
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c !== '\u001b') {
      if (c === '\n' || c === '\t' || c >= ' ') text += c;
      continue;
    }
    const next = s[i + 1];
    if (next === '[') {
      let j = i + 2;
      while (j < s.length && !(s[j] >= '@' && s[j] <= '~')) j++;
      if (j >= s.length) { i = j; break; } // a sequence cut by the limit: nothing to draw
      if (s[j] === 'm') {
        flush();
        const body = s.slice(i + 2, j);
        const codes = body === '' ? [0] : body.split(';').map((p) => Number.parseInt(p, 10) || 0);
        applyCodes(state, codes);
      }
      i = j;
      continue;
    }
    if (next === ']') {
      let j = i + 2;
      while (j < s.length && s[j] !== '\u0007' && !(s[j] === '\u001b' && s[j + 1] === '\\')) j++;
      i = s[j] === '\u001b' ? j + 1 : j;
      continue;
    }
    i += 1; // any other two-byte escape
  }
  flush();
  return out;
}
