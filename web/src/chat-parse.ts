/* What a turn of the Coordinator's chat is made of, read out of its plain text: the board events it ends its
   answer with («Added PLN-5 and started an agent on it»), a notice the board sent it («[Уведомление доски] …»),
   the files attached to a human message, and the task keys in any text. Pure functions: no DOM, no signals. */
import { t, tn } from './i18n';
import type { SavedFile } from './attach';

/** A task key as the board prints it: `PLN-9`, `APC-11`. */
export const KEY = /\b([A-Z][A-Z0-9]{1,6}-\d+)\b/g;
/** A task named by repository and number: `apicatalog/2`. */
const REPO_N = /\b([a-z][\w.-]*\/\d+)\b/g;

export type Verb =
  | 'added' | 'approved' | 'started' | 'updated' | 'removed' | 'sent' | 'stopped' | 'merged' | 'rejected'
  | 'restarted' | 'paused' | 'resumed' | 'moved' | 'archived';

export interface BoardEvent {
  verb: Verb;
  /** Task keys the sentence names (`PLN-5`); empty for «Updated worker.instructions». */
  keys: string[];
  /** The words after the keys: `and started an agent on it`, `the CI failure to fix`, `worker.instructions`. */
  rest: string;
  /** What stood in the trailing parentheses: `description`, `queued after PLN-25`. */
  paren: string;
  raw: string;
}

const VERBS: [RegExp, Verb][] = [
  [/^Added\b/, 'added'], [/^Approved\b/, 'approved'], [/^Started an agent on\b/, 'started'], [/^Updated\b/, 'updated'],
  [/^Removed\b/, 'removed'], [/^Sent\b/, 'sent'], [/^Stopped\b/, 'stopped'], [/^Merged\b/, 'merged'], [/^Rejected\b/, 'rejected'],
  [/^Restarted\b/, 'restarted'], [/^Paused\b/, 'paused'], [/^Resumed\b/, 'resumed'], [/^Moved\b/, 'moved'], [/^Archived\b/, 'archived'],
];
const CYRILLIC = /[Ѐ-ӿ]/;
/** A new sentence starts after `. ` and a capital letter, or on a new line. */
const SENTENCE = /\.\s+(?=[A-Z])|\n+/;

/** One sentence as a board event, or null when it is not one (a false event is worse than a missed one). */
export function parseEvent(sentence: string): BoardEvent | null {
  const raw = sentence.trim().replace(/\.$/, '');
  if (!raw || CYRILLIC.test(raw)) return null;
  const hit = VERBS.find(([re]) => re.test(raw));
  if (!hit) return null;
  const verb = hit[1];
  let body = raw.replace(hit[0], '').trim();
  let paren = '';
  const pm = /\s*\(([^()]*)\)$/.exec(body);
  if (pm) { paren = pm[1].trim(); body = body.slice(0, pm.index).trim(); }
  // the keys at the front: `PLN-1`, `PLN-1, PLN-2`, `PLN-1, PLN-2 and PLN-3`, `BOT-1, OMRC-1, …`
  const keys: string[] = [];
  let rest = body;
  const lead = /^((?:[A-Z][A-Z0-9]{1,6}-\d+)(?:\s*(?:,|and|,\s*and)\s*(?:[A-Z][A-Z0-9]{1,6}-\d+|…))*)/.exec(body);
  if (lead) {
    for (const m of lead[1].matchAll(KEY)) keys.push(m[1]);
    rest = body.slice(lead[1].length).trim().replace(/^,\s*/, '');
  }
  if (!keys.length && verb !== 'updated') return null;
  return { verb, keys, rest, paren, raw };
}

/** The whole paragraph as events, or null when any sentence in it is not one. */
export function parseEvents(paragraph: string): BoardEvent[] | null {
  const parts = paragraph.trim().split(SENTENCE).map((s) => s.trim()).filter(Boolean);
  if (!parts.length) return null;
  const out: BoardEvent[] = [];
  for (const p of parts) {
    const ev = parseEvent(p);
    if (!ev) return null;
    out.push(ev);
  }
  return out;
}

/** The answer without its last paragraph when that paragraph is nothing but board events. */
export function splitEvents(a: string): { body: string; events: BoardEvent[] } {
  const text = (a ?? '').replace(/\r\n?/g, '\n').trim();
  if (!text) return { body: '', events: [] };
  const cut = text.lastIndexOf('\n\n');
  const last = cut < 0 ? text : text.slice(cut + 2);
  const events = parseEvents(last);
  if (!events) return { body: text, events: [] };
  return { body: cut < 0 ? '' : text.slice(0, cut).trim(), events };
}

/** The parenthesised tail of an event in the page's language: the technical values stay as they are. */
function parenText(paren: string): string {
  if (!paren) return '';
  if (paren === 'description') return t('chat.ev.description');
  const q = /^queued after (.+)$/.exec(paren);
  if (q) return t('chat.ev.queuedAfter', { keys: q[1] });
  return paren;
}

