/* The page's state: signals. The world and the panels both read from here. */
import { computed, signal } from '@preact/signals';
import type {
  LoadInfo, Entity, GalaxyView, Level, Request, Runner, Snapshot, Status, Task, TurnData,
} from './types';
import { ATTENTION, STATUS_ORDER } from './types';
import { collectRequests } from './requests';

const QS = new URLSearchParams(location.search);
const mq = matchMedia('(prefers-reduced-motion: reduce)');

const store = (k: string, fallback: string): string => {
  try { return localStorage.getItem(k) ?? fallback; } catch { return fallback; }
};
export const persist = (k: string, v: string): void => {
  try { localStorage.setItem(k, v); } catch { /* private mode */ }
};

export const SIMPLE_KEY = 'herdr-planner.simple';
export const NOTIFY_KEY = 'herdr-planner.notify';
export const SENS_KEY = 'herdr-planner.sens';
export const INVERT_KEY = 'herdr-planner.invertY';
export const SEEN_KEY = 'herdr-planner.seen';

export const snap = signal<Snapshot | null>(null);
export const live = signal(false);
/** The machine's load for the bars in the top bar; null until the first `load` event. */
export const loadInfo = signal<LoadInfo | null>(null);
/** When (by the browser's clock) the last reading arrived: a stream that went quiet is told by it. */
export const loadRecv = signal(0);
export const setLoad = (l: LoadInfo): void => { loadInfo.value = l; loadRecv.value = Date.now(); };
export const lastError = signal('');

export const selected = signal<Entity | null>(null);
export const level = signal<Level>('galaxy');
export const universe = signal<string | null>(null);
export const mode = signal<'auto' | 'manual'>('auto');
export const locked = signal(false);
export const edge = signal(false);
export const speed = signal({ now: 0, cruise: 95 });
export const aim = signal<{ entity: Entity | null; hold: number }>({ entity: null, hold: 0 });
export const flight = signal<{ phase: 'idle' | 'accel' | 'warp' | 'decel'; target: Entity | 'overview' | null }>({ phase: 'idle', target: null });
export const hover = signal<Entity | null>(null);

export const simple = signal(QS.has('calm') || mq.matches || store(SIMPLE_KEY, 'off') === 'on');
export const notifyOn = signal(store(NOTIFY_KEY, 'off') === 'on');
export const sensitivity = signal(Number(store(SENS_KEY, '1')) || 1);
export const invertY = signal(store(INVERT_KEY, '0') === '1');
export const showQuiet = signal(true);
export const webgl = signal(true);

export const panels = {
  nav: signal(true),
  focus: signal(true),
  log: signal(false),
  wait: signal(false),
  todo: signal(false),
  chat: signal(false),
  help: signal(false),
  limits: signal(false),
  archive: signal(false),
  settings: signal(false),
  herdr: signal(false),
  quiet: signal(false),
};

export const filter = signal<Status[] | null>(null);
export const linkMode = signal(false);
export const linkSel = signal<{ from: string; to: string; kind: string; label?: string } | null>(null);
export const armed = signal<{ key: string; at: number } | null>(null);
export const narrow = signal(innerWidth < 900);
export const details = signal<Record<string, any>>({});
export const screens = signal<Record<string, any>>({});
export const answered = signal<Set<string>>(new Set());
export const navFolds = signal<Record<string, boolean>>(JSON.parse(store('herdr-planner.nav.folds', '{}') || '{}'));
export const fresh = signal<Record<string, number>>({});
export const tellDraft = signal<Record<string, string>>({});

/** The Coordinator's chat: turns of the transcript, the compact tail and a pending message. */
export const chat = signal<{
  turns: TurnData[];
  available: boolean | null;
  loading: boolean;
  loadingLatest: boolean;
  total: number;
  /** Search state. */
  q: string;
  hits: { n: number; at: number; from: string; q_snip: string; a_snip: string }[];
  hitsBusy: boolean;
  pending: { text: string; attachments: { id: string; name: string }[]; base: number; at: number } | null;
  highlight: number | null;
  error: string;
}>({
  turns: [], available: null, loading: false, loadingLatest: false, total: 0, q: '', hits: [], hitsBusy: false,
  pending: null, highlight: null, error: '',
});

export const searchState = signal<{ open: boolean; q: string; hits: any[]; cur: number }>({ open: false, q: '', hits: [], cur: 0 });

/* ───────── derived ───────── */

export const currentView = computed<GalaxyView>(() => {
  const s = snap.value;
  if (!s) return { label: '', ok: true, tasks: [], projects: [], links: [] };
  return { label: s.host || 'localhost', ok: s.board.ok, tasks: s.tasks, projects: s.projects, links: s.links };
});

export const runners = computed<Runner[]>(() => snap.value?.runners ?? []);

export const tasksIndex = computed(() => {
  const m = new Map<string, Task>();
  for (const t of currentView.value.tasks) m.set(t.id, t);
  for (const t of snap.value?.archived ?? []) if (!m.has(t.id)) m.set(t.id, t);
  return m;
});

export const taskOf = (id: string): Task | undefined => tasksIndex.value.get(id);

export const requests = computed<Request[]>(() => {
  const s = snap.value;
  if (!s) return [];
  return collectRequests({ tasks: currentView.value.tasks, coordinator: s.coordinator, runners: s.runners, suggestions: s.memory_suggestions });
});

export const attention = computed(() => requests.value.filter((r) => r.type !== 'coordinator-options').length);

export const selectedTask = computed<Task | null>(() => {
  const e = selected.value;
  if (!e || e.kind !== 'planet') return null;
  return taskOf(e.id) ?? null;
});

export const attentionOf = (s: Status): boolean => ATTENTION.includes(s);
export const urgent = (s: Status): number => STATUS_ORDER.indexOf(s);

export function stageCount(statuses: Status[]): number {
  return currentView.value.tasks.filter((t) => statuses.includes(t.status)).length;
}

export const QUERY = QS;
