/* The Galaxy's memory (Markdown files, three levels): the state of its panels, the «В память» dialog and the calls they
   make. The calls mirror src/memory_api.rs. */
import { signal } from '@preact/signals';
import { api, post } from '../api';
import { t } from '../i18n';

export const KINDS = ['rule', 'preference', 'decision', 'convention', 'pitfall', 'reference'] as const;
export type MemKind = (typeof KINDS)[number];

export interface MemEntry {
  id: string;
  title: string;
  description: string;
  kind: string;
  body: string;
  added: string;
  source: string;
  at: number;
  /** A universe's draft (outside the checkout, not in git yet): `new`, `changed`, `deleted`; empty for a committed entry. */
  draft?: string;
}

/** The memory block in the board's `coordinator.instructions`. */
export interface MemConnect {
  setting: string;
  connected: boolean;
  /** A block from an older version or another folder is there. */
  stale: boolean;
  current: string;
  block: string;
  error: string;
}

export interface MemSuggestion {
  task: string;
  repo: string;
  tag: string;
  title: string;
  lines: { kind: string; text: string }[];
  at: number;
}

export interface MemData {
  ok: true;
  galaxy: string;
  repo: string;
  level: 'galaxy' | 'universe';
  dir: string;
  exists: boolean;
  writable: boolean;
  entries: MemEntry[];
  foreign?: { program: string; label: string; dir: string; entries: MemEntry[] }[];
  connect?: MemConnect;
  settings?: { suggest: boolean };
  git?: { checkout: boolean; dirty: { state: string; file: string }[]; drafts: number };
  suggestions?: MemSuggestion[];
  note?: string;
}

export interface MemState {
  data: MemData | null;
  error: string;
  loading: boolean;
  at: number;
}

const key = (galaxy: string, repo: string): string => `${galaxy}|${repo}`;

/** The panels' data, by `galaxy|repo` (`repo` empty: the Galaxy's own memory). */
export const memState = signal<Record<string, MemState>>({});

export const memoryOf = (galaxy: string, repo: string): MemState =>
  memState.value[key(galaxy, repo)] ?? { data: null, error: '', loading: false, at: 0 };

function put(galaxy: string, repo: string, patch: Partial<MemState>): void {
  const k = key(galaxy, repo);
  memState.value = { ...memState.value, [k]: { ...memoryOf(galaxy, repo), ...patch } };
}

export async function loadMemory(galaxy: string, repo: string): Promise<void> {
  put(galaxy, repo, { loading: true });
  try {
    const data = await api<MemData>(`/api/memory?galaxy=${encodeURIComponent(galaxy)}${repo ? `&repo=${encodeURIComponent(repo)}` : ''}`);
    put(galaxy, repo, { data, error: '', loading: false, at: Date.now() });
  } catch (e) {
    put(galaxy, repo, { error: (e as Error).message, loading: false, at: Date.now() });
  }
}

export interface SaveBody {
  galaxy: string;
  repo: string;
  id?: string;
  title: string;
  description?: string;
  kind: string;
  body: string;
  source?: string;
}

/** One file's change, as the human is shown it before anything is written. */
export interface MemChange {
  op: 'add' | 'change' | 'delete';
  id: string;
  /** A path for the Galaxy, `docs/memory/<id>.md` for a universe. */
  file: string;
  before: string | null;
  after: string | null;
}

/** What the check for personal data and secrets found (a secret only by its first letters). */
export interface MemFinding {
  file: string;
  kind: 'ip' | 'email' | 'host' | 'secret' | 'path' | 'person' | 'own';
  text: string;
  field: string;
  line: number;
}

/** The answer to a change without `confirm`: «было → стало», where it goes, what was found. `risk`: `none`; `confirm` (a
 *  «да» about the findings is needed); `block` (a public repository: forbidden unless said explicitly). */
export interface MemPreview {
  token: string;
  level: 'galaxy' | 'universe';
  galaxy: string;
  repo: string;
  /** The entries go to the universe's drafts (outside the checkout), not to files. */
  draft: boolean;
  /** This is a commit of the drafts, not a change of an entry. */
  commit?: boolean;
  changes: MemChange[];
  findings: MemFinding[];
  risk: 'none' | 'confirm' | 'block';
  visibility: 'public' | 'private' | 'local' | 'unknown' | null;
  /** The same entries without what was found: `{title, description, body}` per change, `null` where nothing was found. */
  redacted: ({ title: string; description: string; body: string } | null)[];
}

