import type { JSX } from 'preact';
import { useRef } from 'preact/hooks';
import { armed, tellDraft } from '../store';
import { ACTION_LABEL, boardOk, runTaskAction, tellTask } from '../actions';
import { t } from '../i18n';
import { AutoTextarea } from './AutoText';
import type { Task } from '../types';

/** The buttons of the board's actions for a task, exactly as the Focus shows them. */
export function TaskActionButtons(props: { task: Task }): JSX.Element {
  const task = props.task;
  const dis = !boardOk();
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
            onClick={() => runTaskAction(a, task)}
          >{on ? t('req.confirm', { a: label.toLowerCase() }) : label}</button>
        );
      })}
    </>
  );
}

/** The line that returns words to the agent (a refusal, a correction, an answer). */
export function TellBox(props: { task: Task; input?: string }): JSX.Element {
  const task = props.task;
  const id = props.input ?? 'tell';
  const dis = !boardOk();
  const ref = useRef<HTMLTextAreaElement>(null);
  const submit = async (): Promise<void> => {
    const el = ref.current;
    if (!el) return;
    await tellTask(task, el.value);
  };
  return (
    <div class="tell">
      <AutoTextarea
        id={id} data-id={task.id} class="field" elRef={ref} autocomplete="off" disabled={dis} value={tellDraft.value[task.id] ?? ''}
        placeholder={t('focus.tellPlaceholder', { tag: task.tag })} aria-label={t('focus.tell')}
        onInput={(e) => { tellDraft.value = { ...tellDraft.value, [task.id]: (e.target as HTMLTextAreaElement).value }; }}
        onSend={() => void submit()}
      />
      <button class="btn" type="button" data-act="tell-agent" data-id={task.id} aria-disabled={dis} onClick={() => void submit()}>{t('focus.send')}</button>
      <div class="keyhint" id={`${id}-hint`}>{t('text.keys.send')}</div>
    </div>
  );
}