/** The event as one line of text in the page's language; the task keys stay in it for the chips. */
export function eventText(ev: BoardEvent): string {
  const keys = ev.keys.join(', ');
  const n = ev.keys.length;
  const tail = ev.paren ? ` (${parenText(ev.paren)})` : '';
  const plain: Partial<Record<Verb, string>> = {
    approved: 'chat.ev.approved', started: 'chat.ev.started', removed: 'chat.ev.removed', stopped: 'chat.ev.stopped',
    merged: 'chat.ev.merged', rejected: 'chat.ev.rejected', restarted: 'chat.ev.restarted', paused: 'chat.ev.paused',
    resumed: 'chat.ev.resumed', moved: 'chat.ev.moved', archived: 'chat.ev.archived',
  };
  switch (ev.verb) {
    case 'added':
      if (!ev.rest) return tn('chat.ev.added', n, { keys }) + tail;
      if (/^and started an agent on (it|them)$/.test(ev.rest)) return tn('chat.ev.addedAgent', n, { keys }) + tail;
      break;
    case 'approved':
      if (!ev.rest) return tn('chat.ev.approved', n, { keys }) + tail;
      if (/^and pushed (it|them) to GitLab$/.test(ev.rest)) return tn('chat.ev.approvedGitlab', n, { keys }) + tail;
      break;
    case 'updated':
      if (!ev.keys.length) return t('chat.ev.updatedWhat', { what: ev.rest }) + tail;
      if (!ev.rest) return tn('chat.ev.updated', n, { keys }) + tail;
      break;
    case 'sent':
      if (ev.rest === 'the CI failure to fix') return tn('chat.ev.sentCi', n, { keys }) + tail;
      if (ev.rest) return tn('chat.ev.sent', n, { keys, what: ev.rest }) + tail;
      break;
    default:
      if (!ev.rest && plain[ev.verb]) return tn(plain[ev.verb] as string, n, { keys }) + tail;
  }
  return ev.raw;
}

export interface Notice {
  /** `repo/n` as the board names the task. */
  id: string;
  title: string;
  stateRu: string;
  stateEn: string;
  /** What else the notice said, without the instruction to the Coordinator. */
  note: string;
}

const NOTICE = /^\[Уведомление доски\]\s*(\S+\/\d+)\s*(?:«([^»]*)»)?\s*:?\s*([\s\S]*)$/;

/** A board notice taken apart; null for a human's words or an unknown shape. */
export function parseNotice(q: string): Notice | null {
  const m = NOTICE.exec((q ?? '').trim());
  if (!m) return null;
  const [, id, title = '', tail] = m;
  // the first sentence is the state: «ждёт тебя (needs you).»
  const first = /^([^.\n(]*)(?:\(([^)]*)\))?\.?/.exec(tail);
  const stateRu = (first?.[1] ?? '').trim();
  const stateEn = (first?.[2] ?? '').trim();
  const after = tail.slice(first?.[0].length ?? 0).trim();
  // the instruction to the Coordinator («Посмотри board details … и доложи итог») is not for the human
  const note = after.split(/(?<=\.)\s+|\n+/).filter((s) => s.trim() && !/^Посмотри\b|board details/.test(s.trim())).join(' ').trim();
  return { id, title, stateRu, stateEn, note };
}

/** The badge of a notice: by its English state, else nothing (the task's own status is then used). */
export function noticeStatus(n: Notice): string {
  const en = n.stateEn.toLowerCase();
  const ru = n.stateRu.toLowerCase();
  if (en.includes('needs you') || ru.includes('ждёт')) return 'waiting';
  if (en.includes('ready to approve') || ru.includes('одобрен')) return 'ready';
  if (en.includes('failed') || ru.includes('упала')) return 'failed';
  if (en.includes('plan') || ru.includes('план')) return 'plan';
  if (en.includes('review') || ru.includes('ревью')) return 'review';
  if (en.includes('done') || ru.includes('сделан')) return 'done';
  return '';
}

/** What a file saved by the planner is called: `<32 hex>.<ext>` (the same shape as `attachments::parse_id`). */
const SAVED = /^[0-9a-f]{32}\.(png|jpe?g|gif|webp|mp3|wav|ogg|flac|m4a|aac|txt|md|log|json|ya?ml|toml|csv)$/i;
const FILES_TAIL = /\n*(?:Картинки|Вложения) \(файлы на машине с доской\):\n([\s\S]*)$/;
const PASTED = /\[Image(?: #\d+| ?: ?source: ?[^\]]*)?\]\s*/g;

/** The human's words apart from the files the planner listed after them and the images pasted into the terminal. */
export function splitAttachments(q: string): { text: string; files: SavedFile[]; pasted: number } {
  let text = (q ?? '').replace(/\r\n?/g, '\n');
  const files: SavedFile[] = [];
  const m = FILES_TAIL.exec(text);
  if (m) {
    const lines = m[1].split('\n').map((s) => s.trim()).filter(Boolean);
    const names = lines.map((p) => p.slice(p.lastIndexOf('/') + 1));
    if (lines.length && names.every((n) => SAVED.test(n))) {
      for (const n of names) files.push({ id: n, name: n });
      text = text.slice(0, m.index);
    }
  }
  let pasted = 0;
  text = text.replace(PASTED, () => { pasted++; return ''; });
  return { text: text.trim(), files, pasted };
}

export type RefPart = string | { ref: string };

/** The text cut at task keys; `repo/n` becomes a reference only when `known` says such a task exists. */
export function splitRefs(text: string, known: (id: string) => boolean = () => false): RefPart[] {
  const out: RefPart[] = [];
  const s = text ?? '';
  const hits: { at: number; len: number; ref: string }[] = [];
  for (const m of s.matchAll(KEY)) hits.push({ at: m.index ?? 0, len: m[1].length, ref: m[1] });
  for (const m of s.matchAll(REPO_N)) if (known(m[1])) hits.push({ at: m.index ?? 0, len: m[1].length, ref: m[1] });
  hits.sort((a, b) => a.at - b.at);
  let last = 0;
  for (const h of hits) {
    if (h.at < last) continue;
    if (h.at > last) out.push(s.slice(last, h.at));
    out.push({ ref: h.ref });
    last = h.at + h.len;
  }
  if (last < s.length) out.push(s.slice(last));
  return out;
}
