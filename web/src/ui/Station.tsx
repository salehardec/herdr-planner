/* The Station («Станция»): the settings of this machine's board, the shipyards and the strays. The crew (who runs the
   Coordinator, the planner, the reviewer, the workers, the light tasks and the spares), the crew's own roles, the presets
   and the subscriptions are in the «Экипаж» window (CrewWindow.tsx), which builds on the pieces exported here. Every
   change goes the same way: a preview («было → стало», the tasks it touches), then two presses (confirmStep), then
   the board's own command. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, currentView, snap } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { api, post } from '../api';
import { dialogOpen } from '../dialogs';
import { Gl } from './Gl';
import { Strays } from './Strays';
import { AgentPick, ChainEditor, programMark, useModels } from './CrewPick';
import { chainOf, chainValue } from './crew-view';
import { coachShown, firstRunShown } from './FirstRun';
import {
  callError, CallError, closeStation, crewDlg, crewSeen, fromLabel, isBool, openCrew, PROGRAM_ORDER, programOf,
  Preview, ROLE_SETTINGS, stationDlg, stationGet, stationLight, stationModels, stationPreview, stationSet, StationData,
  StationProgram, StationSetting,
} from './station-api';
import './station.css';

export { lightPreset } from './station-api';

/* ───────── the change in progress (one at a time, shared by every row) ───────── */

interface Edit {
  /** The setting's name, or `light` for the «Лёгкие задачи» preset (kept by the planner, not the board). */
  name: string;
  repo: string;
  /** `null` unsets the setting. */
  value: string | null;
  light?: boolean;
}

const edit = signal<Edit | null>(null);
const prev = signal<Preview | null>(null);
const prevErr = signal<CallError | null>(null);
const setErr = signal<CallError | null>(null);
const saving = signal(false);

export const resetEdit = (): void => {
  edit.value = null;
  prev.value = null;
  prevErr.value = null;
  setErr.value = null;
};

const editKey = (e: Edit): string => `st:${e.repo}|${e.name}|${e.value ?? '-'}`;
const sameEdit = (e: Edit | null, name: string, repo: string, light: boolean): boolean =>
  !!e && e.name === name && e.repo === repo && !!e.light === light;

/** A preview of the change: the board's answer, or the planner's own for the light preset. */
async function startEdit(e: Edit, lightWas = ''): Promise<void> {
  edit.value = e;
  prev.value = null;
  prevErr.value = null;
  setErr.value = null;
  if (e.light) {
    prev.value = { ok: true, name: 'light', repo: '', was: lightWas, was_from: '', now: e.value, valid: true, error: '', affects: [], note: '' };
    return;
  }
  try {
    const p = await stationPreview({
      name: e.name, repo: e.repo || undefined, ...(e.value === null ? { unset: true } : { value: e.value }),
    });
    if (edit.value === e) prev.value = p;
  } catch (err) {
    if (edit.value === e) prevErr.value = callError(err);
  }
}

/** The second press of the two-step confirm sends the change; the board's refusal stays on the card. */
function confirmEdit(reload: () => void): void {
  const e = edit.value;
  const p = prev.value;
  if (!e || !p || !p.valid || saving.value) return;
  confirmStep(editKey(e), async () => {
    saving.value = true;
    setErr.value = null;
    try {
      if (e.light) await stationLight(e.value ?? '');
      else await stationSet({ name: e.name, repo: e.repo || undefined, ...(e.value === null ? { unset: true } : { value: e.value }) });
      toast(t('st.saved'));
      resetEdit();
      reload();
    } catch (err) {
      if (edit.value === e) setErr.value = callError(err);
    } finally {
      saving.value = false;
    }
  });
}

const shown = (v: string | null | undefined): string => (v ? v : t('st.empty'));

