/* The Station («Станция»): the crew (who runs the Coordinator, the planner, the reviewer, the workers,
   the light tasks and the spares) and the settings of the board. Every change goes the same way: a preview
   («было → стало», the tasks it touches), then two presses (confirmStep), then the board's own command.
   The first-run wizard («Экипаж корабля») is this same screen in wizard mode. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, snap } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { api, post } from '../api';
import { dialogOpen } from '../dialogs';
import { Gl } from './Gl';
import { Strays } from './Strays';
import { AgentPick, ChainEditor, programMark, useModels } from './CrewPick';
import { CrewSummary, Programs } from './CrewPrograms';
import { Presets } from './CrewPresets';
import { chainOf, chainValue } from './crew-view';
import { coachShown, firstRunShown } from './FirstRun';
import {
  callError, CallError, closeStation, failoverBack, failoverSet, FailoverRole, crewSeen, fromLabel, isBool, openStation, PROGRAM_ORDER, programOf,
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
  /** The role whose spares the planner keeps (`coordinator`, `plan`, `light`): the change is to its chain. */
  failover?: string;
}

const edit = signal<Edit | null>(null);
const prev = signal<Preview | null>(null);
const prevErr = signal<CallError | null>(null);
const setErr = signal<CallError | null>(null);
const saving = signal(false);

const resetEdit = (): void => {
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
    /* The board has no such setting: only its check of the chain is taken, the rest is the planner's own. */
    if (e.failover) Object.assign(p, { was: lightWas, was_from: '', affects: [], note: '' });
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
      else if (e.failover) await failoverSet({ role: e.failover, chain: e.value ?? 'none' });
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
      <div class="chg-h">{t('st.changeOf')} <code>{e.light ? t('st.lightName') : e.failover ? t('st.failName', { role: t(`st.r.${e.failover}`) }) : e.name}</code>{e.repo && <span class="m"> · {e.repo}</span>}</div>
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
          {p.was_from && !e.light && !e.failover && <div class="hint">{t('st.wasFrom', { from: fromLabel(p.was_from) })}</div>}
          {!p.valid && <div class="hint bad" id="st-invalid">{p.error}</div>}
          {e.light
            ? <div class="hint" id="st-affects">{t('st.lightAffects')}</div>
            : e.failover
              ? <div class="hint" id="st-affects">{t('st.failAffects')}</div>
              : <div class="hint" id="st-affects">{p.affects.length ? tn('st.affects', p.affects.length) : t('st.affectsNone')}</div>}
          {p.affects.length > 0 && (
            <ul class="aff">
              {p.affects.slice(0, 8).map((a) => <li key={a.id}><span class="tg">{a.id}</span> {a.title} <span class="m">{a.status}</span></li>)}
              {p.affects.length > 8 && <li class="m">{t('st.andMore', { n: p.affects.length - 8 })}</li>}
            </ul>
          )}
          {p.note && <div class="hint">{p.note}</div>}
          <div class="hint">{e.light ? t('st.lightWhere') : e.failover ? t('st.failWhere') : t('st.byBoard')}</div>
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
  programs: StationProgram[]; agent: string; reload: () => void; suggest?: string; failover?: string;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const light = props.kind === 'light';
  const e = edit.value;
  const mine = sameEdit(e, props.name, props.repo, light);
  const start = (value: string): void => {
    setOpen(false);
    void startEdit({ name: props.name, repo: props.repo, value, light, failover: props.failover }, props.value);
  };
  return (
    <div class="fld" data-field={props.kind}>
      <div class="fl">
        <span class="kl">{props.label}</span>
        <b class="v" data-value={props.name}>{shown(props.value)}</b>
        {props.from && !light && <span class="src">{fromLabel(props.from)}</span>}
        <button class="btn sm" type="button" data-act={`st-edit-${props.name}`} aria-expanded={open} onClick={() => setOpen(!open)}>{t('st.change')}</button>
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
  { id: 'light', title: 'st.r.light', hint: 'st.r.lightHint', light: true },
];

/** The roles whose spares the planner keeps (the board has `fallback` for the Reviewer and the heavy tasks only). */
const FAILOVER: string[] = ['coordinator', 'plan', 'light'];

/** The first program of the order that is installed and signed in (or needs no sign-in), and a model of it for each role. */
async function suggestCrew(data: StationData): Promise<Record<string, string>> {
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

/** What the planner does with a role's spares: the «переключать само» mark, the switch it has made, the return. */
function Spares(props: { role: string; f: FailoverRole; reload: () => void }): JSX.Element | null {
  const { role, f } = props;
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState('');
  if (!f.chain && !f.switched) return null;
  const run = async (job: () => Promise<unknown>, done: string): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      await job();
      toast(done);
      props.reload();
    } catch (e) {
      setErr(callError(e).message);
    } finally {
      setBusy(false);
    }
  };
  const sw = f.switched;
  return (
    <div class="fld spares" data-field="spares" data-spares={role}>
      {f.chain && (
        <>
          <label class="chk">
            <input type="checkbox" id={`st-auto-${role}`} checked={f.auto} disabled={busy}
              onChange={(e) => void run(() => failoverSet({ role, auto: (e.target as HTMLInputElement).checked }), t('st.saved'))} />
            <span>{t('st.failAuto')}</span>
          </label>
          <div class="hint">{f.auto ? t(`st.failAutoOn.${role}`) : t('st.failAutoOff')}</div>
        </>
      )}
      {role === 'light' && f.use && <div class="hint" id="st-fail-use">{t('st.failLightUse', { agent: f.use })}</div>}
      {sw && (
        <div class="failnow" id={`st-fail-now-${role}`}>
          <div>{t('st.failNow', { to: sw.to, home: sw.home, words: sw.words })}</div>
          {sw.back_ready
            ? (
              <div class="act">
                <span class="hint">{t('st.failBackReady', { home: sw.home })}</span>
                <button class="btn sm primary" type="button" id={`st-fail-back-${role}`} disabled={busy}
                  onClick={() => void run(() => failoverBack(role, 'back'), t('st.failBackDone', { home: sw.home }))}>{t('st.failBack')}</button>
                <button class="btn sm" type="button" id={`st-fail-keep-${role}`} disabled={busy}
                  onClick={() => void run(() => failoverBack(role, 'keep'), t('st.saved'))}>{t('st.failKeep')}</button>
              </div>
            )
            : <div class="hint">{t('st.failBackWait', { home: sw.home })}</div>}
        </div>
      )}
      {err && <div class="hint bad">{err}</div>}
    </div>
  );
}

