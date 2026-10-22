/* One container of a task on a shipyard: «Логи» (a window with the log tail, refreshed while it is open, with a search)
   and «Остановить» / «Перезапустить» / «Удалить» (a window with «было → стало» and two presses). The buttons are locked
   with their reason for a foreign container, an unsure one and the container of a task in work seen from another
   screen; the server checks the same rule again. */
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { ApiError, post } from '../api';
import { confirmStep } from '../actions';
import { addDialogs } from '../dialogs';
import { t } from '../i18n';
import { armed, runners } from '../store';
import { toast } from '../toast';
import type { RunnerContainer } from '../types';
import { clockOf } from '../util/dates';
import { Gl } from './Gl';
import { type CtlAct, ctlGate, grep, isUp, logsGate, stripAnsi } from './ctl-gate';
import './ctl.css';

/** The log window reads again this often while it is open. */
const REFRESH_MS = 2500;
const TAILS = [100, 300, 1000];

const ACTS: CtlAct[] = ['stop', 'restart', 'delete'];

export function ContainerButtons(props: { runner: string; c: RunnerContainer; scope?: string }): JSX.Element {
  const { runner, c } = props;
  const scope = props.scope ?? '';
  const logsLock = logsGate(c);
  return (
    <span class="ctl-btns" data-id={c.id}>
      <button class="btn sm" type="button" data-ctl="logs" disabled={!!logsLock} title={logsLock ? t(logsLock) : undefined}
        onClick={() => { addDialogs.ctlLogs.value = { runner, id: c.id }; }}>{t('ctl.logs')}</button>
      {ACTS.map((act) => {
        const lock = ctlGate(c, act, scope);
        return (
          <button key={act} class={`btn sm ${act === 'delete' ? 'danger' : ''}`} type="button" data-ctl={act} disabled={!!lock} title={lock ? t(lock) : undefined}
            onClick={() => { addDialogs.ctlAct.value = { runner, id: c.id, act, scope }; }}>{t(`ctl.${act}`)}</button>
        );
      })}
    </span>
  );
}

const findContainer = (runner: string, id: string): RunnerContainer | undefined =>
  runners.value.find((r) => r.id === runner)?.containers.find((c) => c.id === id);

interface LogsAnswer { ok: boolean; text: string; cut: boolean; tail: number; name: string; state: string; at: number }

/** The line with the searched words marked. */
function marked(line: string, q: string): (string | JSX.Element)[] {
  const needle = q.trim().toLowerCase();
  if (!needle) return [line];
  const out: (string | JSX.Element)[] = [];
  const low = line.toLowerCase();
  let at = 0;
  for (let i = low.indexOf(needle); i >= 0; i = low.indexOf(needle, at)) {
    if (i > at) out.push(line.slice(at, i));
    out.push(<mark key={i}>{line.slice(i, i + needle.length)}</mark>);
    at = i + needle.length;
  }
  if (at < line.length) out.push(line.slice(at));
  return out;
}

