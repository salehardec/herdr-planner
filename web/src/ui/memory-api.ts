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
  /** A universe's draft (outside the checkout, not in git yet): `new`, `changed`, `deleted`; `sent` / `sentdel`: a board task
   *  has it and has not merged it yet (`task` says which); empty for a committed entry. */
  draft?: string;
  task?: string;
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
  repo: string;
  level: 'galaxy' | 'universe';
  dir: string;
  exists: boolean;
  writable: boolean;
  entries: MemEntry[];
  foreign?: { program: string; label: string; dir: string; entries: MemEntry[] }[];
  connect?: MemConnect;
  settings?: { suggest: boolean };
  git?: { checkout: boolean; dirty: { state: string; file: string }[]; drafts: number; sent: { task: string; id: string | null; title: string; files: number }[] };
  suggestions?: MemSuggestion[];
  note?: string;
}

export interface MemState {
  data: MemData | null;
  error: string;
  loading: boolean;
  at: number;
}

/** The panels' data, by repo (`''`: the Galaxy's own memory). */
export const memState = signal<Record<string, MemState>>({});

export const memoryOf = (repo: string): MemState =>
  memState.value[repo] ?? { data: null, error: '', loading: false, at: 0 };

function put(repo: string, patch: Partial<MemState>): void {
  memState.value = { ...memState.value, [repo]: { ...memoryOf(repo), ...patch } };
}

export async function loadMemory(repo: string): Promise<void> {
  put(repo, { loading: true });
  try {
    const data = await api<MemData>(`/api/memory${repo ? `?repo=${encodeURIComponent(repo)}` : ''}`);
    put(repo, { data, error: '', loading: false, at: Date.now() });
  } catch (e) {
    put(repo, { error: (e as Error).message, loading: false, at: Date.now() });
  }
}

export interface SaveBody {
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
  repo: string;
  /** The entries go to the universe's drafts (outside the checkout), not to files. */
  draft: boolean;
  /** This is the sending of the drafts to the repository by a board task, not a change of an entry. */
  send?: boolean;
  /** This throws a draft away. */
  discard?: boolean;
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
  void loadMemory(b.repo);
  return r.entry;
};

export const previewDelete = (repo: string, id: string): Promise<MemPreview> =>
  post<MemPreview>('/api/memory/delete', { repo, id });

export const deleteEntry = async (repo: string, id: string, token: string, a: Accept = {}): Promise<void> => {
  await post('/api/memory/delete', { repo, id, ...gate(token, a) });
  void loadMemory(repo);
};

export const previewDiscard = (repo: string, id: string): Promise<MemPreview> => post<MemPreview>('/api/memory/discard', { repo, id });

/** Drops a universe's draft of an entry (what git has stays), after its card was confirmed. */
export const discardDraft = async (repo: string, id: string, token: string): Promise<void> => {
  await post('/api/memory/discard', { repo, id, confirm: true, token });
  void loadMemory(repo);
};

export const previewSend = (repo: string): Promise<MemPreview> => post<MemPreview>('/api/memory/send', { repo });

export const forgetSent = async (repo: string, task: string): Promise<void> => {
  await post('/api/memory/sent/forget', { repo, task });
  void loadMemory(repo);
};

/** The board task that was made for a send: its tag (`PLN-35`), id (`herdr-planner/35`) and title. */
export interface SentTask { task: string; id: string | null; title: string; files: number }

/** Hands the drafts to a board task (it has its own working copy; the checkout is not touched). */
export const sendMemory = async (repo: string, token: string, a: Accept = {}): Promise<SentTask> => {
  const r = await post<SentTask>('/api/memory/send', { repo, ...gate(token, a) });
  void loadMemory(repo);
  return r;
};

/** Switches the memory block in the board's `coordinator.instructions` on or off. Without `expect` it only previews
 *  (`current` → `now`, `now: null` means the setting would be unset); with `expect` (the text the card showed) it
 *  makes the change, unless the instructions are not those any more (409). */
export const connectMemory = (on: boolean, expect?: string): Promise<{ current: string; now: string | null }> =>
  post<{ current: string; now: string | null }>('/api/memory/connect', expect === undefined ? { on } : { on, expect, confirm: true });

export const setSuggest = async (suggest: boolean): Promise<void> => {
  await post('/api/memory/settings', { suggest });
  void loadMemory('');
};

type Line = { kind: string; text: string };

export const previewSuggestion = (task: string, lines: Line[]): Promise<MemPreview> =>
  post<MemPreview>('/api/memory/suggestion', { task, action: 'accept', lines });

export const keepSuggestion = (task: string, lines: Line[], token: string, a: Accept = {}): Promise<{ saved: MemEntry[] }> =>
  post<{ saved: MemEntry[] }>('/api/memory/suggestion', { task, action: 'accept', lines, ...gate(token, a) });

export const dismissSuggestion = (task: string): Promise<unknown> => post('/api/memory/suggestion', { task, action: 'dismiss' });

/** The memory window over the page: the list of one level (`repo` empty: the Galaxy; `program`: the memory another program
 *  keeps, read only), with the search the human typed in the panel and the entry to open. */
export interface MemoryWin { repo: string; program?: string; q?: string; open?: string }
export const memoryWin = signal<MemoryWin | null>(null);
export function openMemory(w: MemoryWin): void {
  memoryWin.value = w;
}

/** Where an entry stands: written (in the files / in git), a draft the planner keeps, or sent and waiting for a task's merge. */
export type EntryState = 'saved' | 'draft' | 'waiting';
export const stateOf = (e: MemEntry): EntryState => (!e.draft ? 'saved' : e.draft.startsWith('sent') ? 'waiting' : 'draft');

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
