/* The two symbols of a message to an agent: `/` — the commands the program itself knows, `@` — the
   things of the board and of the repositories worth naming. Everything here is data: the page draws
   the list, the component that owns the field reads the keys. Nothing here touches the network. */
import { swapLayout } from './search';
import type { Snapshot } from './types';

export type CmdKind = 'builtin' | 'skill' | 'command';

/** One command of a program, as the server reads it from the program's own files. */
export interface SlashCmd {
  /** The word after the slash, without it (`compact`). */
  name: string;
  /** A key of the dictionary for a built-in, or the words of the skill's or the command's own file. */
  desc: string;
  kind: CmdKind;
}

export type Group = 'command' | 'task' | 'universe' | 'galaxy' | 'runner' | 'file' | 'image';

/** The order of the groups in the list. */
export const GROUPS: Group[] = ['command', 'task', 'universe', 'galaxy', 'runner', 'file', 'image'];

export interface Item {
  group: Group;
  /** What the person sees first. */
  label: string;
  /** The words beside it: a title, a path, a description. */
  hint: string;
  /** What goes into the message. */
  insert: string;
  icon: string;
}

/** A file or a folder of a repository, as `GET /api/hints/files` finds it. */
export interface FileHit {
  name: string;
  path: string;
  rel: string;
  repo: string;
  dir: boolean;
}

/** What the list of `@` is built from: the page's own state and the files the server found. */
export interface MentionData {
  tasks: { tag: string; title: string }[];
  universes: { name: string; root: string; key: string }[];
  host: string;
  runners: { id: string; target: string; ok: boolean }[];
  images: { name: string; path: string }[];
  files: FileHit[];
}

/** Where the symbol being typed is: `start` is the symbol itself, `end` the caret, `query` the words after it. */
export interface Trigger {
  char: '/' | '@';
  start: number;
  end: number;
  query: string;
}