/** The card of the change in progress: was → now, the tasks it touches, the two presses. */
function ChangeCard(props: { reload: () => void }): JSX.Element | null {
  const e = edit.value;
  if (!e) return null;
  const p = prev.value;
  const pe = prevErr.value;
  const se = setErr.value;
  const on = armed.value?.key === editKey(e);
  return (
    <div class={`chg ${(p && !p.valid) || pe || se ? 'bad' : ''}`} id="st-change" data-edit={e.light ? 'light' : e.name}>
      <div class="chg-h">{t('st.changeOf')} <code>{e.light ? t('st.lightName') : e.name}</code>{e.repo && <span class="m"> · {e.repo}</span>}</div>
      {pe && (
        <>
          <div class="hint bad" id="st-prev-err">{pe.message}</div>
          <div class="act"><button class="btn sm" type="button" id="st-retry-prev" onClick={() => void startEdit(e)}>{t('st.retry')}</button><button class="btn sm" type="button" onClick={resetEdit}>{t('cancel')}</button></div>
        </>
      )}
      {!pe && !p && <div class="hint">{t('st.checking')}</div>}
      {p && (
        <>
          <div class="ba">
            <span class="kl">{t('st.was')}</span> <code class="was">{shown(p.was)}</code>
            <span class="arr" aria-hidden="true">→</span>
            <span class="kl">{t('st.now')}</span> <code class="now" id="st-now">{p.now === null ? t('st.unsetNow') : shown(p.now)}</code>
          </div>
          {p.was_from && !e.light && <div class="hint">{t('st.wasFrom', { from: fromLabel(p.was_from) })}</div>}
          {!p.valid && <div class="hint bad" id="st-invalid">{p.error}</div>}
          {e.light
            ? <div class="hint" id="st-affects">{t('st.lightAffects')}</div>
            : <div class="hint" id="st-affects">{p.affects.length ? tn('st.affects', p.affects.length) : t('st.affectsNone')}</div>}
          {p.affects.length > 0 && (
            <ul class="aff">
              {p.affects.slice(0, 8).map((a) => <li key={a.id}><span class="tg">{a.id}</span> {a.title} <span class="m">{a.status}</span></li>)}
              {p.affects.length > 8 && <li class="m">{t('st.andMore', { n: p.affects.length - 8 })}</li>}
            </ul>
          )}
          {p.note && <div class="hint">{p.note}</div>}
          <div class="hint">{e.light ? t('st.lightWhere') : t('st.byBoard')}</div>
          {se && <div class="hint bad" id="st-set-err">{se.message}</div>}
          <div class="act">
            <button class={`btn primary ${on ? 'arm' : ''}`} type="button" id="st-confirm" disabled={!p.valid || saving.value} onClick={() => confirmEdit(props.reload)}>
              {on ? t('st.confirm') : t('st.apply')}
            </button>
            <button class="btn" type="button" id="st-cancel" onClick={resetEdit}>{t('cancel')}</button>
          </div>
        </>
      )}
    </div>
  );
}

/* ───────── the forms that start a change ───────── */