export function ContainerLogs(): JSX.Element | null {
  const ref = addDialogs.ctlLogs.value;
  const key = ref ? `${ref.runner}|${ref.id}` : '';
  const c = ref ? findContainer(ref.runner, ref.id) : undefined;
  const [loaded, setLoaded] = useState<{ key: string; a: LogsAnswer | null; err: string; at: number } | null>(null);
  const [tail, setTail] = useState(300);
  const [q, setQ] = useState('');
  const [live, setLive] = useState(true);
  const [hidden, setHidden] = useState(() => document.hidden);
  const [again, setAgain] = useState(0);
  const wrap = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    const onVisibility = (): void => setHidden(document.hidden);
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

  const shown = loaded && loaded.key === key ? loaded : null;
  const up = c ? isUp(c) : true;
  const paused = !live ? 'off' : !up ? 'stopped' : hidden ? 'hidden' : '';

  useEffect(() => {
    if (!ref) return;
    const { runner, id } = ref;
    let alive = true;
    let busy = false;
    const load = async (): Promise<void> => {
      // One read at a time: a slow yard must not collect a process per tick.
      if (busy) return;
      busy = true;
      try {
        const a = await post<LogsAnswer>('/api/runner/container/logs', { runner, id, tail });
        if (alive) setLoaded({ key, a, err: '', at: Date.now() });
      } catch (e) {
        if (alive) setLoaded((was) => ({ key, a: was && was.key === key ? was.a : null, err: (e as ApiError).message, at: Date.now() }));
      } finally {
        busy = false;
      }
    };
    void load();
    // A hidden tab, a stopped container and a switched-off refresh do not ask again.
    if (!live || !up || hidden) return () => { alive = false; };
    const timer = window.setInterval(() => { if (!document.hidden) void load(); }, REFRESH_MS);
    return () => { alive = false; window.clearInterval(timer); };
  }, [key, tail, live, up, hidden, again]);

  const text = shown?.a?.text ?? '';
  useEffect(() => {
    const el = wrap.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [text, q]);

  if (!ref) return null;
  const close = (): void => { addDialogs.ctlLogs.value = null; };
  const found = grep(stripAnsi(text), q);
  const name = shown?.a?.name || c?.name || '';
  const note = paused === 'off' ? t('ctl.logsOff') : paused === 'stopped' ? t('ctl.logsStopped') : paused === 'hidden' ? t('screen.hidden') : '';
  return (
    <div class="scrim ctl-scrim" id="ctl-logs" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel ctl-logs" role="dialog" aria-modal="true" aria-label={t('ctl.logsTitle')} data-paused={paused}>
        <header>
          <span class="sh-title">
            <Gl n="screen" />
            <b>{t('ctl.logsTitle')}</b>
            <code>{name}</code>
            <span class="tagk">{ref.runner}</span>
            {c?.task && <span class="tagk">{c.task.tag}</span>}
            {c && <span class={`ctl-state s-${c.state}`}>{stateWord(c.state)}</span>}
          </span>
          <span class="sh-buttons">
            <span class="hint" id="ctl-updated" data-at={shown?.at ?? 0}>{shown ? t('screen.updated', { at: clockOf(shown.at) }) : t('req.reading')}</span>
            <button class="btn" type="button" data-act="ctl-close" onClick={close}><Gl n="shrink" />{t('focus.collapse')}</button>
            <button class="btn icon sm" type="button" data-act="ctl-close" aria-label={t('close')} onClick={close}><Gl n="close" /></button>
          </span>
        </header>
        <div class="ctl-bar">
          <input class="field" type="search" id="ctl-search" value={q} placeholder={t('ctl.logsSearch')} aria-label={t('ctl.logsSearch')}
            onInput={(e) => setQ((e.target as HTMLInputElement).value)} />
          <label class="ctl-tail">{t('ctl.logsTail')}
            <select class="field" id="ctl-tail" value={String(tail)} onChange={(e) => setTail(Number((e.target as HTMLSelectElement).value))}>
              {TAILS.map((n) => <option key={n} value={String(n)}>{n}</option>)}
            </select>
          </label>
          <label class="ctl-live"><input type="checkbox" id="ctl-live" checked={live} onChange={(e) => setLive((e.target as HTMLInputElement).checked)} />{t('ctl.logsLive')}</label>
          <button class="btn" type="button" id="ctl-refresh" onClick={() => setAgain((n) => n + 1)}><Gl n="refresh" />{t('ctl.logsRefresh')}</button>
          <span class="hint" id="ctl-count">{q.trim() ? t('ctl.logsFound', { n: found.lines.length, total: found.total }) : t('ctl.logsLines', { n: found.total })}</span>
        </div>
        <div class="ctl-body">
          <div class="term-wrap" ref={wrap} onScroll={(e) => {
            const el = e.currentTarget as HTMLDivElement;
            stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          }}>
            {shown?.err && !shown.a ? <div class="hint bad" id="ctl-error">{shown.err}</div> : (
              <pre class="term" id="ctl-term">
                {!shown ? t('req.reading') : found.lines.length === 0 ? (found.total === 0 ? t('ctl.logsEmpty') : t('ctl.logsNoMatch'))
                  : found.lines.map((l, i) => <div key={i} class="ll">{marked(l, q)}</div>)}
              </pre>
            )}
          </div>
          <div class="ctl-foot">
            {shown?.a?.cut && <span class="hint">{t('ctl.logsCut')}</span>}
            {shown?.err && shown.a && <span class="hint bad" id="ctl-error">{shown.err}</span>}
            <span class="hint" id="ctl-paused">{note}</span>
          </div>
        </div>
      </section>
    </div>
  );
}

const STATES = ['running', 'exited', 'paused', 'restarting', 'created', 'dead', 'gone'];
const stateWord = (s: string): string => (STATES.includes(s) ? t(`ctl.state.${s}`) : s);

interface Side { state: string; status: string }
interface Preview {
  ok: boolean; runner: string; act: CtlAct; id: string; name: string; image: string; task: string; ports: string; mounts: string;
  was: Side; now: Side; token: string; expires_ms: number;
}

export function ContainerAct(): JSX.Element | null {
  const ref = addDialogs.ctlAct.value;
  const key = ref ? `${ref.runner}|${ref.id}|${ref.act}|${ref.scope}` : '';
  const [plan, setPlan] = useState<{ key: string; p: Preview | null; err: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [actErr, setActErr] = useState('');
  const [again, setAgain] = useState(0);

  useEffect(() => { setActErr(''); }, [key]);
  useEffect(() => {
    if (!ref) return;
    let alive = true;
    setPlan(null);
    post<Preview>('/api/runner/container/preview', { runner: ref.runner, id: ref.id, act: ref.act, task: ref.scope })
      .then((p) => { if (alive) setPlan({ key, p, err: '' }); })
      .catch((e: ApiError) => { if (alive) setPlan({ key, p: null, err: e.message }); });
    return () => { alive = false; };
  }, [key, again]);

  if (!ref) return null;
  const close = (): void => { addDialogs.ctlAct.value = null; };
  const shown = plan && plan.key === key ? plan : null;
  const p = shown?.p ?? null;
  const pressKey = `ctl:${key}`;
  const armedNow = armed.value?.key === pressKey;
  const run = (): void => {
    if (!p) return;
    confirmStep(pressKey, async () => {
      setBusy(true);
      try {
        await post('/api/runner/container/act', { runner: ref.runner, token: p.token, confirm: true });
        toast(t(`ctl.done.${ref.act}`, { name: p.name }), { icon: 'done' });
        // The answer may come late: close only the card it was for, not one opened since.
        const now = addDialogs.ctlAct.value;
        if (now && `${now.runner}|${now.id}|${now.act}|${now.scope}` === key) close();
      } catch (e) {
        // The press spent its token: the card is made again from the yard as it is now, with the reason above it.
        setActErr((e as ApiError).message);
        setAgain((n) => n + 1);
      } finally {
        setBusy(false);
      }
    });
  };
  const row = (label: string, value: string): JSX.Element | null => (value ? <><span>{label}</span><b>{value}</b></> : null);
  return (
    <div class="scrim ctl-scrim" id="ctl-act" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel ctl-act" role="dialog" aria-modal="true" aria-label={t(`ctl.title.${ref.act}`)} data-act={ref.act}>
        <header>
          <span class="sh-title"><b>{t(`ctl.title.${ref.act}`)}</b></span>
          <span class="sh-buttons">
            <button class="btn icon sm" type="button" data-act="ctl-close" aria-label={t('close')} onClick={close}><Gl n="close" /></button>
          </span>
        </header>
        <div class="ctl-act-body">
          {!shown && <div class="hint">{t('ctl.loading')}</div>}
          {(actErr || shown?.err) && <p class="hint bad" id="ctl-act-error">{actErr || shown?.err}</p>}
          {p && (
            <>
              <div class="kv ctl-kv">
                {row(t('ctl.rowContainer'), p.name)}
                {row(t('ctl.rowYard'), p.runner)}
                {row(t('ctl.rowTask'), p.task)}
                {row(t('ctl.rowImage'), p.image)}
                {row(t('ctl.rowPorts'), p.ports)}
              </div>
              <div class="ctl-was" id="ctl-was">
                <div class="ctl-side"><span class="lbl">{t('ctl.was')}</span><b class={`ctl-state s-${p.was.state}`}>{stateWord(p.was.state)}</b><span class="hint">{p.was.status}</span></div>
                <span class="ctl-arrow" aria-hidden="true">→</span>
                <div class="ctl-side"><span class="lbl">{t('ctl.now')}</span><b class={`ctl-state s-${p.now.state}`}>{stateWord(p.now.state)}</b></div>
              </div>
              <p class="hint">{t(`ctl.note.${p.act}`)}</p>
              {p.act === 'delete' && p.mounts && <p class="hint warn">{t('ctl.noteMounts', { mounts: p.mounts })}</p>}
              <div class="act">
                <button class={`btn danger ${armedNow ? 'arm' : ''}`} type="button" id="ctl-go" disabled={busy} onClick={run}>
                  {armedNow ? t(`ctl.confirm.${p.act}`) : t(`ctl.${p.act}`)}
                </button>
                <button class="btn" type="button" id="ctl-cancel" onClick={close}>{t('cancel')}</button>
              </div>
            </>
          )}
          {shown && !p && <div class="act"><button class="btn" type="button" id="ctl-cancel" onClick={close}>{t('cancel')}</button></div>}
        </div>
      </section>
    </div>
  );
}