const BEFORE = /[\s([{«"']/;

/**
 * The symbol being typed at the caret, if any. `/` counts only at the very start of the message (there it
 * reaches the program as its own command); `@` counts at the start of a word, so an e-mail is left alone.
 */
export function detect(text: string, caret: number): Trigger | null {
  const at = Math.max(0, Math.min(caret, text.length));
  if (text.startsWith('/')) {
    const typed = text.slice(0, at);
    return /^\/[^\s]*$/.test(typed) ? { char: '/', start: 0, end: at, query: typed.slice(1) } : null;
  }
  let i = at - 1;
  while (i >= 0 && !BEFORE.test(text[i])) i--;
  const start = i + 1;
  if (text[start] !== '@') return null;
  return { char: '@', start, end: at, query: text.slice(start + 1, at) };
}

/** The commands that answer the typed words: the name first, then the description. */
export function slashItems(cmds: SlashCmd[], query: string): Item[] {
  const q = query.trim().toLowerCase();
  if (!cmds.length) return [];
  const swapped = swapLayout(q);
  const needles = swapped !== q && swapped ? [q, swapped] : [q];
  const scored: { cmd: SlashCmd; score: number }[] = [];
  for (const cmd of cmds) {
    const name = cmd.name.toLowerCase();
    // A built-in's words are a key of the dictionary, not text: searching them would find every command.
    const desc = cmd.desc.startsWith('cmd.') ? '' : cmd.desc.toLowerCase();
    let best = Infinity;
    for (const needle of needles) {
      if (!needle) { best = 0; break; }
      const s = name.startsWith(needle) ? 0 : name.includes(needle) ? 1 : desc.includes(needle) ? 2 : Infinity;
      if (s < best) best = s;
    }
    if (best !== Infinity) scored.push({ cmd, score: best });
  }
  // `sort` is stable: commands with the same match keep the order the program's files came in.
  scored.sort((a, b) => a.score - b.score);
  return scored.slice(0, 60).map(({ cmd }) => ({
    group: 'command',
    label: `/${cmd.name}`,
    hint: cmd.desc,
    insert: `/${cmd.name}`,
    icon: cmd.kind === 'builtin' ? 'send' : cmd.kind === 'skill' ? 'memory' : 'file',
  }));
}

const CAPS: Record<Group, number> = { command: 60, task: 6, universe: 4, galaxy: 1, runner: 2, file: 8, image: 4 };

interface Candidate extends Item {
  hay: string;
}

function match(hay: string, query: string): boolean {
  if (!query) return true;
  const q = query.toLowerCase();
  const h = hay.toLowerCase();
  return h.includes(q) || h.includes(swapLayout(q));
}

/** The `@` list: tasks, universes, this machine's galaxy, shipyards, files and folders, pictures of the chat. */
export function mentionItems(query: string, data: MentionData): Item[] {
  const q = query.trim();
  const out: Item[] = [];
  const per = new Map<Group, number>();
  const add = (c: Candidate): void => {
    const n = per.get(c.group) ?? 0;
    if (n >= CAPS[c.group]) return;
    if (!match(c.hay, q)) return;
    per.set(c.group, n + 1);
    const { hay: _hay, ...item } = c;
    out.push(item);
  };
  for (const t of data.tasks) {
    add({ group: 'task', label: t.tag, hint: t.title, insert: t.tag, icon: 'ship', hay: `${t.tag} ${t.title}` });
  }
  for (const u of data.universes) {
    add({
      group: 'universe', label: u.name, hint: u.root, insert: u.root || u.name, icon: 'universe',
      hay: `${u.name} ${u.key} ${u.root}`,
    });
  }
  add({ group: 'galaxy', label: data.host, hint: '', insert: data.host, icon: 'galaxy', hay: data.host });
  for (const r of data.runners) {
    add({ group: 'runner', label: r.id, hint: r.target, insert: r.id, icon: 'yard', hay: `${r.id} ${r.target}` });
  }
  for (const f of data.files) {
    add({
      group: 'file', label: f.name, hint: f.dir ? `${f.rel}/ · ${f.repo}` : `${f.rel} · ${f.repo}`,
      insert: f.path, icon: f.dir ? 'folder' : 'file', hay: `${f.rel} ${f.repo}`,
    });
  }
  for (const im of data.images) {
    add({ group: 'image', label: im.name, hint: im.path, insert: im.path, icon: 'attach', hay: im.name });
  }
  // Groups keep the order of GROUPS; within a group the order the data came in (files come sorted by the server).
  return out.sort((a, b) => GROUPS.indexOf(a.group) - GROUPS.indexOf(b.group));
}

/** The groups that have items, in order. */
export function groupsOf(items: Item[]): Group[] {
  return GROUPS.filter((g) => items.some((i) => i.group === g));
}

/** The message with the item at the symbol's place: a command keeps a space after it, ready for its words. */
export function applyItem(text: string, trigger: Trigger, item: Item): { text: string; caret: number } {
  const before = text.slice(0, trigger.start);
  const after = text.slice(trigger.end);
  if (trigger.char === '/') {
    const gap = after.startsWith(' ') ? '' : ' ';
    const inserted = item.insert + gap;
    return { text: before + inserted + after, caret: before.length + inserted.length };
  }
  // A word that follows right after the path would run into it: a space keeps them apart.
  const gap = after === '' || /^\s/.test(after) ? '' : ' ';
  const inserted = item.insert + gap;
  return { text: before + inserted + after, caret: before.length + inserted.length };
}

/* ───────── the `@` data of the page ───────── */

/** The `@` data of the page: what it already holds, and the pictures that went with the chat. */
export function mentionData(s: Snapshot | null, found: FileHit[]): MentionData {
  const dir = s?.server.attachments_dir ?? '';
  const images = new Map<string, { name: string; path: string }>();
  for (const m of s?.coordinator.history ?? []) {
    for (const a of m.attachments ?? []) {
      if (!a.mime?.startsWith('image/')) continue;
      if (!images.has(a.id)) images.set(a.id, { name: a.name, path: dir ? `${dir}/${a.id}` : a.id });
    }
  }
  return {
    tasks: (s?.tasks ?? []).map((t) => ({ tag: t.tag, title: t.title })),
    universes: (s?.projects ?? []).map((p) => ({ name: p.name, root: p.root, key: p.key })),
    host: s?.host ?? '',
    runners: (s?.runners ?? []).map((r) => ({ id: r.id, target: r.target, ok: r.ok })),
    images: [...images.values()],
    files: found,
  };
}