/** A program and one of its models («codex/gpt-5-codex»), both picked from what the program takes. */
function AgentForm(props: {
  programs: StationProgram[]; current: string; clear?: { label: string; value: string };
  dlid: string; onPick: (value: string) => void; onCancel: () => void;
}): JSX.Element {
  const [agent, setAgent] = useState(props.current);
  return (
    <div class="fform">
      <AgentPick programs={props.programs} value={agent} onChange={setAgent} id={props.dlid} />
      <div class="act">
        <button class="btn primary sm" type="button" id={`st-next-${props.dlid}`} disabled={!agent || agent === props.current} onClick={() => props.onPick(agent)}>{t('st.next')}</button>
        {props.clear && <button class="btn sm" type="button" onClick={() => props.onPick(props.clear?.value ?? '')}>{props.clear.label}</button>}
        <button class="btn sm" type="button" onClick={props.onCancel}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/** A fallback chain: the subscriptions that take over, in order, when the role's agent cannot go on. */
function ChainForm(props: { programs: StationProgram[]; current: string; agent: string; dlid: string; onPick: (value: string) => void; onCancel: () => void }): JSX.Element {
  const start = chainValue(chainOf(props.current));
  const [value, setValue] = useState(start);
  const off = props.programs.filter((p) => p.installed && p.state.avail.state === 'limited');
  return (
    <div class="fform">
      <ChainEditor programs={props.programs} value={value} onChange={setValue} id={props.dlid} agent={props.agent} />
      {off.length > 0 && <div class="hint">{t('st.chainLimitedHint', { list: off.map((p) => `${p.name} ${programMark(p)}`).join(', ') })}</div>}
      <div class="act">
        <button class="btn primary sm" type="button" id={`st-next-${props.dlid}`} disabled={value === start} onClick={() => props.onPick(value)}>{t('st.next')}</button>
        <button class="btn sm" type="button" onClick={props.onCancel}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/** The reasoning effort a role's program takes (its models answer lists the levels). */
function EffortForm(props: { agent: string; current: string; dlid: string; onPick: (value: string) => void; onCancel: () => void }): JSX.Element {
  const prog = programOf(props.agent);
  const { data, loading } = useModels(prog);
  const levels = data?.effort ?? [];
  const [pick, setPick] = useState(props.current);
  const all = levels.includes(props.current) || !props.current ? levels : [props.current, ...levels];
  return (
    <div class="fform">
      <label class="fl2">{t('st.effortLevel', { p: prog })}
        <select class="field" id={`st-effort-${props.dlid}`} value={pick} onChange={(e) => setPick((e.target as HTMLSelectElement).value)}>
          {all.map((l) => <option key={l} value={l}>{l}</option>)}
        </select>
      </label>
      <div class="hint">{loading ? t('st.modelsLoading') : levels.length ? '' : t('st.noEffort', { p: prog })}</div>
      <div class="act">
        <button class="btn primary sm" type="button" id={`st-next-${props.dlid}`} disabled={!pick || pick === props.current} onClick={() => props.onPick(pick)}>{t('st.next')}</button>
        <button class="btn sm" type="button" onClick={props.onCancel}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/** One value of a role: the current one, where it comes from, and the change. */
function Field(props: {
  repo: string; kind: 'agent' | 'effort' | 'fallback' | 'light'; name: string; label: string; value: string; from: string;
  programs: StationProgram[]; agent: string; reload: () => void; suggest?: string;
  /** The level has its own entry for the setting: it can be taken away (the board's `unset`). */
  own?: boolean;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const light = props.kind === 'light';
  const e = edit.value;
  const mine = sameEdit(e, props.name, props.repo, light);
  const start = (value: string): void => {
    setOpen(false);
    void startEdit({ name: props.name, repo: props.repo, value, light }, props.value);
  };
  return (
    <div class="fld" data-field={props.kind}>
      <div class="fl">
        <span class="kl">{props.label}</span>
        <b class="v" data-value={props.name}>{shown(props.value)}</b>
        {props.from && !light && <span class="src">{fromLabel(props.from)}</span>}
        <button class="btn sm" type="button" data-act={`st-edit-${props.name}`} aria-expanded={open} onClick={() => setOpen(!open)}>{t('st.change')}</button>
        {props.own && !light && <button class="btn sm danger" type="button" data-unset={props.name} onClick={() => { setOpen(false); void startEdit({ name: props.name, repo: props.repo, value: null }); }}>{t('st.reset')}</button>}
      </div>
      {props.suggest && props.suggest !== props.value && (
        <div class="sugline">{t('st.suggest')} <code>{props.suggest}</code>
          <button class="btn sm primary" type="button" data-suggest={props.name} onClick={() => void startEdit({ name: props.name, repo: props.repo, value: props.suggest ?? '', light }, props.value)}>{t('st.choose')}</button>
        </div>
      )}
      {open && props.kind === 'effort' && (
        <EffortForm agent={props.agent} current={props.value} dlid={props.name} onPick={start} onCancel={() => setOpen(false)} />
      )}
      {open && props.kind === 'fallback' && (
        <ChainForm programs={props.programs} current={props.value} agent={props.agent} dlid={props.name} onPick={start} onCancel={() => setOpen(false)} />
      )}
      {open && props.kind !== 'effort' && props.kind !== 'fallback' && (
        <AgentForm
          programs={props.programs} current={props.value} dlid={props.name}
          clear={light ? { label: t('st.clearLight'), value: '' } : undefined}
          onPick={start} onCancel={() => setOpen(false)}
        />
      )}
      {mine && <ChangeCard reload={props.reload} />}
    </div>
  );
}

/* ───────── the sections ───────── */

const ROLES: { id: string; title: string; hint: string; agent?: string; effort?: string; fallback?: string; light?: boolean }[] = [
  { id: 'coordinator', title: 'st.r.coordinator', hint: 'st.r.coordinatorHint', agent: 'coordinator.agent', effort: 'coordinator.effort' },
  { id: 'plan', title: 'st.r.plan', hint: 'st.r.planHint', agent: 'plan.agent' },
  { id: 'reviewer', title: 'st.r.reviewer', hint: 'st.r.reviewerHint', agent: 'reviewer.agent', effort: 'reviewer.effort', fallback: 'reviewer.fallback' },
  { id: 'worker', title: 'st.r.worker', hint: 'st.r.workerHint', agent: 'worker.agent', effort: 'worker.effort', fallback: 'worker.fallback' },
  { id: 'watcher', title: 'st.r.watcher', hint: 'st.r.watcherHint', agent: 'watcher.agent' },
  { id: 'light', title: 'st.r.light', hint: 'st.r.lightHint', light: true },
];

/** The first program of the order that is installed and signed in (or needs no sign-in), and a model of it for each role. */
export async function suggestCrew(data: StationData): Promise<Record<string, string>> {
  const prog = PROGRAM_ORDER.find((p) => data.programs.some((x) => x.name === p && x.installed && ['ok', 'unknown'].includes(x.state.avail.state)));
  if (!prog) return {};
  const m = await stationModels(prog);
  if (!m.names.length) return {};
  /* The first model of the preferred words, in their order; the program's first model when none of them is there. */
  const pick = (prefer: string[]): string => {
    for (const w of prefer) {
      const hit = m.names.find((n) => n.includes(w));
      if (hit) return hit;
    }
    return m.names[0];
  };
  const big = pick(['opus', 'pro', 'gpt-5', 'sonnet']);
  const work = pick(['sonnet', 'gpt-5', 'pro', 'opus']);
  const light = pick(['haiku', 'mini', 'flash', 'lite']);
  return { 'coordinator.agent': big, 'plan.agent': big, 'reviewer.agent': big, 'worker.agent': work, light };
}

/** What a standard role sets on the board, and on which live tasks it works now (the seat the board gives it). */
function RoleFacts(props: { role: { id: string; agent?: string; effort?: string; fallback?: string; light?: boolean } }): JSX.Element {
  const r = props.role;
  const names = [r.agent, r.effort, r.fallback].filter(Boolean) as string[];
  const seated = r.id === 'worker' || r.id === 'reviewer'
    ? currentView.value.tasks.filter((x) => !x.archived && x.seat?.role === r.id)
    : null;
  return (
    <div class="hint cr-std" data-std-sets={r.id}>
      {r.light ? t('cr.stdLight') : <>{t('cr.stdSets')}: {names.map((n, i) => <span key={n}>{i > 0 ? ', ' : ''}<code>{n}</code></span>)}</>}
      {seated && (
        <span data-std-tasks={seated.length}> · {seated.length === 0
          ? t('cr.usage.none')
          : <>{t('cr.usage.on')} {seated.slice(0, 6).map((x, i) => <span key={x.id} class="tg">{i > 0 ? ', ' : ''}{x.tag}</span>)}{seated.length > 6 ? ` +${seated.length - 6}` : ''}</>}</span>
      )}
    </div>
  );
}

/** The standard roles: the board's own settings of each, changed through «было → стало». */
export function Crew(props: { repo: string; data: StationData; reload: () => void; wizard: boolean; suggest: Record<string, string> }): JSX.Element {
  const { data } = props;
  const val = (name: string): StationSetting | undefined => data.settings.find((s) => s.name === name);
  return (
    <section class="st-sec" id="st-crew">
      <h3>{props.wizard ? t('st.crew') : t('st.stdRoles')}</h3>
      <p class="hint">{props.wizard ? t('st.crewWizHint') : t('st.crewHint')}</p>
      {ROLES.map((r) => {
        const agent = r.agent ? val(r.agent) : undefined;
        const agentValue = r.light ? data.light : agent?.value ?? '';
        const common = { repo: props.repo, programs: data.programs, agent: agentValue, reload: props.reload };
        return (
          <div class="role" data-role={r.id} key={r.id}>
            <div class="rh"><b>{t(r.title)}</b><span class="hint">{t(r.hint)}</span></div>
            <RoleFacts role={r} />
            {r.agent && (
              <Field {...common} kind="agent" name={r.agent} label={t('st.f.agent')} value={agentValue} from={agent?.from ?? ''} own={agent?.own}
                suggest={props.suggest[r.agent]} />
            )}
            {r.effort && (
              <Field {...common} kind="effort" name={r.effort} label={t('st.f.effort')} value={val(r.effort)?.value ?? ''} from={val(r.effort)?.from ?? ''} own={val(r.effort)?.own} />
            )}
            {r.fallback && (
              <Field {...common} kind="fallback" name={r.fallback} label={t('st.f.fallback')} value={val(r.fallback)?.value ?? ''} from={val(r.fallback)?.from ?? ''} own={val(r.fallback)?.own} />
            )}
            {r.light && (
              <Field {...common} kind="light" name="light" label={t('st.f.light')} value={data.light} from="" suggest={props.suggest.light} />
            )}
          </div>
        );
      })}
    </section>
  );
}

/** A name of a setting may break after a dot (never inside a word). */
function dotted(name: string): JSX.Element[] {
  return name.split('.').map((part, i, all) => <>{part}{i < all.length - 1 ? <>.<wbr /></> : null}</>);
}

/** A value of the board that is not a crew setting: a toggle for the booleans, an input for the rest. */
function SettingRow(props: { repo: string; s: StationSetting; reload: () => void }): JSX.Element {
  const { s } = props;
  const [open, setOpen] = useState(false);
  const [val, setVal] = useState(s.value);
  const bool = isBool(s.value);
  const mine = sameEdit(edit.value, s.name, props.repo, false);
  const go = (value: string | null): void => {
    setOpen(false);
    void startEdit({ name: s.name, repo: props.repo, value });
  };
  return (
    <div class="setrow" data-setting={s.name}>
      <div class="sl">
        <code class="nm">{dotted(s.name)}</code>
        <span class="tags">
          <span class="src">{fromLabel(s.from)}</span>
          {s.own && <span class="own" title={t('st.ownTip')}>{t('st.own')}</span>}
        </span>
      </div>
      <div class="setval">
        {bool ? (
          <button type="button" class={`tog ${s.value === 'true' ? 'on' : ''}`} aria-pressed={s.value === 'true'} data-toggle={s.name}
            onClick={() => go(s.value === 'true' ? 'false' : 'true')}>
            {s.value === 'true' ? t('st.on') : t('st.off')}
          </button>
        ) : <b class="v" data-value={s.name}>{s.value}</b>}
        {!bool && <button class="btn sm" type="button" data-act={`st-edit-${s.name}`} aria-expanded={open} onClick={() => { setVal(s.value); setOpen(!open); }}>{t('st.change')}</button>}
        {s.own && <button class="btn sm danger" type="button" data-unset={s.name} onClick={() => go(null)}>{t('st.reset')}</button>}
      </div>
      {s.note && <div class="hint">{s.note}</div>}
      {open && !bool && (
        <form class="sedit" onSubmit={(ev) => { ev.preventDefault(); if (val.trim()) go(val.trim()); }}>
          <input class="field" value={val} aria-label={s.name} onInput={(e) => setVal((e.target as HTMLInputElement).value)} />
          <button class="btn primary sm" type="submit" disabled={!val.trim()}>{t('st.next')}</button>
          <button class="btn sm" type="button" onClick={() => setOpen(false)}>{t('cancel')}</button>
        </form>
      )}
      {mine && <ChangeCard reload={props.reload} />}
    </div>
  );
}

function Board(props: { repo: string; data: StationData; reload: () => void }): JSX.Element {
  const rows = props.data.settings.filter((s) => !ROLE_SETTINGS.includes(s.name));
  const groups: { prefix: string; rows: StationSetting[] }[] = [];
  for (const s of rows) {
    const prefix = s.name.includes('.') ? s.name.slice(0, s.name.indexOf('.')) : s.name;
    let g = groups.find((x) => x.prefix === prefix);
    if (!g) { g = { prefix, rows: [] }; groups.push(g); }
    g.rows.push(s);
  }
  return (
    <section class="st-sec" id="st-board">
      <h3>{t('st.board')}</h3>
      <p class="hint">{t('st.boardHint')}</p>
      {groups.map((g) => (
        <div class="grp" key={g.prefix} data-group={g.prefix}>
          <h4>{t(`st.grp.${g.prefix}`) === `st.grp.${g.prefix}` ? g.prefix : t(`st.grp.${g.prefix}`)}</h4>
          {g.rows.map((s) => <SettingRow key={s.name} repo={props.repo} s={s} reload={props.reload} />)}
        </div>
      ))}
    </section>
  );
}

export function ErrorBlock({ err, onRetry }: { err: CallError; onRetry: () => void }): JSX.Element {
  return (
    <div class="chg bad" id="st-error">
      <div class="hint bad">{err.message}</div>
      <div class="act"><button class="btn sm" type="button" id="st-retry" onClick={onRetry}>{t('st.retry')}</button></div>
    </div>
  );
}

/* ───────── the shipyards' block in this machine's instructions ───────── */

interface RunnerSetting {
  setting: string; connected: boolean; stale: boolean; current: string; block: string;
  next_on: string; next_on_dropped: string; next_off: string; has_old_docker_paragraph: boolean;
}

function Shipyards(): JSX.Element {
  const [rows, setRows] = useState<RunnerSetting[] | null>(null);
  const [err, setErr] = useState('');
  const [drop, setDrop] = useState(false);
  const load = async (): Promise<void> => {
    setErr('');
    try {
      const r = await api<{ settings: RunnerSetting[] }>('/api/runner/connect');
      setRows(r.settings);
    } catch (e) {
      setRows(null);
      setErr((e as Error).message);
    }
  };
  useEffect(() => { void load(); }, []);
  const key = 'sy:home';
  const on = !!rows && rows.some((r) => r.connected);
  // The card previews what the button will do, not what is in place now: «Подключить» writes the block, «Отключить»
  // takes it out. Everything below (the «Станет» text, the drop-old tick, drop_old) keys off the same action.
  const willConnect = !on;
  const stale = !!rows && rows.some((r) => r.stale);
  const go = (): void => {
    confirmStep(key, async () => {
      setErr('');
      try {
        // Dropping the old hand-written docker lines is a separate, explicit choice; never a side effect of connecting.
        await post('/api/runner/connect', { on: willConnect, drop_old: willConnect ? drop : false, confirm: true });
        toast(t('runner.saved'));
        setDrop(false);
        await load();
      } catch (e) {
        setErr((e as Error).message);
      }
    });
  };
  if (err) return <section class="st-sec" id="st-yards"><h3>{t('runner.stationTitle')}</h3><p class="hint bad">{err}</p><button class="btn sm" type="button" onClick={() => void load()}>{t('runner.retry')}</button></section>;
  if (!rows) return <section class="st-sec" id="st-yards"><h3>{t('runner.stationTitle')}</h3><p class="hint">{t('st.loading')}</p></section>;
  const hasYards = rows.some((r) => r.block.length > 0);
  return (
    <section class="st-sec" id="st-yards">
      <h3>{t('runner.stationTitle')}</h3>
      <p class="hint">
        {!hasYards ? t('runner.stationNone') : on ? (stale ? t('runner.stationStale') : t('runner.stationOn')) : t('runner.stationOff')}
      </p>
      {hasYards && (
        <details class="rf-block">
          <summary>{t('runner.stationPreview')}</summary>
          {rows.map((r) => (
            <div key={r.setting} data-setting={r.setting}>
              <div class="hint">{r.setting} · {t('runner.stationWas')}</div>
              <pre>{r.current || t('runner.stationEmpty')}</pre>
              <div class="hint">{t('runner.stationNow')}</div>
              <pre data-next={r.setting}>{(willConnect ? (drop ? r.next_on_dropped : r.next_on) : r.next_off) || t('runner.stationEmpty')}</pre>
            </div>
          ))}
        </details>
      )}
      {hasYards && willConnect && rows.some((r) => r.has_old_docker_paragraph) && (
        <label class="rf-check"><input type="checkbox" id="st-yards-drop" checked={drop} onChange={(e) => setDrop((e.target as HTMLInputElement).checked)} />{t('runner.dropOld')}</label>
      )}
      {hasYards && (
        <div class="act">
          <button class={`btn ${armed.value?.key === key ? 'arm' : ''}`} type="button" id="st-yards-btn" onClick={go}>
            {armed.value?.key === key ? t('runner.saveConfirm') : on ? t('runner.stationOffBtn') : t('runner.stationOnBtn')}
          </button>
        </div>
      )}
    </section>
  );
}

/** What the Station and the crew window both read: the level (for me, or one repository), the station's answer and a reload. */
export function useStationData(): {
  data: StationData | null; err: CallError | null; loading: boolean; level: 'you' | 'repo'; repo: string; scope: string;
  chooseLevel: (l: 'you' | 'repo') => void; setRepo: (r: string) => void; reload: () => void;
} {
  const [data, setData] = useState<StationData | null>(null);
  const [err, setErrState] = useState<CallError | null>(null);
  const [loading, setLoading] = useState(true);
  const [level, setLevel] = useState<'you' | 'repo'>('you');
  const [repo, setRepo] = useState('');
  const [tick, setTick] = useState(0);
  const reload = (): void => setTick((n) => n + 1);

  useEffect(() => () => resetEdit(), []);
  useEffect(() => {
    let alive = true;
    setLoading(true);
    setErrState(null);
    stationGet(level === 'repo' ? repo : '')
      .then((d) => { if (alive) { setData(d); setLoading(false); } })
      .catch((e) => { if (alive) { setErrState(callError(e)); setLoading(false); } });
    return () => { alive = false; };
  }, [level, repo, tick]);

  const chooseLevel = (l: 'you' | 'repo'): void => {
    setLevel(l);
    if (l === 'you') setRepo('');
    else if (!repo) setRepo(data?.repos[0] ?? '');
  };
  return { data, err, loading, level, repo, scope: level === 'repo' ? repo : '', chooseLevel, setRepo, reload };
}

type StationState = ReturnType<typeof useStationData>;

/** «Для меня» / «Для репозитория»: which level the settings below are read and changed at. */
export function LevelBar({ st }: { st: StationState }): JSX.Element {
  const { level, repo, data } = st;
  return (
    <div class="lvl" role="group" aria-label={t('st.levelAria')}>
      <span class="kl">{t('st.level')}</span>
      <button type="button" class={`btn sm ${level === 'you' ? 'on' : ''}`} id="st-level-you" aria-pressed={level === 'you'} onClick={() => st.chooseLevel('you')}>{t('st.levelYou')}</button>
      <button type="button" class={`btn sm ${level === 'repo' ? 'on' : ''}`} id="st-level-repo" aria-pressed={level === 'repo'} onClick={() => st.chooseLevel('repo')}>{t('st.levelRepo')}</button>
      {level === 'repo' && (
        <select class="field sm" id="st-repo" value={repo} aria-label={t('st.levelRepo')} onChange={(e) => st.setRepo((e.target as HTMLSelectElement).value)}>
          {(data?.repos ?? (repo ? [repo] : [])).map((r) => <option key={r} value={r}>{r}</option>)}
        </select>
      )}
      <span class="hint">{level === 'repo' ? t('st.levelRepoHint', { repo: repo || '—' }) : t('st.levelYouHint')}</span>
    </div>
  );
}

function StationBody(): JSX.Element {
  const st = useStationData();
  const { data, err, loading, scope } = st;
  return (
    <div class="bd st-bd">
      <LevelBar st={st} />
      {loading && !data && <div class="hint" id="st-loading">{t('st.loading')}</div>}
      {err && <ErrorBlock err={err} onRetry={st.reload} />}
      {data && (
        <>
          <section class="st-sec" id="st-to-crew">
            <h3>{t('st.crew')}</h3>
            <p class="hint">{t('st.toCrewHint')}</p>
            <div class="act"><button class="btn sm" type="button" id="st-open-crew" onClick={() => { closeStation(); openCrew(); }}>{t('st.openCrew')}</button></div>
          </section>
          <Shipyards />
          <Strays />
          <Board repo={scope} data={data} reload={st.reload} />
        </>
      )}
    </div>
  );
}

/** The Station screen. Opened by `openStation()`. */
export function Station(): JSX.Element | null {
  const d = stationDlg.value;
  if (!d) return null;
  const host = snap.value?.host ?? '';
  const title = t('st.titleHome', { host });
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) closeStation(); }}>
      <section class="panel dlg wide station" id="station" role="dialog" aria-label={title}>
        <header><b>{title}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={closeStation}><Gl n="close" /></button></header>
        <StationBody />
      </section>
    </div>
  );
}

/**
 * The first-run crew window opens by itself once: after the coach tour and the empty galaxy's card
 * (FirstRun, Coach) are gone, and only with no other dialog open, not with ?nocoach or ?calm. Seen = closed.
 */
export function StationAuto(): null {
  const s = snap.value;
  const dlg = crewDlg.value;
  const coach = coachShown(s);
  const first = firstRunShown(s);
  useEffect(() => {
    if (!s || crewSeen() || dlg || coach || first || dialogOpen()) return;
    const q = new URLSearchParams(location.search);
    if (q.has('nocoach') || q.has('calm')) return;
    openCrew(true);
  }, [s, dlg, coach, first]);
  return null;
}