function Crew(props: { repo: string; data: StationData; reload: () => void; wizard: boolean; suggest: Record<string, string> }): JSX.Element {
  const { data } = props;
  const val = (name: string): StationSetting | undefined => data.settings.find((s) => s.name === name);
  return (
    <section class="st-sec" id="st-crew">
      <h3>{t('st.crew')}</h3>
      <p class="hint">{props.wizard ? t('st.crewWizHint') : t('st.crewHint')}</p>
      {ROLES.map((r) => {
        const agent = r.agent ? val(r.agent) : undefined;
        const agentValue = r.light ? data.light : agent?.value ?? '';
        const common = { repo: props.repo, programs: data.programs, agent: agentValue, reload: props.reload };
        return (
          <div class="role" data-role={r.id} key={r.id}>
            <div class="rh"><b>{t(r.title)}</b><span class="hint">{t(r.hint)}</span></div>
            {r.agent && (
              <Field {...common} kind="agent" name={r.agent} label={t('st.f.agent')} value={agentValue} from={agent?.from ?? ''}
                suggest={props.suggest[r.agent]} />
            )}
            {r.effort && (
              <Field {...common} kind="effort" name={r.effort} label={t('st.f.effort')} value={val(r.effort)?.value ?? ''} from={val(r.effort)?.from ?? ''} />
            )}
            {r.fallback && (
              <Field {...common} kind="fallback" name={r.fallback} label={t('st.f.fallback')} value={val(r.fallback)?.value ?? ''} from={val(r.fallback)?.from ?? ''} />
            )}
            {r.light && (
              <Field {...common} kind="light" name="light" label={t('st.f.light')} value={data.light} from="" suggest={props.suggest.light} />
            )}
            {FAILOVER.includes(r.id) && props.repo && <div class="hint" data-field="fallback-global">{t('st.failGlobal')}</div>}
            {FAILOVER.includes(r.id) && !props.repo && (
              <>
                <Field {...common} kind="fallback" name={`${r.id}.fallback`} label={t('st.f.fallback')} value={data.failover?.[r.id]?.chain ?? ''} from="" failover={r.id} />
                {data.failover?.[r.id] && <Spares role={r.id} f={data.failover[r.id]} reload={props.reload} />}
              </>
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

function ErrorBlock({ err, onRetry }: { err: CallError; onRetry: () => void }): JSX.Element {
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

function StationBody(props: { wizard: boolean }): JSX.Element {
  const { wizard } = props;
  const [data, setData] = useState<StationData | null>(null);
  const [err, setErrState] = useState<CallError | null>(null);
  const [loading, setLoading] = useState(true);
  const [level, setLevel] = useState<'you' | 'repo'>('you');
  const [repo, setRepo] = useState('');
  const [tick, setTick] = useState(0);
  const [suggest, setSuggest] = useState<Record<string, string>>({});
  const [sugDone, setSugDone] = useState(false);
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
  useEffect(() => {
    if (!wizard || !data) return;
    let alive = true;
    suggestCrew(data)
      .then((m) => { if (alive) { setSuggest(m); setSugDone(true); } })
      .catch(() => { if (alive) setSugDone(true); });
    return () => { alive = false; };
  }, [wizard, data]);

  const chooseLevel = (l: 'you' | 'repo'): void => {
    setLevel(l);
    if (l === 'you') setRepo('');
    else if (!repo) setRepo(data?.repos[0] ?? '');
  };

  return (
    <div class="bd st-bd">
      {wizard && <p class="hint" id="st-intro">{t('st.wizIntro')}</p>}
      {!wizard && (
        <div class="lvl" role="group" aria-label={t('st.levelAria')}>
          <span class="kl">{t('st.level')}</span>
          <button type="button" class={`btn sm ${level === 'you' ? 'on' : ''}`} id="st-level-you" aria-pressed={level === 'you'} onClick={() => chooseLevel('you')}>{t('st.levelYou')}</button>
          <button type="button" class={`btn sm ${level === 'repo' ? 'on' : ''}`} id="st-level-repo" aria-pressed={level === 'repo'} onClick={() => chooseLevel('repo')}>{t('st.levelRepo')}</button>
          {level === 'repo' && (
            <select class="field sm" id="st-repo" value={repo} aria-label={t('st.levelRepo')} onChange={(e) => setRepo((e.target as HTMLSelectElement).value)}>
              {(data?.repos ?? (repo ? [repo] : [])).map((r) => <option key={r} value={r}>{r}</option>)}
            </select>
          )}
          <span class="hint">{level === 'repo' ? t('st.levelRepoHint', { repo: repo || '—' }) : t('st.levelYouHint')}</span>
        </div>
      )}
      {loading && !data && <div class="hint" id="st-loading">{t('st.loading')}</div>}
      {err && <ErrorBlock err={err} onRetry={reload} />}
      {data && (
        <>
          <CrewSummary data={data} />
          <Programs programs={data.programs} />
          {!wizard && <Presets data={data} repo={level === 'repo' ? repo : ''} reload={reload} />}
          <Crew repo={level === 'repo' ? repo : ''} data={data} reload={reload} wizard={wizard} suggest={suggest} />
          {wizard && sugDone && Object.keys(suggest).length === 0 && <div class="hint bad">{t('st.wizNone')}</div>}
          {wizard && (
            <div class="act">
              <button class="btn primary" type="button" id="st-done" onClick={closeStation}>{t('st.done')}</button>
              <button class="btn" type="button" id="st-skip" onClick={closeStation}>{t('st.skip')}</button>
              <button class="btn" type="button" id="st-all" onClick={() => openStation(false)}>{t('st.allSettings')}</button>
            </div>
          )}
          {!wizard && <Shipyards />}
          {!wizard && <Strays />}
          {!wizard && <Board repo={level === 'repo' ? repo : ''} data={data} reload={reload} />}
        </>
      )}
    </div>
  );
}

/** The Station screen. Opened by `openStation(wizard?)`. */
export function Station(): JSX.Element | null {
  const d = stationDlg.value;
  if (!d) return null;
  const host = snap.value?.host ?? '';
  const title = d.wizard ? t('st.wizTitle') : t('st.titleHome', { host });
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) closeStation(); }}>
      <section class={`panel dlg wide station ${d.wizard ? 'wizard' : ''}`} id="station" role="dialog" aria-label={title}>
        <header><b>{title}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={closeStation}><Gl n="close" /></button></header>
        <StationBody key={d.wizard ? 'w' : 'n'} wizard={!!d.wizard} />
      </section>
    </div>
  );
}

/**
 * The first-run wizard opens by itself once: after the coach tour and the empty galaxy's card
 * (FirstRun, Coach) are gone, and only with no other dialog open, not with ?nocoach or ?calm. Seen = closed.
 */
export function StationAuto(): null {
  const s = snap.value;
  const dlg = stationDlg.value;
  const coach = coachShown(s);
  const first = firstRunShown(s);
  useEffect(() => {
    if (!s || crewSeen() || dlg || coach || first || dialogOpen()) return;
    const q = new URLSearchParams(location.search);
    if (q.has('nocoach') || q.has('calm')) return;
    openStation(true);
  }, [s, dlg, coach, first]);
  return null;
}

