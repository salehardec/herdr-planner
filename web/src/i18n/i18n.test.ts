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
import { scan } from './scan';
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

describe('words outside the dictionary', () => {
  const found = (): string[] => sources(join(__dirname, '..')).flatMap((f) =>
    scan(f.split('/src/')[1], readFileSync(f, 'utf8')).map((x) => `${x.file}:${x.line} ${x.where}: «${x.text}»`));
  it('no text, title, placeholder, aria-label or toast is written into the source', () => {
    expect(found()).toEqual([]);
  });
  it('the scan itself sees what it must', () => {
    const hit = scan('x.tsx', "export const A = () => <button title=\"Close it\" data-x=\"keep\">Compact</button>;").map((x) => x.text);
    expect(hit).toEqual(['Close it', 'Compact']);
    expect(scan('x.tsx', "toast('Saved now'); toast(t('a')); toast(`Saved ${n}`);").length).toBe(2);
    expect(scan('x.tsx', "export const A = () => <kbd>Ctrl</kbd>;")).toEqual([]);
    expect(scan('x.tsx', "export const A = () => <span>{t('k')}</span>;")).toEqual([]);
    // the places the first version missed
    expect(scan('x.tsx', "export const A = () => <b>{ok ? 'Saved' : 'Failed to save'}</b>;").map((x) => x.text)).toEqual(['Saved', 'Failed to save']);
    expect(scan('x.ts', "const o = { label: 'Open the file', id: 'x' };").map((x) => x.text)).toEqual(['Open the file']);
    expect(scan('x.ts', "el.textContent = 'Loading data'; el.dataset.x = 'two words';").map((x) => x.text)).toEqual(['Loading data']);
    expect(scan('x.tsx', "export const A = () => <b>{ok ? t('a') : t('b')}</b>;")).toEqual([]);
    expect(scan('x.tsx', "export const A = () => <b title={ok ? 'Close it' : 'Open it'} class={ok ? 'on top' : 'off'}>x</b>;").map((x) => x.text)).toEqual(['Close it', 'Open it']);
    expect(scan('x.tsx', "export const A = () => <b>{ok && 'Saved now'}{x || 'Nothing here'}</b>;").map((x) => x.text)).toEqual(['Saved now', 'Nothing here']);
    expect(scan('x.tsx', "export const A = () => <b>{ok && <i>{t('a')}</i>}{list.length > 3 && t('b')}</b>;")).toEqual([]);
  });
});

describe('the pages of the login and the cabin', () => {
  it('every data-i18n key of the HTML pages is in both dictionaries', () => {
    const root = join(__dirname, '..', '..');
    const missing: string[] = [];
    for (const page of ['index.html', 'login.html']) {
      const html = readFileSync(join(root, page), 'utf8');
      for (const m of html.matchAll(/data-i18n="([^"]+)"/g)) if (!(m[1] in ru) || !(m[1] in en)) missing.push(`${page}: ${m[1]}`);
    }
    expect(missing).toEqual([]);
  });
});

describe('the Russian dictionary is Russian', () => {
  // proper names and acronyms that read the same in both languages
  const SAME = new Set(['lang.ru', 'lang.en', 'ship.tls', 'ship.version', 'load.cpu', 'load.mem', 'load.gpu']);
  const words = (v: string): boolean => /\p{L}{3,}/u.test(v.replace(/\{\w+\}/g, ''));
  it('no Russian string is English (the buttons «Compact» and «New» stood in English in the Russian page)', () => {
    const bad = Object.keys(ru).filter((k) => !SAME.has(k)).filter((k) => (Array.isArray(ru[k]) ? ru[k] : [ru[k]]).some((v) => /[A-Za-z]{3,}/.test(v.replace(/\{\w+\}/g, '')) && !/[А-Яа-яЁё]/.test(v)));
    expect(bad).toEqual([]);
  });
  it('and none is the same text as the English one', () => {
    const same = Object.keys(ru).filter((k) => !SAME.has(k) && JSON.stringify(ru[k]) === JSON.stringify(en[k]) && words(String(ru[k])));
    expect(same).toEqual([]);
  });
  it('the English dictionary has no Russian letters', () => {
    const bad = Object.keys(en).filter((k) => k !== 'lang.ru').filter((k) => (Array.isArray(en[k]) ? en[k] : [en[k]]).some((v) => /[А-Яа-яЁё]/.test(v)));
    expect(bad).toEqual([]);
  });
});
