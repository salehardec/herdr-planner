import { describe, expect, it } from 'vitest';
import { ru } from './ru';
import { en } from './en';

describe('i18n', () => {
  it('has the same keys in both languages', () => {
    const a = Object.keys(ru).sort();
    const b = Object.keys(en).sort();
    expect(a.filter((k) => !(k in en))).toEqual([]);
    expect(b.filter((k) => !(k in ru))).toEqual([]);
  });
  it('keeps the placeholders of every string', () => {
    const ph = (s: string | string[]): string[] => (String(s).match(/\{\w+\}/g) ?? []).sort();
    for (const k of Object.keys(ru)) expect(ph(en[k]), k).toEqual(ph(ru[k]));
  });
});

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? sources(p) : /\.(ts|tsx)$/.test(f) && !f.endsWith('.test.ts') && !p.includes('/i18n/') ? [p] : [];
  });
}

describe('i18n keys in the source', () => {
  it('every key used by t() exists', () => {
    const missing: string[] = [];
    for (const f of sources(join(__dirname, '..'))) {
      const text = readFileSync(f, 'utf8');
      for (const m of text.matchAll(/\bt[n]?\(\s*'([^']+)'/g)) if (!(m[1] in ru)) missing.push(`${f.split('/src/')[1]}: ${m[1]}`);
    }
    expect(missing).toEqual([]);
  });
  it('the dynamic families exist for every value', () => {
    for (const s of ['waiting', 'failed', 'ready', 'plan', 'review', 'working', 'queued', 'done']) expect(ru[`status.${s}`], s).toBeTruthy();
    for (const s of ['running', 'quiet', 'done']) { expect(ru[`moon.${s}`], s).toBeTruthy(); expect(ru[`moon.${s}.short`], s).toBeTruthy(); }
    for (const s of ['idle', 'accel', 'warp', 'decel']) expect(`flight.${s}` in ru, s).toBe(true);
  });
});
