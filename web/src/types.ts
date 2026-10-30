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

/** Who is on a task now: the reviewer while it is under review, the working agent while it works. */
export interface Seat {
  role: 'reviewer' | 'worker';
  program: string;
  model: string;
  took_over: boolean;
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
  seat?: Seat | null;
  /** The crew's own roles put on the task (the planner's record). */
  crew_roles?: RoleMark[];
}

/** One role of the crew's own on a task's row: its name, how it went on, where it came from. */
export interface RoleMark {
  role: string;
  name: string;
  /** `worker`, `instead`, `after` (the extra check runs after the task) or `followup` (the task is that check). */
  how: 'worker' | 'instead' | 'after' | 'followup';
  by: 'chosen' | 'rule';
  rule: string;
  of: string;
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
  /** Share used, 0..1. */
  percent: number;
  severity: 'ok' | 'warn' | 'crit';
  resets_at_ms: number | null;
  /** `session`, `daily`, `weekly`, `monthly`, `named` (the label is the service's own name for it), or empty. */
  kind?: string;
  /** How long the window runs (ms); 0 when unknown. */
  length_ms?: number;
  /** Percent points in reserve (positive) or in deficit (negative) against an even spend; absent when the pace cannot be judged. */
  reserve?: number | null;
  /** When the window runs out at the pace so far, ms from the reading; absent when it outlasts its reset. */
  runs_out_ms?: number | null;
}

export type UsageSection =
  | { kind: 'facts'; title: string; facts: [string, string][] }
  | { kind: 'shares'; title: string; shares: [string, number][] };

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
  /** `live` (the planner asked the provider), `omarchy` (Omarchy's record), or empty. */
  source?: string;
  account?: string;
  /** Why the planner's own read failed: `rejected`, `expired`, `status:500`… */
  why?: string;
  extra?: LimitWindow[];
  sections?: UsageSection[];
  dashboard?: string;
  status_page?: string;
  /** Whose sign-in was asked with (`pi`, `omp`, `claude`, `env`, …), the crew role that runs the provider through it, and the other programs it runs through. */
  key_from?: string;
  key_role?: string;
  key_also?: string[];
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
  /** The disk, when the machine reports it (bytes, share 0..100). */
  disk?: { used: number; total: number; percent: number };
}

export interface Usage {
  agents: AgentUsage[];
  stale_after_secs?: number;
  /** Providers the planner reads itself that have no sign-in on this machine. */
  unconnected?: string[];
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
  /** What the planner told the Coordinator about the human's actions: shown as notices, not as the human's lines. */
  notices: ChatNotice[];
  subagents: Subagent[];
  subagents_note: string;
  /** Tokens of the last turn against the window of the model (0 when unknown). */
  context: { used: number; window: number; percent: number; known: boolean; model: string };
  can_compact: boolean;
  program: string;
}

export interface ChatNotice {
  text: string;
  at: number;
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

/** One published port of a container, parsed from docker's `0.0.0.0:15432->5432/tcp`. */
export interface RunnerPort {
  host_ip: string;
  /** The port on the shipyard; null for a range, which has no single link. */
  host_port: number | null;
  container_port: number | null;
  proto: string;
  /** A known service of the port: the page prints its client command. */
  service: 'postgres' | 'redis' | null;
  /** A web port of an all-interface binding: the page makes a link of it. */
  http: boolean;
  https: boolean;
  /** Bound to the shipyard's own loopback: no link and no command from here. */
  loopback: boolean;
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
  /** Published ports out of `ports`; empty for an older saved read (the page falls back to the text). */
  published?: RunnerPort[];
  networks: string;
  mounts: string;
  size: string;
  size_bytes: number;
  cpu_pct: number | null;
  mem_bytes: number | null;
  mem_limit: number | null;
  /** The CPU limit in cores; null — no limit at all. */
  cpu_limit?: number | null;
  /** A container of a live task that has no limit the yard's rule asks for (PLN-43). */
  no_limits?: boolean;
  pids: number | null;
  net_io: string;
  block_io: string;
  gpu_mem_mib: number | null;
  task: RunnerTaskRef | null;
  class: 'active' | 'forgotten' | 'unsure' | 'foreign' | 'shared';
}

/** One thing of a board repository's build cache (`herdr.shared=<repo>` on the yard, scripts/yard.sh). */
export interface RunnerShared {
  kind: 'container' | 'image' | 'volume';
  name: string;
  repo: string;
  size_bytes: number | null;
}

export interface RunnerBlock {
  connected: boolean;
  stale: boolean;
  at: number;
  why: string;
}

/** The auto-clean switch of a shipyard and what it has done. */
export interface RunnerAuto { enabled: boolean; hours: number }
export interface RunnerAutoStats { containers: number; networks: number; volumes: number; runs: number; last_ms: number; last_line: string }

/** An alarm of a shipyard (PLN-42): little disk, a long silence, a task's container fallen, a forgotten container. */
export interface RunnerAlert {
  key: string;
  runner: string;
  kind: 'offline' | 'disk' | 'crash' | 'forgotten';
  since_ms: number;
  free_pct?: number;
  free_bytes?: number;
  total_bytes?: number;
  minutes?: number;
  name?: string;
  tag?: string;
  task?: string;
  state?: string;
  status?: string;
}

/** When a shipyard is worth an alarm, as `runners.json` has it. */
export interface RunnerAlertRules { disk_free_pct: number; offline_min: number }

/** What a task's container may take on a shipyard (PLN-43); zero asks for no limit of that kind. */
export interface RunnerLimits { cpus: number; mem_mb: number; per_task: number }

/** A task's or a repository's rule for its yard (PLN-45): a yard by label, tags it needs, or both. */
export interface RunnerTaskRule { runner?: string; tags?: string[] }

/** The rules of the list file, as the page reads and edits them. */
export interface RunnerRules {
  default: string;
  busy_pct: number;
  assign: Record<string, string>;
  need: Record<string, string[]>;
  tasks: Record<string, RunnerTaskRule>;
}

/** The yard chosen for a task or a repository, with what stands behind it. */
export interface RunnerPick {
  runner: string;
  target: string;
  ports_host: string;
  /** `task`, `repo` or `default`. */
  reason: 'task' | 'repo' | 'default';
  need: string[];
  preferred: string;
  chain: string[];
  /** `busy` or `offline` when the first allowed yard was passed over. */
  shifted: '' | 'busy' | 'offline';
  why: string;
}

/** A live task that keeps more containers than the shipyard allows. */
export interface RunnerOver { task: string; tag: string; containers: number; max: number }

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
  /** The build caches of the board's repositories: never a task's, never forgotten, never touched here. */
  shared: RunnerShared[];
  auto: RunnerAuto;
  auto_stats: RunnerAutoStats;
  /** The alarms of this yard, from its last read. */
  alerts: RunnerAlert[];
  /** The thresholds as the list file has them now. */
  alert_rules: RunnerAlertRules;
  /** The limits of a task's containers, as the list file has them now. */
  limits: RunnerLimits;
  /** This yard's own busy threshold in percent, `null` — the list's (PLN-45). */
  busy_pct: number | null;
  /** The CPU limit of this yard's check container, cores; 0 — no limit (PLN-58). */
  check_cpus: number;
  /** How many full checks may run on this yard at once; 0 — no queue (PLN-58). */
  check_slots: number;
  /** Live tasks that keep more containers than the yard allows. */
  over: RunnerOver[];
  /** When the yard stopped answering (ms); 0 while it answers. */
  down_since_ms: number;
}