/** What the human says when they confirm a change with findings. */
export interface Accept {
  accept_risk?: boolean;
  accept_public?: boolean;
}

/** Without a token a call only previews; with it (and `confirm`) it writes. */
const gate = (token: string, a: Accept): Record<string, unknown> => ({ confirm: true, token, ...a });

export const previewSave = (b: SaveBody): Promise<MemPreview> => post<MemPreview>('/api/memory/save', b);

export const saveEntry = async (b: SaveBody, token: string, a: Accept = {}): Promise<MemEntry> => {
  const r = await post<{ entry: MemEntry }>('/api/memory/save', { ...b, ...gate(token, a) });
  void loadMemory(b.galaxy, b.repo);
  return r.entry;
};

export const previewDelete = (galaxy: string, repo: string, id: string): Promise<MemPreview> =>
  post<MemPreview>('/api/memory/delete', { galaxy, repo, id });

export const deleteEntry = async (galaxy: string, repo: string, id: string, token: string, a: Accept = {}): Promise<void> => {
  await post('/api/memory/delete', { galaxy, repo, id, ...gate(token, a) });
  void loadMemory(galaxy, repo);
};

/** Drops a universe's draft of an entry (what git has stays). */
export const discardDraft = async (repo: string, id: string): Promise<void> => {
  await post('/api/memory/discard', { repo, id });
  void loadMemory('', repo);
};

export const previewCommit = (repo: string): Promise<MemPreview> => post<MemPreview>('/api/memory/commit', { repo });

export const commitMemory = async (repo: string, token: string, a: Accept = {}): Promise<{ commit: string; message: string; files: number }> => {
  const r = await post<{ commit: string; message: string; files: number }>('/api/memory/commit', { repo, ...gate(token, a) });
  void loadMemory('', repo);
  return r;
};

/** Switches the memory block in the board's `coordinator.instructions` on or off. Without `expect` it only previews
 *  (`current` → `now`, `now: null` means the setting would be unset); with `expect` (the text the card showed) it
 *  makes the change, unless the instructions are not those any more (409). */
export const connectMemory = (galaxy: string, on: boolean, expect?: string): Promise<{ current: string; now: string | null }> =>
  post<{ current: string; now: string | null }>('/api/memory/connect', expect === undefined ? { galaxy, on } : { galaxy, on, expect, confirm: true });

export const setSuggest = async (suggest: boolean): Promise<void> => {
  await post('/api/memory/settings', { suggest });
  void loadMemory('', '');
};

type Line = { kind: string; text: string };

export const previewSuggestion = (task: string, lines: Line[]): Promise<MemPreview> =>
  post<MemPreview>('/api/memory/suggestion', { task, action: 'accept', lines });

export const keepSuggestion = (task: string, lines: Line[], token: string, a: Accept = {}): Promise<{ saved: MemEntry[] }> =>
  post<{ saved: MemEntry[] }>('/api/memory/suggestion', { task, action: 'accept', lines, ...gate(token, a) });

export const dismissSuggestion = (task: string): Promise<unknown> => post('/api/memory/suggestion', { task, action: 'dismiss' });

/** An entry to show: the search found it. The panel opens it and scrolls to it. */
export const memFocus = signal<{ galaxy: string; repo: string; id: string } | null>(null);

/** The «В память» dialog: what to remember (the human edits it) and where it came from. */
export interface RememberInit {
  text: string;
  /** `chat`, `task PLN-9`, … */
  source: string;
  title?: string;
  kind?: string;
  /** The universe it is probably about (a task's repo); the dialog starts on that level. */
  repo?: string;
}
export const rememberDlg = signal<RememberInit | null>(null);
export function openRemember(init: RememberInit): void {
  rememberDlg.value = init;
}

/** The kind in words. */
export const kindLabel = (k: string): string => (k && (KINDS as readonly string[]).includes(k) ? t(`mem.kind.${k}`) : t('mem.kind.note'));
