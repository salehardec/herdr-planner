// @ts-check
// Small helpers shared by the page's modules; nothing here touches the DOM
// or the page's state.

/** @param {string} s */
export function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] || c);
}

/** @param {string} tag @param {Record<string,string>} attrs @param {string} inner */
export function h(tag, attrs, inner) {
  const a = Object.entries(attrs).map(([k, v]) => ` ${k}="${esc(v)}"`).join('');
  return `<${tag}${a}>${inner}</${tag}>`;
}

/** @param {number} n @param {string} one @param {string} few @param {string} many */
export function plural(n, one, few, many) {
  const m10 = n % 10, m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/** A short «how long ago» for a unix-millisecond time: «сейчас», «5 мин», «2 ч», «3 дн». @param {number} ms @param {number} [now] */
export function ago(ms, now = Date.now()) {
  if (!ms) return '';
  const min = Math.floor(Math.max(0, now - ms) / 60000);
  if (min < 1) return 'сейчас';
  if (min < 60) return `${min} мин`;
  if (min < 60 * 24) return `${Math.floor(min / 60)} ч`;
  return `${Math.floor(min / 1440)} дн`;
}

/** @param {string} s @param {number} n */
export const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);

/** @param {string} s */
export const oneLine = (s) => s.replace(/\s+/g, ' ').trim();

/** Every word of `needle` occurs in `hay`, in any case. @param {string} hay @param {string} needle */
export function textMatches(hay, needle) {
  const h = hay.toLowerCase();
  return needle.split(/\s+/).filter(Boolean).every((w) => h.includes(w.toLowerCase()));
}

/**
 * The key a shortcut means: with a Russian (or any non-Latin) layout the letter
 * keys are told apart by their physical position, so `ч` on the X key is `x`.
 * @param {KeyboardEvent} ev
 */
export function keyOf(ev) {
  const k = ev.key;
  if (k.length > 1 || k.charCodeAt(0) < 128) return k;
  const letter = /^Key([A-Z])$/.exec(ev.code);
  if (letter) return letter[1].toLowerCase();
  if (ev.code === 'Slash') return ev.shiftKey ? '?' : '/';
  if (ev.code === 'Comma') return ',';
  return k;
}

/** @param {string} url @param {any} body */
export async function post(url, body) {
  const r = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
  return data;
}

/** @param {string} url */
export async function getJson(url) {
  const r = await fetch(url);
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
  return data;
}