/** A row of a shipyard's journal (PLN-47): who did what and when. */
export interface RunnerLogEntry {
  n: number;
  at: number;
  kind: 'stop' | 'restart' | 'delete' | 'clean' | 'disk' | 'auto' | 'rules' | 'down' | 'remove';
  who: 'human' | 'auto' | 'planner';
  task: string;
  tag: string;
  name: string;
  ok: boolean;
  why: string;
  /** For a downtime row: when the yard answered again (0 — it is silent now). */
  until: number;
  data: {
    action?: 'run' | 'switch';
    containers?: number;
    networks?: number;
    volumes?: number;
    bytes?: number;
    stopped?: number;
    builders?: number;
    images?: boolean;
    cache?: boolean;
    failed?: number;
    on?: boolean;
    hours?: number;
    drop_old?: boolean;
    settings?: number;
    tags?: string[];
    names?: string[];
    force?: boolean;
    warned?: string[];
  };
}

export interface RunnerLogData {
  ok: boolean;
  at: number;
  entries: RunnerLogEntry[];
  more: boolean;
  keep_days: number;
}

export interface Snapshot {
  version: string;
  at: number;
  seq: number;
  /** Only in the light page's projection: grows when the Coordinator's transcript does. */
  chat_rev?: number;
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
  memory_suggestions: MemorySuggestion[];
  journal: JournalEntry[];
  coordinator: CoordinatorState;
  /** Shipyards: machines the tasks' containers run on. */
  runners: Runner[];
  /** The yard chosen for every task, by task id. */
  runner_picks: Record<string, RunnerPick>;
  /** The yard chosen for every repository. */
  runner_repo_picks: Record<string, RunnerPick>;
  /** The rules behind those picks (PLN-45). */
  runner_rules: RunnerRules;
  /** Whether the docker block is in this machine's agent instructions. */
  runner_block: RunnerBlock;
  theme: Theme;
  herdr: { running: boolean; socket: string };
  auth: { required: boolean; user: string };
  server: { listen: string; port: number; local_only: boolean; tls: boolean; projects_dir: string; attachments_dir: string };
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
  /** What the Coordinator is doing now (its last tool call), only on a turn that is still running. */
  doing?: string;
  /** When the answer was written; absent while there is none. */
  at_a?: number;
  /** Media types of the images in the human's message; `/api/chat/image/{n}/{i}` serves them. */
  images?: string[];
}

export interface MemorySuggestion { task: string; repo: string; tag: string; title: string; lines: { kind: string; text: string }[]; at: number }

export interface Request {
  key: string;
  type: string;
  title: string;
  id: string;
  tag: string;
  repo: string;
  msg: string;
  text: string;
  /** Why the human is needed, in one word: «план», «разрешение», «вопрос», «упала», «к одобрению». */
  why?: string;
  task?: Task;
  options?: { label: string; value: string; short: string }[];
  /** An alarm of a shipyard: the row leads to that yard's focus, not to a task. */
  alert?: RunnerAlert;
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

/** A choice of a selection menu an agent shows in its terminal (see src/menu.rs). */
export interface MenuOption {
  /** The row in the program's list: where the cursor has to go. */
  at: number;
  label: string;
  desc: string;
  checked: boolean;
  /** The row that takes the person's own words («Type something», «Other»). */
  own: boolean;
}

export interface MenuData {
  program: string;
  /** Changes with the question and its choices, not with the cursor or the ticks. */
  sig: string;
  question: string;
  context: string;
  options: MenuOption[];
  multi: boolean;
  cursor: number;
  submit_at: number | null;
  own_editor: boolean;
  step: number;
  steps: number;
  tabs: string[];
}

export type MenuAnswer =
  | { kind: 'pick'; at: number }
  | { kind: 'own'; text: string }
  | { kind: 'multi'; picked: number[]; own?: string }
  | { kind: 'cancel' };
