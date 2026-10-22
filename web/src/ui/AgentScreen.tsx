/* An agent's terminal in a window over the Galaxy: the screen large and monospaced, refreshed while
   the window is open, and the line that returns words to the agent. Esc, «Свернуть» and a click on
   the background close it; the polling stops with it. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ApiError, api } from '../api';
import { addDialogs } from '../dialogs';
import { taskOf } from '../store';
import { t } from '../i18n';
import { clockOf } from '../util/dates';
import { Badge, Gl } from './Gl';
import { TellBox } from './AgentActions';
import { KeyButtons } from './Keys';
import { ansiSpans, type AnsiSpan } from './ansi';
import './agentscreen.css';

/** The window refreshes the screen this often while it is open (about once a second or two). */
const REFRESH_MS = 1500;

const spanStyle = (s: AnsiSpan): string | undefined => {
  const css: string[] = [];
  if (s.fg) css.push(`color:${s.fg}`);
  if (s.bg) css.push(`background:${s.bg}`);
  return css.length ? css.join(';') : undefined;
};

const spanClass = (s: AnsiSpan): string =>
  [s.bold && 'b', s.dim && 'd', s.italic && 'i', s.underline && 'u', s.inverse && 'inv'].filter(Boolean).join(' ');

/** What the server says about one agent's screen (see `api_coordinator_screen`). */
interface ScreenAnswer {
  ok: boolean;
  running?: boolean;
  target?: string;
  text?: string;
  ansi?: string;
  error?: string;
}

interface Loaded {
  task: string;
  role: string;
  r: ScreenAnswer;
  at: number;
}

export function AgentScreen(): JSX.Element | null {
  const ref = addDialogs.screen.value;
  const task = ref ? taskOf(ref.task) : undefined;
  const key = ref ? `${ref.task}|${ref.role ?? ''}` : '';
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [hidden, setHidden] = useState(() => document.hidden);
  const [again, setAgain] = useState(0);

  useEffect(() => {
    const onVisibility = (): void => setHidden(document.hidden);
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

  const shown = loaded && `${loaded.task}|${loaded.role}` === key ? loaded : null;
  const r = shown?.r;
  const at: number = shown?.at ?? 0;
  // A queued task has no agent yet, a done one has none any more, and an exited one has left:
  // there is nothing to read.
  const active = !!task && task.exited_at === 0 && task.status !== 'queued' && task.status !== 'done';
  const running = r ? r.running !== false : true;
  const paused = !active ? 'stopped' : hidden ? 'hidden' : !running ? 'herdr' : '';

  useEffect(() => {
    if (!ref) return;
    const target = ref.task;
    const role = ref.role ?? '';
    let alive = true;
    let busy = false;
    const load = async (): Promise<void> => {
      // One read at a time: a slow herdr (and the text fallback of an old one) must not pile up
      // a process per tick.
      if (busy) return;
      busy = true;
      const q = '/api/coordinator/screen?target=' + encodeURIComponent(target) + (role ? '&role=' + encodeURIComponent(role) : '');
      try {
        const answer = await api<ScreenAnswer>(q);
        if (alive) setLoaded({ task: target, role, r: answer, at: Date.now() });
      } catch (e) {
        if (!alive) return;
        const data: Partial<ScreenAnswer> = (e as ApiError).data ?? {};
        setLoaded({ task: target, role, r: { ...data, ok: false, error: (e as Error).message }, at: Date.now() });
      } finally {
        busy = false;
      }
    };
    void load();
    if (hidden || !active || !running) return () => { alive = false; };
    // A hidden tab does not ask: the timer is not even started, and the window reads at once when it is shown again.
    const timer = window.setInterval(() => { if (!document.hidden) void load(); }, REFRESH_MS);
    return () => { alive = false; window.clearInterval(timer); };
  }, [key, hidden, active, running, again]);

  if (!ref) return null;
  const close = (): void => { addDialogs.screen.value = null; };
  const refresh = (): void => setAgain((n) => n + 1);
  const title = ref.role === 'reviewer' ? t('screen.reviewer') : t('screen.title');
  const spans = r?.ansi ? ansiSpans(r.ansi) : [];
  const text = r?.text || '';

  return (
    <div class="scrim agentscreen-scrim" id="agent-screen" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel agent-screen" role="dialog" aria-modal="true" aria-label={title} data-paused={paused}>
        <header>
          <span class="sh-title">
            <Gl n="screen" />
            <b>{title}</b>
            {task && <span class="tagk">{task.tag}</span>}
            {task && <Badge status={task.status} label={task.label || t(`status.${task.status}`)} />}
          </span>
          <span class="sh-buttons">
            <span class="hint" id="screen-updated" data-at={at} title={t('screen.updatedTip')}>
              {at ? t('screen.updated', { at: clockOf(at) }) : t('req.reading')}
            </span>
            <button class="btn" type="button" data-act="screen-refresh" onClick={refresh}>
              <Gl n="refresh" />{t('req.refreshScreen')}
            </button>
            <button class="btn" type="button" data-act="screen-close" onClick={close}>
              <Gl n="shrink" />{t('focus.collapse')}
            </button>
            <button class="btn icon sm" type="button" data-act="screen-close" aria-label={t('focus.collapse')} onClick={close}>
              <Gl n="close" />
            </button>
          </span>
        </header>
        <div class="agent-screen-body">
          <div class="term-wrap">
            {r?.error ? (
              <div class="hint bad" id="screen-error">{r.error}</div>
            ) : spans.length ? (
              <pre class="term" id="screen-term">
                {spans.map((s, i) => <span key={i} class={spanClass(s)} style={spanStyle(s)}>{s.text}</span>)}
              </pre>
            ) : (
              <pre class="term" id="screen-term">{text || (at ? t('screen.empty') : t('req.reading'))}</pre>
            )}
          </div>
          <div class="agent-screen-keys">
            <span class="hint" id="screen-paused">
              {paused === 'stopped' ? t('screen.stopped') : paused === 'hidden' ? t('screen.hidden') : ''}
            </span>
            <KeyButtons target={ref.task} role={ref.role} onSent={refresh} />
          </div>
        </div>
        {task && (
          <div class="agent-screen-foot">
            <TellBox task={task} input="screen-tell" onSent={refresh} />
          </div>
        )}
      </section>
    </div>
  );
}
