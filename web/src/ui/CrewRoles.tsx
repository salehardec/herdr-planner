/* The crew's own roles (PLN-90): «Дизайнер», «Ревьюер безопасности», «Документатор». A role is the planner's: a name, an
   agent, an effort, the spares and the instructions. The board has no roles of its own, so a role reaches it only as the
   board's ordinary settings (`set worker.agent … --task <id>`) or as a research task after the task — and only after a
   «было → стало» card and two presses. The rules put a role on a task when it is added or started. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, currentView, tasksIndex } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { AgentPick, ChainEditor, EffortPick } from './CrewPick';
import type { Task } from '../types';
import { callError, StationData } from './station-api';
import { markFrom, markLabel, marksOf } from './crew-roles';
import {
  CrewRole, CrewRule, RoleInput, RolePreview, RoleUsage, RoleWay, RolesData, roleApply, rolePreview, rolesGet, rolesPost,
} from './crew-roles-api';

/* ───────── the data ───────── */

export interface RolesState { data: RolesData | null; error: string; reload: () => void }

/** The own roles and the rules, read from the planner; `reload` asks again. */
export function useRoles(): RolesState {
  const [data, setData] = useState<RolesData | null>(null);
  const [error, setError] = useState('');
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    rolesGet()
      .then((d) => { if (alive) { setData(d); setError(''); } })
      .catch((e) => { if (alive) setError(callError(e).message); });
    return () => { alive = false; };
  }, [tick]);
  return { data, error, reload: () => setTick((n) => n + 1) };
}

const shown = (v: string | null | undefined): string => (v ? v : t('st.empty'));
const clipText = (v: string, n = 90): string => { const one = v.replace(/\s+/g, ' ').trim(); return one.length > n ? `${one.slice(0, n - 1)}…` : one; };

/* ───────── putting a role on a task: the card with «было → стало» ───────── */

const putFor = signal<{ role: string; task: string; how: string; key: string } | null>(null);
const putPrev = signal<RolePreview | null>(null);
const putErr = signal<string>('');
const putting = signal(false);

export const closePut = (): void => { putFor.value = null; putPrev.value = null; putErr.value = ''; };

async function startPut(role: string, task: string, how: string, key: string): Promise<void> {
  const mine = { role, task, how, key };
  putFor.value = mine;
  putPrev.value = null;
  putErr.value = '';
  try {
    const p = await rolePreview({ role, task, how: how || undefined });
    if (putFor.value === mine) putPrev.value = p;
  } catch (e) {
    if (putFor.value === mine) putErr.value = callError(e).message;
  }
}

