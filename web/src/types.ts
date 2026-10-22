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

export interface Lag {
  origin: string;
  main: string;
  behind: number;
  ahead: number;
  known: boolean;
  as_of_ms: number;
  note: string;
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

/** How loaded this machine is (GET /api/load, the `load` event). Shares are 0..100, memory is in bytes. */
export interface LoadGpu { name: string; util: number; mem_used: number; mem_total: number }
export interface LoadInfo {
  at: number;
  ok: boolean;
  error: string;
  every_ms: number;
  /** The server's clock when it sent the reading: how old `at` is is judged by it, not by the browser's clock. */
  sent: number;
  /** This system cannot be read (no /proc): no bars at all. */
  unsupported?: boolean;
  cpu: { percent: number; cores: number };
  mem: { used: number; total: number; percent: number };
  gpus: LoadGpu[];
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

export interface TodoItem {
  id: string;
  text: string;
  at: number;
  to: string;
  /** Files kept with the item (they travel with it when it becomes a task or a message). */
  attachments: ChatAttachment[];
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

export interface RunnerTaskRef {
  id: string;
  tag: string;
  status: string;
  title: string;
}

export interface RunnerHost {
  hostname: string;
  os: string;
  arch: string;
  cpus: number;
  load1: number;
  load5: number;
  load15: number;
  mem_total: number;
  mem_avail: number;
  swap_total: number;
  swap_free: number;
  disk_root: string;
  disk_size: number;
  disk_avail: number;
  docker_version: string;
  docker_arch: string;
  compose_version: string;
  runtimes: string[];
  docker_cpus: number;
  docker_mem: number;
  docker_containers: number;
  docker_running: number;
  docker_images: number;
  gpu: { index: string; name: string; util_pct: number | null; mem_used_mib: number | null; mem_total_mib: number | null; temp_c: number | null }[];
}

export interface RunnerContainer {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  created_ms: number;
  labels: Record<string, string>;
  ports: string;
  networks: string;
  mounts: string;
  size: string;
  size_bytes: number;
  cpu_pct: number | null;
  mem_bytes: number | null;
  mem_limit: number | null;
  pids: number | null;
  net_io: string;
  block_io: string;
  gpu_mem_mib: number | null;
  task: RunnerTaskRef | null;
  class: 'active' | 'forgotten' | 'unsure' | 'foreign';
}

export interface RunnerBlock {
  connected: boolean;
  stale: boolean;
  at: number;
  why: string;
}

export interface Runner {
  id: string;
  target: string;
  ports_host: string;
  tags: string[];
  protected: string[];
  note: string;
  ok: boolean;
  why: { line: string; detail: string[] } | null;
  fetched_at_ms: number;
  host: RunnerHost;
  containers: RunnerContainer[];
  networks: { id: string; name: string; driver: string; labels: Record<string, string> }[];
  volumes: { name: string; driver: string; labels: Record<string, string> }[];
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
  /** Lines worth keeping in a universe's memory, proposed when a task was done; they wait for the human. */
  memory_suggestions: { task: string; repo: string; tag: string; title: string; lines: { kind: string; text: string }[]; at: number }[];
  journal: JournalEntry[];
  coordinator: CoordinatorState;
  /** Shipyards: machines the tasks' containers run on. */
  runners: Runner[];
  /** Whether the docker block is in this machine's agent instructions. */
  runner_block: RunnerBlock;
  theme: Theme;
  herdr: { running: boolean; socket: string };
  auth: { required: boolean; user: string };
  server: { listen: string; port: number; local_only: boolean; tls: boolean; projects_dir: string };
}

/** This machine's board as the world draws it. */
export interface GalaxyView {
  label: string;
  ok: boolean;
  tasks: Task[];
  projects: Project[];
  links: Link[];
}

export type Level = 'galaxy' | 'nebula';

export type EntityKind = 'quasar' | 'universe' | 'planet' | 'moon' | 'runner';

/** What is selected. */
export interface Entity {
  kind: EntityKind;
  /** universe: repo name · planet: task id · moon: `<task id>#<subagent id>` */
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
}

export interface SearchHit {
  kind: 'galaxy' | 'quasar' | 'universe' | 'planet' | 'moon' | 'runner' | 'todo' | 'chat' | 'memory' | 'closing';
  id: string;
  path: string;
  title: string;
  snippet: string;
  status: string;
  rank: number;
}

export const entityKey = (e: Entity): string => `${e.kind}|${e.id}`;
export const sameEntity = (a: Entity | null, b: Entity | null): boolean =>
  !!a && !!b && a.kind === b.kind && a.id === b.id;
