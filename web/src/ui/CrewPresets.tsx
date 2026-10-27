/* Crew presets («Наборы экипажа»): a named set that gives every role its agent, effort and fallback. The planner keeps
   them. Applying one shows the whole «было → стало» over all roles and the tasks it touches, and only after two presses
   sends the board its own `set` and `unset` commands, in the order shown. Which one is active, and whether the crew
   differs from it, is on every row. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { AgentPick, ChainEditor, EffortPick } from './CrewPick';
import { ROLE_IDS, ROLE_SETTING, RoleId, rolesOrEmpty, sameRoles } from './crew-view';
import {
  callError, CallError, PresetItem, PresetPreview, PresetRole, presetApply, presetPreview, presetsPost, StationData,
} from './station-api';

/* ───────── the apply card (one at a time) ───────── */

const applyFor = signal<{ id: string; repo: string } | null>(null);
const applyPrev = signal<PresetPreview | null>(null);
const applyErr = signal<CallError | null>(null);
const applying = signal(false);

const closeApply = (): void => {
  applyFor.value = null;
  applyPrev.value = null;
  applyErr.value = null;
};

/** Opens the card and reads the preview: every step that would run, the checks, and the tasks it touches. */
async function startApply(id: string, repo: string): Promise<void> {
  const key = { id, repo };
  applyFor.value = key;
  applyPrev.value = null;
  applyErr.value = null;
  try {
    const p = await presetPreview(id, repo);
    if (applyFor.value === key) applyPrev.value = p;
  } catch (e) {
    if (applyFor.value === key) applyErr.value = callError(e);
  }
}

const fieldTitle = (role: string, field: string): string => `${t(`st.r.${role}`)} · ${t(`st.f.${field}`)}`;
const shown = (v: string | null | undefined): string => (v ? v : t('st.empty'));

