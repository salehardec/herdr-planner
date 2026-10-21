import type { JSX } from 'preact';
import { armed, galaxyId, tellDraft } from '../store';
import { ACTION_LABEL, boardOk, runTaskAction, tellTask } from '../actions';
import { t } from '../i18n';
import type { Task } from '../types';

/** The buttons of the board's actions for a task, exactly as the Focus shows them. */
export function TaskActionButtons(props: { task: Task; galaxy?: string }): JSX.Element {
  const task = props.task;
  const galaxy = props.galaxy ?? galaxyId.value;
  const dis = !boardOk() && galaxy === '';
  const actions = (task.actions || []).filter((a) => !['tell', 'answer'].includes(a));
  return (
    <>
      {actions.map((a) => {
        const on = armed.value?.key === `${task.id}:${a}`;
        const label = a === 'approve' && task.status === 'plan' ? t('req.approvePlan') : t(ACTION_LABEL[a] ?? a);
        return (
          <button
            key={a} type="button" data-act={a} data-id={task.id} aria-disabled={dis}
            class={`btn ${a === 'approve' ? 'go' : ''} ${a === 'anyway' ? 'danger' : ''} ${on ? 'arm' : ''}`}
            onClick={() => runTaskAction(a, task, { kind: 'planet', galaxy, id: task.id })}
          >{on ? t('req.confirm', { a: label.toLowerCase() }) : label}</button>
        );
      })}
    </>
  );
}

/** The line that returns words to the agent (a refusal, a correction, an answer). */
export function TellBox(props: { task: Task; galaxy?: string; input?: string }): JSX.Element {
  const task = props.task;
  const id = props.input ?? 'tell';
  const galaxy = props.galaxy ?? galaxyId.value;
  const dis = !boardOk() && galaxy === '';
  const submit = async (): Promise<void> => {
    const el = document.getElementById(id) as HTMLInputElement | null;
    if (!el) return;
    if (await tellTask(task, el.value, galaxy || undefined)) el.value = '';
  };
  return (
    <div class="tell">
      <input
        id={id} data-id={task.id} class="field" autocomplete="off" disabled={dis} value={tellDraft.value[task.id] ?? ''}
        placeholder={t('focus.tellPlaceholder', { tag: task.tag })} aria-label={t('focus.tell')}
        onInput={(e) => { tellDraft.value = { ...tellDraft.value, [task.id]: (e.target as HTMLInputElement).value }; }}
        onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void submit(); } }}
      />
      <button class="btn" type="button" data-act="tell-agent" data-id={task.id} aria-disabled={dis} onClick={() => void submit()}>{t('focus.send')}</button>
    </div>
  );
}