/** The card: what the board will be told (each command with was → now), the checks, and the two presses. */
function PutCard(props: { place: string; reload: () => void; onDone?: () => void }): JSX.Element | null {
  const f = putFor.value;
  if (!f || f.key !== props.place) return null;
  const p = putPrev.value;
  const key = `rput:${f.role}|${f.task}|${f.how}`;
  const on = armed.value?.key === key;
  const go = (): void => {
    if (!p || !p.valid || putting.value) return;
    confirmStep(key, async () => {
      putting.value = true;
      putErr.value = '';
      try {
        const r = await roleApply({ role: f.role, task: f.task, how: f.how || undefined });
        toast(t('cr.put.done', { role: r.role, task: p.task.tag || p.task.id }));
        closePut();
        props.reload();
        props.onDone?.();
      } catch (e) {
        putErr.value = callError(e).message;
      } finally {
        putting.value = false;
      }
    });
  };
  return (
    <div class={`chg ${(p && !p.valid) || putErr.value ? 'bad' : ''}`} id="cr-put-card" data-put-card={f.role}>
      <div class="chg-h">{t('cr.put.title', { role: p?.role.name ?? '…', task: p?.task.tag || f.task })}{p && <span class="m"> · {t(`cr.way.${p.how}`)}</span>}</div>
      {!p && !putErr.value && <div class="hint">{t('st.checking')}</div>}
      {p && (
        <>
          {p.nothing && <div class="hint" id="cr-put-nothing">{t('cr.put.nothing')}</div>}
          <ol class="pv-steps" id="cr-put-steps">
            {p.steps.map((s) => (
              <li key={s.setting} data-step={s.setting} class={s.error ? 'bad' : ''}>
                <div class="pv-name">{s.kind === 'add' ? t('cr.put.research') : s.setting}</div>
                {s.kind === 'set' && (
                  <div class="ba">
                    <code class="was">{clipText(shown(s.was), 60)}</code>
                    <span class="arr" aria-hidden="true">→</span>
                    <code class="now">{clipText(shown(s.now), 60)}</code>
                  </div>
                )}
                {s.was_from && s.kind === 'set' && <div class="hint">{t('st.wasFrom', { from: s.was_from })}</div>}
                <div class="hint pv-cmd"><code>{s.command}</code></div>
                {s.error && <div class="hint bad" data-step-error>{s.error}</div>}
              </li>
            ))}
          </ol>
          {p.started && p.how !== 'after' && <div class="hint warn" id="cr-put-started">{t('cr.put.started')}</div>}
          {p.duplicate && <div class="hint bad" id="cr-put-dup">{t('cr.put.duplicate')}</div>}
          <div class="hint">{p.how === 'after' ? t('cr.put.afterWhy') : t('st.byBoard')}</div>
        </>
      )}
      {putErr.value && <div class="hint bad" id="cr-put-err">{putErr.value}</div>}
      <div class="act">
        <button class={`btn primary ${on ? 'arm' : ''}`} type="button" id="cr-put-confirm" disabled={!p || !p.valid || p.nothing || putting.value} onClick={go}>
          {on ? t('st.confirm') : t('st.apply')}
        </button>
        <button class="btn" type="button" id="cr-put-cancel" onClick={closePut}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/** The form that picks a task (and the way, for a reviewer) and shows the card. `place` says which card is open. */
export function PutForm(props: { role: CrewRole; place: string; taskId?: string; reload: () => void; onClose?: () => void }): JSX.Element {
  const { role } = props;
  const tasks = currentView.value.tasks.filter((x) => !x.archived);
  const [task, setTask] = useState(props.taskId ?? tasks[0]?.id ?? '');
  const [how, setHow] = useState<string>(role.kind === 'reviewer' ? role.how : 'worker');
  const showCard = (): void => { if (task) void startPut(role.id, task, how, props.place); };
  return (
    <div class="fform put-form" data-put-form={role.id}>
      {!props.taskId && (
        <label class="fl2">{t('cr.put.task')}
          <select class="field" id={`cr-put-task-${role.id}`} value={task} onChange={(e) => { setTask((e.target as HTMLSelectElement).value); closePut(); }}>
            {tasks.map((x) => <option key={x.id} value={x.id}>{x.tag} · {x.title}</option>)}
          </select>
        </label>
      )}
      {role.kind === 'reviewer' && (
        <fieldset class="how" data-how-pick>
          <legend>{t('cr.put.how')}</legend>
          {(['instead', 'after'] as RoleWay[]).map((w) => (
            <label key={w}>
              <input type="radio" name={`cr-how-${role.id}-${props.place}`} value={w} checked={how === w} data-how={w} onChange={() => { setHow(w); closePut(); }} />
              {t(`cr.way.${w}`)} <span class="hint">{t(`cr.way.${w}Hint`)}</span>
            </label>
          ))}
        </fieldset>
      )}
      <div class="act">
        <button class="btn primary sm" type="button" id={`cr-put-show-${role.id}`} disabled={!task} onClick={showCard}>{t('cr.put.show')}</button>
        {props.onClose && <button class="btn sm" type="button" onClick={() => { closePut(); props.onClose?.(); }}>{t('cancel')}</button>}
      </div>
      <PutCard place={props.place} reload={props.reload} onDone={props.onClose} />
    </div>
  );
}

/* ───────── one role's editor ───────── */

const emptyInput = (): RoleInput => ({ name: '', kind: 'worker', agent: '', effort: '', fallback: '', instructions: '', how: 'worker' });

function RoleEditor(props: { role?: CrewRole; programs: StationData['programs']; locked: boolean; onDone: () => void }): JSX.Element {
  const r = props.role;
  const [v, setV] = useState<RoleInput>(r ? { name: r.name, kind: r.kind, agent: r.agent, effort: r.effort, fallback: r.fallback, instructions: r.instructions, how: r.how } : emptyInput());
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const set = (patch: Partial<RoleInput>): void => setV((x) => ({ ...x, ...patch }));
  const id = r?.id ?? 'new';
  const save = async (): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      const body: RoleInput = { ...v, how: v.kind === 'worker' ? 'worker' : v.how === 'after' ? 'after' : 'instead' };
      await rolesPost(r ? { op: 'update', id: r.id, role: body } : { op: 'add', role: body });
      toast(t('st.saved'));
      props.onDone();
    } catch (e) {
      setErr(callError(e).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="pe cr-editor" data-role-editor={id}>
      <label class="fl2">{t('cr.name')}
        <input class="field" id={`cr-name-${id}`} value={v.name} maxLength={40} placeholder={t('cr.namePh')} onInput={(e) => set({ name: (e.target as HTMLInputElement).value })} />
      </label>
      <label class="fl2">{t('cr.kind')}
        <select class="field" id={`cr-kind-${id}`} value={v.kind} disabled={props.locked} onChange={(e) => set({ kind: (e.target as HTMLSelectElement).value as RoleInput['kind'], how: (e.target as HTMLSelectElement).value === 'worker' ? 'worker' : 'instead' })}>
          <option value="worker">{t('cr.kind.worker')}</option>
          <option value="reviewer">{t('cr.kind.reviewer')}</option>
        </select>
      </label>
      <div class="hint">{props.locked ? t('cr.kindLocked') : t(`cr.kind.${v.kind}Hint`)}</div>
      {v.kind === 'reviewer' && (
        <label class="fl2">{t('cr.howDefault')}
          <select class="field" id={`cr-how-${id}`} value={v.how === 'after' ? 'after' : 'instead'} onChange={(e) => set({ how: (e.target as HTMLSelectElement).value })}>
            <option value="instead">{t('cr.way.instead')} — {t('cr.way.insteadHint')}</option>
            <option value="after">{t('cr.way.after')} — {t('cr.way.afterHint')}</option>
          </select>
        </label>
      )}
      <AgentPick programs={props.programs} value={v.agent} onChange={(a) => set({ agent: a, ...(a ? {} : { effort: '' }) })} id={`cr-${id}`} emptyLabel={t('cr.inherit')} />
      <EffortPick agent={v.agent} value={v.effort} onChange={(e) => set({ effort: e })} id={`cr-${id}`} emptyLabel={t('st.effortDefault')} />
      <div class="pe-fb">
        <span class="kl">{t('st.f.fallback')}</span>
        {v.fallback === ''
          ? <><span class="hint">{t('cr.fallbackNone')}</span> <button class="btn sm" type="button" data-cr-fb-set={id} onClick={() => set({ fallback: 'none' })}>{t('st.peSetChain')}</button></>
          : (
            <>
              <ChainEditor programs={props.programs} value={v.fallback} onChange={(f) => set({ fallback: f })} id={`cr-${id}-fb`} agent={v.agent} />
              <button class="btn sm" type="button" data-cr-fb-unset={id} onClick={() => set({ fallback: '' })}>{t('st.peUnsetChain')}</button>
            </>
          )}
      </div>
      <label class="fl2">{t('cr.instructions')}
        <textarea class="field" id={`cr-instr-${id}`} rows={5} maxLength={4000} value={v.instructions} placeholder={t('cr.instructionsPh')} onInput={(e) => set({ instructions: (e.target as HTMLTextAreaElement).value })} />
      </label>
      <div class="hint">{t(v.kind === 'worker' ? 'cr.instructionsWorker' : 'cr.instructionsReviewer')}</div>
      {err && <div class="hint bad" id="cr-edit-err">{err}</div>}
      <div class="act">
        <button class="btn primary sm" type="button" id={`cr-save-${id}`} disabled={busy || !v.name.trim()} onClick={() => void save()}>{t('st.peSave')}</button>
        <button class="btn sm" type="button" id={`cr-cancel-${id}`} onClick={props.onDone}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/* ───────── what uses a role ───────── */

function UsageList(props: { usage: RoleUsage; id: string }): JSX.Element {
  const u = props.usage;
  return (
    <div class="cr-usage" data-usage={props.id} data-tasks={u.tasks.length}>
      {u.tasks.length === 0
        ? <span class="unk" data-usage-none>{t('cr.usage.none')}</span>
        : (
          <>
            <span class="kl">{t('cr.usage.on')}</span>{' '}
            {u.tasks.map((x, i) => <span key={`${x.id}|${x.how}`} class="tg" data-usage-task={x.tag} title={`${x.title} · ${x.status}`}>{i > 0 ? ', ' : ''}{x.tag} <span class="m">{t(`cr.way.${x.how}`)}</span></span>)}
          </>
        )}
      {(u.rules > 0 || u.presets.length > 0) && (
        <span class="m"> · {u.rules > 0 && tn('cr.usage.rules', u.rules)}{u.rules > 0 && u.presets.length > 0 && ', '}{u.presets.length > 0 && t('cr.usage.presets', { list: u.presets.join(', ') })}</span>
      )}
    </div>
  );
}

/** What the role sets on the board, by way: the setting and its value. */
function SetsList(props: { role: CrewRole }): JSX.Element {
  const ways = (Object.keys(props.role.sets) as RoleWay[]).filter((w) => (props.role.sets[w] ?? []).length > 0 || props.role.kind === 'reviewer');
  return (
    <details class="cr-sets" data-sets={props.role.id}>
      <summary>{t('cr.sets')}</summary>
      {ways.map((w) => (
        <div key={w} class="cr-way" data-way={w}>
          <div class="hint"><b>{t(`cr.way.${w}`)}</b> — {t(`cr.way.${w}Cmd`)}</div>
          {(props.role.sets[w] ?? []).length === 0
            ? <div class="hint">{t('cr.sets.empty')}</div>
            : <ul class="sets">{(props.role.sets[w] ?? []).map((s) => <li key={s.setting} data-set={s.setting}><code>{s.setting}</code> = <span>{clipText(s.value, 80)}</span></li>)}</ul>}
        </div>
      ))}
    </details>
  );
}

function brief(r: CrewRole): string {
  const parts = [r.agent || t('cr.inherit'), r.effort && `${t('st.f.effort')}: ${r.effort}`, r.fallback && `${t('st.f.fallback')}: ${r.fallback}`].filter(Boolean);
  return parts.join(' · ');
}

/* ───────── the roles ───────── */

type Panel = '' | 'edit' | 'rename' | 'put' | 'delete';

function RoleCard(props: { role: CrewRole; programs: StationData['programs']; reload: () => void }): JSX.Element {
  const { role } = props;
  const [panel, setPanel] = useState<Panel>('');
  const [name, setName] = useState(role.name);
  const [err, setErr] = useState('');
  const [inUse, setInUse] = useState<RoleUsage | null>(null);
  const [busy, setBusy] = useState(false);
  const place = `role:${role.id}`;
  const open = (p: Panel): void => { closePut(); setErr(''); setInUse(null); setPanel(panel === p ? '' : p); };
  const rename = async (): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      await rolesPost({ op: 'update', id: role.id, role: { name, kind: role.kind, agent: role.agent, effort: role.effort, fallback: role.fallback, instructions: role.instructions, how: role.how } });
      setPanel('');
      props.reload();
    } catch (e) {
      setErr(callError(e).message);
    } finally {
      setBusy(false);
    }
  };
  const drop = async (force: boolean): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      await rolesPost({ op: 'delete', id: role.id, force });
      toast(t('cr.deleted'));
      props.reload();
    } catch (e) {
      const c = callError(e);
      const usage = (e as { data?: { usage?: RoleUsage } }).data?.usage;
      if (c.status === 409 && usage) { setInUse(usage); setPanel('delete'); } else setErr(c.message);
    } finally {
      setBusy(false);
    }
  };
  const del = (): void => {
    confirmStep(`rdel:${role.id}`, async () => { await drop(false); });
  };
  return (
    <li class="cr-role" data-role-id={role.id} data-role-name={role.name} data-kind={role.kind}>
      <div class="pr-row">
        <b class="nm">{role.name}</b>
        <span class="chip">{t(`cr.kind.${role.kind}`)}</span>
        {role.kind === 'reviewer' && <span class="chip" data-role-how={role.how}>{t(`cr.way.${role.how}`)}</span>}
        <span class="grow" />
        <button class="btn sm primary" type="button" data-role-put={role.id} aria-expanded={panel === 'put'} onClick={() => open('put')}>{t('cr.put.open')}</button>
        <button class="btn sm" type="button" data-role-edit={role.id} aria-expanded={panel === 'edit'} onClick={() => open('edit')}>{t('st.change')}</button>
        <button class="btn sm" type="button" data-role-rename={role.id} onClick={() => { setName(role.name); open('rename'); }}>{t('st.pr.rename')}</button>
        <button class={`btn sm danger ${armed.value?.key === `rdel:${role.id}` ? 'arm' : ''}`} type="button" data-role-del={role.id} disabled={busy} onClick={del}>
          {armed.value?.key === `rdel:${role.id}` ? t('st.confirm') : t('st.pr.delete')}
        </button>
      </div>
      <div class="hint pr-brief" data-role-brief>{brief(role)}</div>
      {role.instructions && <div class="hint cr-instr" data-role-instr>{t('cr.instructions')}: {clipText(role.instructions, 140)}</div>}
      <SetsList role={role} />
      <UsageList usage={role.usage} id={role.id} />
      {panel === 'rename' && (
        <form class="sedit" onSubmit={(ev) => { ev.preventDefault(); void rename(); }}>
          <input class="field" id={`cr-rename-${role.id}`} value={name} maxLength={40} aria-label={t('cr.name')} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
          <button class="btn primary sm" type="submit" disabled={busy || !name.trim() || name.trim() === role.name}>{t('st.pr.rename')}</button>
          <button class="btn sm" type="button" onClick={() => setPanel('')}>{t('cancel')}</button>
        </form>
      )}
      {panel === 'edit' && <RoleEditor role={role} programs={props.programs} locked={role.usage.tasks.length > 0 || role.usage.rules > 0} onDone={() => { setPanel(''); props.reload(); }} />}
      {panel === 'put' && <PutForm role={role} place={place} reload={props.reload} onClose={() => setPanel('')} />}
      {panel === 'delete' && inUse && (
        <div class="chg bad" id="cr-del-card" data-del-card={role.id}>
          <div class="chg-h">{t('cr.del.title', { name: role.name })}</div>
          <div class="hint">{t('cr.del.why')}</div>
          <UsageList usage={inUse} id={`${role.id}-del`} />
          <div class="hint">{t('cr.del.keeps')}</div>
          <div class="act">
            <button class="btn danger" type="button" id="cr-del-force" disabled={busy} onClick={() => void drop(true)}>{t('cr.del.force')}</button>
            <button class="btn" type="button" id="cr-del-cancel" onClick={() => { setPanel(''); setInUse(null); }}>{t('cancel')}</button>
          </div>
        </div>
      )}
      {err && <div class="hint bad" data-role-err>{err}</div>}
    </li>
  );
}