function ApplyCard(props: { repo: string; reload: () => void }): JSX.Element | null {
  const f = applyFor.value;
  if (!f) return null;
  const p = applyPrev.value;
  const err = applyErr.value;
  const key = `pv:${f.id}|${f.repo}`;
  const on = armed.value?.key === key;
  const go = (): void => {
    if (!p || !p.valid || applying.value) return;
    confirmStep(key, async () => {
      applying.value = true;
      applyErr.value = null;
      try {
        const r = await presetApply(f.id, f.repo);
        toast(t('st.pv.done', { name: r.preset }));
        closeApply();
        props.reload();
      } catch (e) {
        applyErr.value = callError(e);
        props.reload();
        void presetPreview(f.id, f.repo).then((np) => { if (applyFor.value === f) applyPrev.value = np; }).catch(() => undefined);
      } finally {
        applying.value = false;
      }
    });
  };
  const running = p?.affects.running ?? [];
  const waiting = p?.affects.waiting ?? [];
  return (
    <div class={`chg ${(p && !p.valid) || err ? 'bad' : ''}`} id="st-preset-card" data-preset-card={f.id}>
      <div class="chg-h">{t('st.pv.title', { name: p?.preset.name ?? '…' })}{f.repo && <span class="m"> · {f.repo}</span>}</div>
      {!p && !err && <div class="hint">{t('st.checking')}</div>}
      {p && (
        <>
          {p.steps.length === 0
            ? <div class="hint" id="st-pv-nothing">{t('st.pv.nothing')}</div>
            : (
              <ol class="pv-steps" id="st-pv-steps">
                {p.steps.map((s) => (
                  <li key={s.setting} data-step={s.setting} class={s.error ? 'bad' : ''}>
                    <div class="pv-name">{fieldTitle(s.role, s.field)}</div>
                    <div class="ba">
                      <code class="was">{shown(s.was)}</code>
                      <span class="arr" aria-hidden="true">→</span>
                      <code class="now">{s.now === null ? t('st.pv.unset') : shown(s.now)}</code>
                    </div>
                    <div class="hint pv-cmd">{s.command ? <code>{s.command}</code> : t('st.pv.byPlanner')}</div>
                    {s.error && <div class="hint bad" data-step-error>{s.error}</div>}
                  </li>
                ))}
              </ol>
            )}
          {p.light_skipped && <div class="hint" id="st-pv-light">{t('st.pv.lightSkipped')}</div>}
          {p.loading.length > 0 && <div class="hint" id="st-pv-loading">{t('st.pv.loading', { list: p.loading.join(', ') })}</div>}
          <div class="pv-affects" id="st-pv-affects" data-running={running.length} data-waiting={waiting.length}>
            {running.length + waiting.length === 0 && <div class="hint">{t('st.pv.noTasks')}</div>}
            {running.length > 0 && (
              <div class="hint warn" data-affects="running">
                <b>{tn('st.pv.running', running.length)}</b> {t('st.pv.runningWhy')}
                <ul class="aff">{running.slice(0, 6).map((a) => <li key={a.id}><span class="tg">{a.tag ?? a.id}</span> {a.title} <span class="m">{a.app}</span></li>)}{running.length > 6 && <li class="m">{t('st.andMore', { n: running.length - 6 })}</li>}</ul>
              </div>
            )}
            {waiting.length > 0 && (
              <div class="hint" data-affects="waiting">
                <b>{tn('st.pv.waiting', waiting.length)}</b> {t('st.pv.waitingWhy')}
                <ul class="aff">{waiting.slice(0, 6).map((a) => <li key={a.id}><span class="tg">{a.tag ?? a.id}</span> {a.title}</li>)}{waiting.length > 6 && <li class="m">{t('st.andMore', { n: waiting.length - 6 })}</li>}</ul>
              </div>
            )}
            {running.length + waiting.length > 0 && <div class="hint">{t('st.pv.own')}</div>}
          </div>
        </>
      )}
      {err && <div class="hint bad" id="st-pv-err">{err.message}</div>}
      <div class="act">
        <button class={`btn primary ${on ? 'arm' : ''}`} type="button" id="st-pv-confirm" disabled={!p || !p.valid || applying.value} onClick={go}>
          {on ? t('st.confirm') : t('st.apply')}
        </button>
        <button class="btn" type="button" id="st-pv-cancel" onClick={closeApply}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/* ───────── the editor of one preset ───────── */

function PresetEditor(props: { preset: PresetItem; programs: StationData['programs']; onDone: () => void }): JSX.Element {
  const { preset, programs } = props;
  const saved = rolesOrEmpty(preset.roles);
  const [roles, setRoles] = useState<Record<string, PresetRole>>(saved);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const set = (role: string, patch: Partial<PresetRole>): void => setRoles((r) => ({ ...r, [role]: { ...r[role], ...patch } }));
  const save = async (): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      await presetsPost({ op: 'update', id: preset.id, roles });
      toast(t('st.saved'));
      props.onDone();
    } catch (e) {
      setErr(callError(e).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="pe" data-preset-editor={preset.id}>
      {ROLE_IDS.map((role: RoleId) => {
        const r = roles[role];
        const sets = ROLE_SETTING[role];
        const agentId = `pe-${role}`;
        return (
          <div class="pe-role" key={role} data-pe-role={role}>
            <div class="rh"><b>{t(`st.r.${role}`)}</b></div>
            <AgentPick programs={programs} value={r.agent} onChange={(v) => set(role, { agent: v, ...(v ? {} : { effort: '' }) })} id={agentId} emptyLabel={t('st.peUnset')} />
            {sets.effort && <EffortPick agent={r.agent} value={r.effort} onChange={(v) => set(role, { effort: v })} id={agentId} emptyLabel={t('st.effortDefault')} />}
            {sets.fallback && (
              <div class="pe-fb">
                <span class="kl">{t('st.f.fallback')}</span>
                {r.fallback === ''
                  ? <><span class="hint">{t('st.peUnsetHint')}</span> <button class="btn sm" type="button" data-pe-fb-set={role} onClick={() => set(role, { fallback: 'none' })}>{t('st.peSetChain')}</button></>
                  : (
                    <>
                      <ChainEditor programs={programs} value={r.fallback} onChange={(v) => set(role, { fallback: v })} id={`${agentId}-fb`} agent={r.agent} />
                      <button class="btn sm" type="button" data-pe-fb-unset={role} onClick={() => set(role, { fallback: '' })}>{t('st.peUnsetChain')}</button>
                    </>
                  )}
              </div>
            )}
          </div>
        );
      })}
      {err && <div class="hint bad" id="st-pe-err">{err}</div>}
      <div class="act">
        <button class="btn primary sm" type="button" id="st-pe-save" disabled={busy || sameRoles(saved, roles)} onClick={() => void save()}>{t('st.peSave')}</button>
        <button class="btn sm" type="button" id="st-pe-cancel" onClick={props.onDone}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/* ───────── the list ───────── */

/** One line of what a preset gives each role. */
function brief(p: PresetItem): string {
  return ROLE_IDS.map((role) => {
    const r = p.roles[role];
    return r?.agent ? `${t(`st.r.${role}`)}: ${r.agent}` : '';
  }).filter(Boolean).join(' · ');
}

const TEMPLATES = ['economy', 'max', 'claude', 'noclaude'] as const;

export function Presets(props: { data: StationData; repo: string; reload: () => void }): JSX.Element {
  const { data, repo } = props;
  const { list, active } = data.presets;
  const [name, setName] = useState('');
  const [kind, setKind] = useState<string>(TEMPLATES[0]);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState('');
  const [renaming, setRenaming] = useState('');
  const [newName, setNewName] = useState('');
  useEffect(() => () => closeApply(), []);
  useEffect(() => { closeApply(); }, [repo]);

  const run = async (fn: () => Promise<unknown>, done?: string): Promise<boolean> => {
    setBusy(true);
    setErr('');
    try {
      await fn();
      if (done) toast(done);
      props.reload();
      return true;
    } catch (e) {
      setErr(callError(e).message);
      return false;
    } finally {
      setBusy(false);
    }
  };
  const capture = async (): Promise<void> => {
    if (await run(() => presetsPost({ op: 'capture', name, repo: repo || undefined }), t('st.saved'))) setName('');
  };
  const fromTemplate = async (): Promise<void> => {
    if (await run(() => presetsPost({ op: 'template', kind, name: name || undefined }), t('st.saved'))) setName('');
  };
  const del = (p: PresetItem): void => {
    confirmStep(`pdel:${p.id}`, async () => { await run(() => presetsPost({ op: 'delete', id: p.id }), t('st.pr.deleted')); });
  };
  const rename = async (p: PresetItem): Promise<void> => {
    if (await run(() => presetsPost({ op: 'rename', id: p.id, name: newName }))) setRenaming('');
  };
  const line = active
    ? (active.state === 'matches' ? t('st.pr.activeSame', { name: active.name }) : tn('st.pr.activeDiffers', active.diff, { name: active.name }))
    : t('st.pr.activeNone');
  return (
    <section class="st-sec" id="st-presets">
      <h3>{t('st.presets')}</h3>
      <p class="hint">{t('st.presetsHint')}</p>
      <div class={`pr-active ${active ? active.state : 'none'}`} id="st-pr-active" data-active={active?.id ?? ''} data-state={active?.state ?? 'none'}>{line}</div>
      {list.length === 0 && <div class="hint" id="st-pr-empty">{t('st.pr.empty')}</div>}
      <ul class="presets">
        {list.map((p) => {
          const isActive = active?.id === p.id;
          return (
            <li key={p.id} class={isActive ? 'active' : ''} data-preset={p.id} data-name={p.name} data-diff={p.diff}>
              <div class="pr-row">
                <b class="nm">{p.name}</b>
                <span class={`chip ${isActive ? (active?.state === 'matches' ? 'ok' : 'warn') : p.diff === 0 ? 'ok' : ''}`}>
                  {isActive ? (active?.state === 'matches' ? t('st.pr.isActive') : tn('st.pr.diffShort', p.diff)) : p.diff === 0 ? t('st.pr.same') : tn('st.pr.diffShort', p.diff)}
                </span>
                <span class="grow" />
                <button class="btn sm primary" type="button" data-preset-apply={p.id} disabled={busy || p.diff === 0} onClick={() => { setEditing(''); void startApply(p.id, repo); }}>{t('st.apply')}</button>
                <button class="btn sm" type="button" data-preset-edit={p.id} aria-expanded={editing === p.id} onClick={() => { closeApply(); setEditing(editing === p.id ? '' : p.id); }}>{t('st.change')}</button>
                <button class="btn sm" type="button" data-preset-rename={p.id} onClick={() => { setRenaming(renaming === p.id ? '' : p.id); setNewName(p.name); }}>{t('st.pr.rename')}</button>
                <button class={`btn sm danger ${armed.value?.key === `pdel:${p.id}` ? 'arm' : ''}`} type="button" data-preset-del={p.id} disabled={busy} onClick={() => del(p)}>
                  {armed.value?.key === `pdel:${p.id}` ? t('st.confirm') : t('st.pr.delete')}
                </button>
              </div>
              <div class="hint pr-brief">{brief(p) || t('st.pr.briefEmpty')}</div>
              {renaming === p.id && (
                <form class="sedit" onSubmit={(ev) => { ev.preventDefault(); void rename(p); }}>
                  <input class="field" id={`st-pr-rename-${p.id}`} value={newName} maxLength={40} aria-label={t('st.pr.name')} onInput={(e) => setNewName((e.target as HTMLInputElement).value)} />
                  <button class="btn primary sm" type="submit" disabled={busy || !newName.trim() || newName.trim() === p.name}>{t('st.pr.rename')}</button>
                  <button class="btn sm" type="button" onClick={() => setRenaming('')}>{t('cancel')}</button>
                </form>
              )}
              {editing === p.id && <PresetEditor preset={p} programs={data.programs} onDone={() => { setEditing(''); props.reload(); }} />}
              {applyFor.value?.id === p.id && <ApplyCard repo={repo} reload={props.reload} />}
            </li>
          );
        })}
      </ul>
      <div class="pr-new">
        <label class="fl2">{t('st.pr.name')}
          <input class="field" id="st-pr-name" value={name} maxLength={40} placeholder={t('st.pr.namePh')} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        </label>
        <div class="act">
          <button class="btn primary sm" type="button" id="st-pr-capture" disabled={busy || !name.trim()} onClick={() => void capture()}>{t('st.pr.capture')}</button>
        </div>
        <label class="fl2">{t('st.pr.template')}
          <select class="field" id="st-pr-kind" value={kind} onChange={(e) => setKind((e.target as HTMLSelectElement).value)}>
            {TEMPLATES.map((k) => <option key={k} value={k}>{t(`st.pr.t.${k}`)}</option>)}
          </select>
        </label>
        <div class="hint">{t(`st.pr.t.${kind}.hint`)}</div>
        <div class="act">
          <button class="btn sm" type="button" id="st-pr-template" disabled={busy} onClick={() => void fromTemplate()}>{t('st.pr.fromTemplate')}</button>
        </div>
      </div>
      {err && <div class="hint bad" id="st-pr-err">{err}</div>}
    </section>
  );
}
