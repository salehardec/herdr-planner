/* What the server sends (GET /api/state, SSE) and the shapes the page derives from it. */

export type Status = 'waiting' | 'failed' | 'ready' | 'plan' | 'review' | 'working' | 'queued' | 'done';

export const STATUS_ORDER: Status[] = ['waiting', 'failed', 'ready', 'plan', 'review', 'working', 'queued', 'done'];
export const ATTENTION: Status[] = ['waiting', 'failed', 'ready', 'plan'];

export interface ReviewBrief {
  pass: boolean | null;
  meets: boolean | null;
  tally: string;
  text: string;
  checks: string[];
  by: string;
}

export interface Subagent {
  id: string;
  kind: string;
  role: string;
  title: string;
  started_at_ms: number;
  last_at_ms: number;
  state: 'running' | 'quiet' | 'done';
  doing: string;
  result: string;
  note: string;
}

export interface Task {
  id: string;
  repo: string;
  n: number;
  tag: string;
  title: string;
  state: string;
  kind: string;
  stage: string;
  msg: string;
  app: string;
  progress: number | null;
  eta: number | null;
  pr: string;
  review: ReviewBrief | null;
  round: number;
  queue: number | null;
  status: Status;
  label: string;
  board_label: string;
  review_failed: boolean;
  merge_blocked: boolean;
  archived: boolean;
  actions: string[];
  created: number;
  description: string;
  worker_agent: string;
  model: string;
  session: string;
  worktree: string;
  exited_at: number;
  key: string;
  subagents: Subagent[];
  subagents_note: string;
}

export interface Link {
  from: string;
  to: string;
  kind: 'waiting' | 'done' | 'far';
  label?: string;
}

export interface LagClone {
  galaxy: string;
  behind: number;
  ahead: number;
  state: 'same' | 'behind' | 'ahead' | 'diverged' | 'unknown';
}

export interface Lag {
  origin: string;
  main: string;
  behind: number;
  ahead: number;
  known: boolean;
  as_of_ms: number;
  note: string;
  vs: LagClone[];
}

export interface Project {
  name: string;
  key: string;
  active: number;
  done: number;
  total: number;
  statuses: Record<string, number>;
  quiet: boolean;
  root: string;
  folder: string;
  origin: string;
  lag: Lag | null;
  empty: boolean;
}

export interface Summary {
  projects: number;
  active_projects: number;
  quiet_projects: number;
  tasks: number;
  done: number;
  archived: number;
  attention: number;
  waiting: number;
  failed: number;
  ready: number;
  plan: number;
  review: number;
  working: number;
  queued: number;
}

export interface LimitWindow {
  label: string;
  percent: number;
  severity: 'ok' | 'warn' | 'crit';
  resets_at_ms: number | null;
}

export interface AgentUsage {
  id: string;
  name: string;
  tier?: string;
  stale?: boolean;
  status?: string;
  note?: string;
  limits?: LimitWindow[];
  balance?: { remaining: number; funded?: number; currency?: string; estimated?: boolean } | null;
  updated_at_ms?: number;
}

export interface Usage {
  agents: AgentUsage[];
  stale_after_secs?: number;
}

export interface ChatAttachment {
  id: string;
  name: string;
  mime: string;
  size: number;
}

export interface ChatMessage {
  text: string;
  at: number;
  attachments: ChatAttachment[];
}

export interface CoordinatorState {
  status: string;
  status_line: string;
  reply: string;
  busy: boolean;
  dialog: boolean;
  error: string;
  last_sent: string;
  sent_at: number;
  source: string;
  history: ChatMessage[];
  subagents: Subagent[];
  subagents_note: string;
  /** Tokens of the last turn against the window of the model (0 when unknown). */
  context: { used: number; window: number; percent: number; known: boolean; model: string };
  can_compact: boolean;
  program: string;
}

export interface Why {
  line: string;
  detail: string[];
}

export interface RemoteProject {
  name: string;
  key: string;
  folder: string;
  origin: string | null;
}

export interface Galaxy {
  label: string;
  target: string;
  ok: boolean;
  connecting: boolean;
  why: Why | null;
  version: string;
  host: string;
  coordinator: { status: string; reply: string; talk: { you: boolean; text: string }[] };
  projects: RemoteProject[];
  tasks: Task[];
  links: Link[];
  fetched_at_ms: number;
  lag: Record<string, Lag>;
}

export interface Bridge {
  origin: string;
  a: { galaxy: string; repo: string };
  b: { galaxy: string; repo: string };
  lag: string;
}

export interface TodoItem {
  id: string;
  text: string;
  at: number;
  to: string;
}

export interface JournalEntry {
  n: number;
  t: number;
  id: string;
  s: string;
  text: string;
}

export interface Theme {
  name: string;
  mode: 'dark' | 'light';
  colors: Record<string, string>;
}

export interface Snapshot {
  version: string;
  at: number;
  seq: number;
  board: { ok: boolean; error: string; stale_at: number };
  host: string;
  tasks: Task[];
  archived: Task[];
  projects: Project[];
  summary: Summary;
  links: Link[];
  limits: Usage;
  todo: TodoItem[];
  journal: JournalEntry[];
  coordinator: CoordinatorState;
  galaxies: Galaxy[];
  bridges: Bridge[];
  theme: Theme;
  herdr: { running: boolean; socket: string };
  auth: { required: boolean; user: string };
  server: { listen: string; port: number; local_only: boolean; tls: boolean; projects_dir: string };
}

/** One galaxy as the world draws it: the machine's board, home or distant. */
export interface GalaxyView {
  /** `''` for this machine, otherwise the machine's label. */
  id: string;
  label: string;
  home: boolean;
  ok: boolean;
  tasks: Task[];
  projects: Project[];
  links: Link[];
}

export type Level = 'galaxy' | 'nebula' | 'far';

export type EntityKind = 'quasar' | 'galaxy' | 'universe' | 'planet' | 'moon';

/** What is selected. `galaxy` is the machine label; empty for this machine. */
export interface Entity {
  kind: EntityKind;
  galaxy: string;
  /** universe: repo name · planet: task id · moon: `<task id>#<subagent id>` · galaxy: label */
  id: string;
}

export interface TurnData {
  n: number;
  at: number;
  from: 'human' | 'board';
  q: string;
  a: string;
}

export interface Request {
  key: string;
  type: string;
  title: string;
  id: string;
  tag: string;
  repo: string;
  msg: string;
  text: string;
  task?: Task;
  options?: { label: string; value: string }[];
  urgency: number;
  galaxy: string;
}

export interface SearchHit {
  kind: 'galaxy' | 'quasar' | 'universe' | 'planet' | 'moon' | 'todo' | 'chat';
  id: string;
  galaxy: string;
  path: string;
  title: string;
  snippet: string;
  status: string;
  rank: number;
}

export const entityKey = (e: Entity): string => `${e.kind}|${e.galaxy}|${e.id}`;
export const sameEntity = (a: Entity | null, b: Entity | null): boolean =>
  !!a && !!b && a.kind === b.kind && a.galaxy === b.galaxy && a.id === b.id;