/** «Свои роли»: the list, the suggested ones that are not on, and the form for a new one. */
export function OwnRoles(props: { programs: StationData['programs']; roles: RolesState; reload: () => void }): JSX.Element {
  const { data, error } = props.roles;
  const [adding, setAdding] = useState(false);
  const [err, setErr] = useState('');
  const done = (): void => { props.roles.reload(); props.reload(); };
  const enable = async (key: string): Promise<void> => {
    setErr('');
    try {
      await rolesPost({ op: 'suggest', key });
      toast(t('st.saved'));
      done();
    } catch (e) {
      setErr(callError(e).message);
    }
  };
  return (
    <section class="st-sec" id="st-roles">
      <h3>{t('cr.title')}</h3>
      <p class="hint">{t('cr.hint')}</p>
      {error && <div class="hint bad" id="cr-load-err">{error}</div>}
      {!data && !error && <div class="hint">{t('st.loading')}</div>}
      {data && data.suggested.length > 0 && (
        <div class="cr-suggest" id="cr-suggested">
          <div class="kl">{t('cr.suggested')}</div>
          {data.suggested.map((s) => (
            <div class="cr-sug" key={s.key} data-suggested={s.key}>
              <div class="pr-row">
                <b class="nm">{s.name}</b><span class="chip">{t(`cr.kind.${s.kind}`)}</span><span class="chip">{t(`cr.way.${s.how}`)}</span>
                <span class="grow" />
                <button class="btn sm primary" type="button" data-suggest-on={s.key} onClick={() => void enable(s.key)}>{t('cr.enable')}</button>
              </div>
              <div class="hint">{clipText(s.instructions, 220)}</div>
            </div>
          ))}
        </div>
      )}
      {data && data.roles.length === 0 && <div class="hint" id="cr-empty">{t('cr.empty')}</div>}
      {data && (
        <ul class="presets cr-roles" id="cr-list">
          {data.roles.map((r) => <RoleCard key={r.id} role={r} programs={props.programs} reload={done} />)}
        </ul>
      )}
      {adding
        ? <RoleEditor programs={props.programs} locked={false} onDone={() => { setAdding(false); done(); }} />
        : <div class="act"><button class="btn primary sm" type="button" id="cr-add" onClick={() => setAdding(true)}>{t('cr.add')}</button></div>}
      {err && <div class="hint bad" id="cr-err">{err}</div>}
    </section>
  );
}

