import { locale } from '../i18n';

export const esc = (s: unknown): string =>
  String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c] as string));

export const money = (n: number): string => Number(n || 0).toLocaleString(locale(), { maximumFractionDigits: 2 });

export const clamp = (v: number, a: number, b: number): number => Math.max(a, Math.min(b, v));

export function firstSentence(text: string, max = 150): string {
  const s = String(text ?? '').replace(/\s+/g, ' ').trim();
  if (!s) return '';
  const m = s.match(/^.{1,}?[.!?…](?=\s|$)/);
  const out = m ? m[0] : s;
  return out.length > max ? out.slice(0, max - 1).trimEnd() + '…' : out;
}

export function hash(str: string): number {
  let h = 2166136261 >>> 0;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h >>> 0;
}

/** Wraps the matching part of `text` in <mark> (text is escaped first). Case-insensitive. */
export function highlight(text: string, needle: string): string {
  const safe = esc(text);
  const n = needle.trim();
  if (!n) return safe;
  const parts = n.split(/\s+/).filter(Boolean).map((p) => esc(p).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  if (!parts.length) return safe;
  return safe.replace(new RegExp(`(${parts.join('|')})`, 'gi'), '<mark>$1</mark>');
}

/** Minutes/hours as «4 мин»: how long something has been going. */
export function duration(ms: number, t: (k: string, v?: Record<string, number>) => string): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 90) return t('dur.sec', { n: s });
  const m = Math.round(s / 60);
  if (m < 90) return t('dur.min', { n: m });
  return t('dur.hour', { n: Math.round(m / 60) });
}

/** `Host` of the server comes as `"GitHub"` or `{"GitLabSelf": "host"}`: its name in lower case. */
export function hostKind(k: unknown): string {
  if (typeof k === 'string') return k.toLowerCase();
  if (k && typeof k === 'object') return (Object.keys(k)[0] ?? '').toLowerCase();
  return '';
}
