/* Two languages: Russian (default) and English. Every string the page shows
   goes through t(); dates and numbers follow the language. */
import { signal } from '@preact/signals';
import { ru } from './ru';
import { en } from './en';

export type Lang = 'ru' | 'en';
type Entry = string | string[];
const DICT: Record<Lang, Record<string, Entry>> = { ru, en };
const KEY = 'herdr-planner.lang';

function initial(): Lang {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'ru' || v === 'en') return v;
  } catch { /* private mode */ }
  return 'ru';
}

export const lang = signal<Lang>(initial());

export function setLang(l: Lang): void {
  lang.value = l;
  try { localStorage.setItem(KEY, l); } catch { /* private mode */ }
  document.documentElement.lang = l;
}

function fill(text: string, vars?: Record<string, string | number>): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
}

/** The string for `key` in the current language (reads the signal, so views re-render). */
export function t(key: string, vars?: Record<string, string | number>): string {
  const l = lang.value;
  const e = DICT[l][key] ?? DICT.ru[key];
  if (e === undefined) return key;
  return fill(Array.isArray(e) ? e[e.length - 1] : e, vars);
}

/** Plural form chosen by `n`; `key` holds [one, few, many] (ru) or [one, other] (en). */
export function tn(key: string, n: number, vars?: Record<string, string | number>): string {
  const l = lang.value;
  const e = DICT[l][key] ?? DICT.ru[key];
  if (e === undefined) return key;
  const forms = Array.isArray(e) ? e : [e];
  let i: number;
  if (l === 'ru') {
    const m10 = n % 10;
    const m100 = n % 100;
    i = m10 === 1 && m100 !== 11 ? 0 : m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14) ? 1 : 2;
  } else {
    i = n === 1 ? 0 : 1;
  }
  return fill(forms[Math.min(i, forms.length - 1)], { n, ...vars });
}

export const locale = (): string => (lang.value === 'ru' ? 'ru-RU' : 'en-GB');

export function keysOf(l: Lang): string[] {
  return Object.keys(DICT[l]);
}