/* ───────── the rules ───────── */

function RuleRow(props: { rule: CrewRule; role: CrewRole | undefined; first: boolean; reload: () => void }): JSX.Element {
  const { rule } = props;
  const [err, setErr] = useState('');
  const call = async (op: 'rule_up' | 'rule_delete'): Promise<void> => {
    setErr('');
    try {
      await rolesPost({ op, id: rule.id });
      props.reload();
    } catch (e) {
      setErr(callError(e).message);
    }
  };
  return (
    <li class="cr-rule" data-rule-id={rule.id} data-rule-role={props.role?.name ?? ''}>
      <span class="cr-if">{t('cr.rules.if')}</span> <span class="cr-words">{rule.words}</span>
      <span class="arr" aria-hidden="true">→</span>
      <b>{props.role?.name ?? t('cr.rules.gone')}</b>
      {rule.how && <span class="chip">{t(`cr.way.${rule.how}`)}</span>}
      <span class="grow" />
      <button class="btn sm" type="button" data-rule-up={rule.id} disabled={props.first} aria-label={t('st.chainUp')} onClick={() => void call('rule_up')}>↑</button>
      <button class="btn sm danger" type="button" data-rule-del={rule.id} onClick={() => void call('rule_delete')}>{t('st.pr.delete')}</button>
      {err && <div class="hint bad">{err}</div>}
    </li>
  );
}

