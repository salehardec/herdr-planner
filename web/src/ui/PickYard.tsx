/* «Верфь задачи»: the yard rule of one task or of its repository (PLN-45). A rule names a yard, the tags the yard must
   carry, or both; the server checks it, writes it to runners.json and puts the rule's block into the agents' instructions
   when the shipyards' block is connected. The card shows what the rule picks right now, spare yards included. */
import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { armed, snap } from '../store';
import { confirmStep } from '../actions';
import { post } from '../api';
import { t } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import type { RunnerPick, Task } from '../types';

type Scope = 'task' | 'repo';

export function PickYard(props: { task: Task; onClose: () => void }): JSX.Element {
  const rules = snap.value?.runner_rules;
  // What this scope says now: its own rule only (a task without one follows its repository).
  const read = (s: Scope): { runner: string; tags: string } => {
    const r = snap.value?.runner_rules;
    if (!r) return { runner: '', tags: '' };
    if (s === 'task') {
      const rule = r.tasks[props.task.id];
      return { runner: rule?.runner ?? '', tags: (rule?.tags ?? []).join(', ') };
    }
    return { runner: r.assign[props.task.repo] ?? '', tags: (r.need[props.task.repo] ?? []).join(', ') };
  };
  const start = read('task');
  const [scope, setScope] = useState<Scope>('task');
  const [runner, setRunner] = useState(start.runner);
  const [tags, setTags] = useState(start.tags);
  const [err, setErr] = useState('');
  const id = scope === 'task' ? props.task.id : props.task.repo;

  // A scope switch fills the fields at once — no effect after the paint that could overwrite a choice just made.
  const switchScope = (s: Scope): void => {
    setScope(s);
    const v = read(s);
    setRunner(v.runner);
    setTags(v.tags);
    setErr('');
  };
  const pick: RunnerPick | undefined = scope === 'task'
    ? snap.value?.runner_picks?.[props.task.id]
    : snap.value?.runner_repo_picks?.[props.task.repo];
  const own = scope === 'task' ? !!rules?.tasks[props.task.id] : !!rules?.assign[props.task.repo] || !!rules?.need[props.task.repo];
  const key = `rpk:${scope}:${id}`;
  const save = (clear: boolean): void => {
    setErr('');
    confirmStep(clear ? `${key}:clear` : key, async () => {
      try {
        await post('/api/runner/pick', {
          scope, id,
          runner: clear ? '' : runner,
          tags: clear ? [] : tags.split(',').map((x) => x.trim()).filter(Boolean),
          confirm: true,
        });
        toast(clear ? t('runner.pick.cleared') : t('runner.pick.saved'));
        props.onClose();
      } catch (e) {
        setErr((e as Error).message);
      }
    });
  };
  return (
    <section class="rf-edit" id="rf-pick">
      <h3>{t('runner.pick.cardTitle')}</h3>
      <div class="row" role="group" aria-label={t('runner.pick.scope')}>
        <button class={`btn sm ${scope === 'task' ? 'primary' : ''}`} type="button" id="rp-scope-task"
          onClick={() => switchScope('task')}>{t('runner.pick.scopeTask')}</button>
        <button class={`btn sm ${scope === 'repo' ? 'primary' : ''}`} type="button" id="rp-scope-repo"
          onClick={() => switchScope('repo')}>{t('runner.pick.scopeRepo')}</button>
      </div>
      <p class="hint" id="rp-hint">
        {scope === 'task' ? t('runner.pick.taskHint', { repo: props.task.repo }) : t('runner.pick.repoHint')}
      </p>
      <label>{t('runner.pick.runner')}
        <select class="field" id="rp-runner" value={runner} onChange={(e) => setRunner((e.target as HTMLSelectElement).value)}>
          <option value="">{t('runner.pick.defaultOpt')}</option>
          {(snap.value?.runners ?? []).map((r) => (
            <option value={r.id} key={r.id}>{r.id}{r.tags.length ? ` · ${r.tags.join(', ')}` : ''}</option>
          ))}
        </select>
      </label>
      <label>{t('runner.pick.tags')}
        <input class="field" id="rp-tags" value={tags} placeholder={t('runner.pick.tagsPh')} onInput={(e) => setTags((e.target as HTMLInputElement).value)} />
      </label>
      <p class="hint" id="rp-now">
        {pick?.runner
          ? `${pick.runner} · ${t(`runner.pick.reason.${pick.reason}`)}${pick.need.length ? ` · ${pick.need.join(', ')}` : ''}${pick.shifted ? ` · ${t(`runner.pick.shifted.${pick.shifted}`)}` : ''}`
          : pick ? (pick.need.length ? t('runner.pick.noneTags', { tags: pick.need.join(', ') }) : t('runner.pick.none')) : '—'}
      </p>
      {pick && pick.chain.length > 1 && <p class="hint">{t('runner.pick.chain', { chain: pick.chain.join(' → ') })}</p>}
      {!own && <p class="hint">{t('runner.pick.noOwn')}</p>}
      {err && <p class="hint bad">{err}</p>}
      <div class="act">
        <button class={`btn primary ${armed.value?.key === key ? 'arm' : ''}`} type="button" id="rp-save" onClick={() => save(false)}>
          <Gl n="yard" />{armed.value?.key === key ? t('runner.pick.saveConfirm') : t('runner.pick.save')}
        </button>
        {own && (
          <button class={`btn ${armed.value?.key === `${key}:clear` ? 'arm' : ''}`} type="button" id="rp-clear" onClick={() => save(true)}>
            {armed.value?.key === `${key}:clear` ? t('runner.pick.clearConfirm') : t('runner.pick.clear')}
          </button>
        )}
        <button class="btn" type="button" onClick={props.onClose}>{t('cancel')}</button>
      </div>
    </section>
  );
}
