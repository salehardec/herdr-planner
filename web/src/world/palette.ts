/* Colours of the world: the Omarchy theme turned into three.js colours. */
import { Color } from 'three';
import type { Status } from '../types';

export interface Palette {
  mode: 'dark' | 'light';
  raw: Record<string, string>;
  bg: Color;
  fg: Color;
  fgBright: Color;
  muted: Color;
  accent: Color;
  status: Record<Status, Color>;
  /** One colour per arm of the galaxy (the folder of the projects). */
  arms: Color[];
  program: Record<string, Color>;
}

export const DEFAULT_COLORS: Record<string, string> = {
  bg: '#1a120c', 'bg-darker': '#0d0906', fg: '#ecded6', 'fg-bright': '#fcf5f1', muted: '#806f64',
  accent: '#dfa360', red: '#e48682', yellow: '#f6bb6f', green: '#98bf7a', cyan: '#5bcbcb',
  blue: '#77b0e9', magenta: '#da8fbf',
};

export const STATUS_COLOR_KEY: Record<Status, string> = {
  waiting: 'yellow', failed: 'red', ready: 'green', plan: 'cyan',
  review: 'magenta', working: 'blue', queued: 'muted', done: 'muted',
};

export const PROGRAM_COLOR_KEY: Record<string, string> = { claude: 'accent', omp: 'blue', pi: 'magenta', codex: 'green' };
export const PROGRAM_SHAPE: Record<string, 'sphere' | 'box' | 'octa' | 'tetra'> = {
  claude: 'sphere', omp: 'box', pi: 'octa', codex: 'tetra',
};

const color = (raw: Record<string, string>, key: string, fallback: string): Color => {
  const v = raw[key];
  return new Color(typeof v === 'string' && v.startsWith('#') ? v : fallback);
};

export function makePalette(colors: Record<string, string>, mode: 'dark' | 'light'): Palette {
  const raw = { ...DEFAULT_COLORS, ...colors };
  const c = (k: string): Color => color(raw, k, DEFAULT_COLORS[k] ?? '#888888');
  const status = {} as Record<Status, Color>;
  for (const [s, k] of Object.entries(STATUS_COLOR_KEY)) status[s as Status] = c(k);
  const program: Record<string, Color> = {};
  for (const [p, k] of Object.entries(PROGRAM_COLOR_KEY)) program[p] = c(k);
  return {
    mode, raw,
    bg: c(mode === 'light' ? 'bg' : 'bg-darker'),
    fg: c('fg'), fgBright: c('fg-bright'), muted: c('muted'), accent: c('accent'),
    status, program,
    arms: ['accent', 'blue', 'magenta', 'cyan', 'green', 'yellow'].map(c),
  };
}

export const hexOf = (col: Color): string => `#${col.getHexString()}`;