/** «Правила»: for the repository X, or if the title or the branch is like …, a task gets the role. The first that fits wins. */
export function Rules(props: { roles: RolesState; reload: () => void }): JSX.Element {
  const { data } = props.roles;
  const [role, setRole] = useState('');
  const [repo, setRepo] = useState('');
  const [title, setTitle] = useState('');
  const [branch, setBranch] = useState('');
  const [how, setHow] = useState('');
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const roles = data?.roles ?? [];
  const chosen = roles.find((r) => r.id === (role || roles[0]?.id));
  useEffect(() => { setHow(''); }, [chosen?.id]);
  const done = (): void => { props.roles.reload(); props.reload(); };
  const add = async (): Promise<void> => {
    if (!chosen) return;
    setBusy(true);
    setErr('');
    try {
      await rolesPost({ op: 'rule_add', rule: { role: chosen.id, repo, title, branch, how } });
      toast(t('st.saved'));
      setRepo(''); setTitle(''); setBranch(''); setHow('');
      done();
    } catch (e) {
      setErr(callError(e).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <section class="st-sec" id="st-rules">
      <h3>{t('cr.rules.title')}</h3>
      <p class="hint">{t('cr.rules.hint')}</p>
      {data && data.rules.length === 0 && <div class="hint" id="cr-rules-empty">{t('cr.rules.empty')}</div>}
      {data && data.rules.length > 0 && (
        <ol class="cr-rules" id="cr-rules-list">
          {data.rules.map((r, i) => <RuleRow key={r.id} rule={r} role={roles.find((x) => x.id === r.role)} first={i === 0} reload={done} />)}
        </ol>
      )}
      {roles.length > 0 && (
        <div class="pr-new cr-rule-new">
          <div class="hint">{t('cr.rules.new')}</div>
          <label class="fl2">{t('cr.rules.role')}
            <select class="field" id="cr-rule-role" value={chosen?.id ?? ''} onChange={(e) => setRole((e.target as HTMLSelectElement).value)}>
              {roles.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
            </select>
          </label>
          <label class="fl2">{t('cr.rules.repo')}
            <select class="field" id="cr-rule-repo" value={repo} onChange={(e) => setRepo((e.target as HTMLSelectElement).value)}>
              <option value="">{t('cr.rules.anyRepo')}</option>
              {(data?.repos ?? []).map((r) => <option key={r} value={r}>{r}</option>)}
            </select>
          </label>
          <label class="fl2">{t('cr.rules.titleHas')}
            <input class="field" id="cr-rule-title" value={title} maxLength={80} placeholder={t('cr.rules.titlePh')} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
          </label>
          <label class="fl2">{t('cr.rules.branchHas')}
            <input class="field" id="cr-rule-branch" value={branch} maxLength={80} placeholder={t('cr.rules.branchPh')} onInput={(e) => setBranch((e.target as HTMLInputElement).value)} />
          </label>
          {chosen?.kind === 'reviewer' && (
            <label class="fl2">{t('cr.put.how')}
              <select class="field" id="cr-rule-how" value={how} onChange={(e) => setHow((e.target as HTMLSelectElement).value)}>
                <option value="">{t('cr.rules.howOwn', { way: t(`cr.way.${chosen.how}`) })}</option>
                <option value="instead">{t('cr.way.instead')}</option>
                <option value="after">{t('cr.way.after')}</option>
              </select>
            </label>
          )}
          <div class="hint">{t('cr.rules.mask')}</div>
          {err && <div class="hint bad" id="cr-rule-err">{err}</div>}
          <div class="act"><button class="btn primary sm" type="button" id="cr-rule-add" disabled={busy || !chosen || (!repo && !title.trim() && !branch.trim())} onClick={() => void add()}>{t('cr.rules.add')}</button></div>
        </div>
      )}
      {data && roles.length === 0 && <div class="hint">{t('cr.rules.needRole')}</div>}
    </section>
  );
}

/* ───────── on a task ───────── */

/** The task's panel: the roles of the crew's own on it (and where each came from), and a way to put another one on. */
export function TaskRoles({ task }: { task: Task }): JSX.Element {
  const roles = useRoles();
  const marks = marksOf(task);
  const [open, setOpen] = useState(false);
  const [pick, setPick] = useState('');
  const list = roles.data?.roles ?? [];
  const chosen = list.find((r) => r.id === (pick || list[0]?.id));
  const tagOf = (id: string): string => tasksIndex.value.get(id)?.tag ?? id;
  useEffect(() => () => closePut(), [task.id]);
  return (
    <section class="cr-task" id="cr-task" data-task-roles={marks.length}>
      <h3>{t('cr.task.title')}</h3>
      {marks.length === 0 && <div class="hint" id="cr-task-none">{t('cr.task.none')}</div>}
      {marks.map((m) => (
        <div class="row" key={`${m.role}|${m.how}`} data-task-role={m.name} data-task-role-how={m.how} data-task-role-by={m.by}>
          <b>{markLabel(m, tagOf)}</b>
          <span class="hint">{markFrom(m)}</span>
        </div>
      ))}
      {list.length > 0 && !open && (
        <div class="act"><button class="btn sm" type="button" id="cr-task-open" onClick={() => setOpen(true)}>{t('cr.task.put')}</button></div>
      )}
      {open && chosen && (
        <>
          <label class="fl2">{t('cr.rules.role')}
            <select class="field" id="cr-task-role" value={chosen.id} onChange={(e) => { setPick((e.target as HTMLSelectElement).value); closePut(); }}>
              {list.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
            </select>
          </label>
          <PutForm key={chosen.id} role={chosen} place={`task:${task.id}`} taskId={task.id} reload={() => { roles.reload(); }} onClose={() => setOpen(false)} />
        </>
      )}
      {list.length === 0 && roles.data && <div class="hint">{t('cr.task.noRoles')}</div>}
    </section>
  );
}
