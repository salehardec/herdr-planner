/* The agent's result or the task's description in a centred window over the Galaxy: markdown in a wide
   column, with the answers of the task in reach. Esc (see keys.ts through dialogs.ts), «Свернуть» and a
   click on the background close it. */
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { addDialogs } from '../dialogs';
import { taskOf } from '../store';
import { closing, loadClosing } from '../closing';
import { t } from '../i18n';
import { Badge, Gl } from './Gl';
import { Translatable } from './Translatable';
import { TaskActionButtons, TellBox } from './AgentActions';
import { RequestActions, RequestHead } from './Request';
import './summary.css';

const attention = (status: string): boolean => ['waiting', 'failed', 'ready', 'plan'].includes(status);

export function AgentSummary(): JSX.Element | null {
  const ref = addDialogs.summary.value;
  const part = ref?.part ?? 'closing';
  const task = ref ? taskOf(ref.task) : undefined;
  useEffect(() => {
    if (task && part === 'closing') void loadClosing(task);
  }, [task?.id, task?.status, task?.msg, part]);
  if (!ref) return null;
  const close = (): void => { addDialogs.summary.value = null; };
  const cl = task && part === 'closing' ? closing.value[task.id] : undefined;
  const title = part === 'description' ? t('focus.description') : t('focus.closing');
  return (
    <div class="scrim summary-scrim" id="summary" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel summary" role="dialog" aria-modal="true" aria-label={title}>
        <header>
          <span class="sh-title">
            <Gl n="expand" />
            <b>{title}</b>
            {task && <span class="tagk">{task.tag}</span>}
            {task && <Badge status={task.status} label={task.label || t(`status.${task.status}`)} />}
          </span>
          <span class="sh-buttons">
            <button class="btn" type="button" data-act="summary-close" onClick={close}><Gl n="shrink" />{t('focus.collapse')}</button>
            <button class="btn icon sm" type="button" data-act="summary-close" aria-label={t('focus.collapse')} onClick={close}><Gl n="close" /></button>
          </span>
        </header>
        <div class="summary-scroll">
          <div class="summary-col">
            {!task ? <p class="hint">{t('focus.gone')}</p> : (
              <>
                <h2>{task.title}</h2>
                {part === 'description' ? (
                  task.description
                    ? <Translatable text={task.description} md bar="top" />
                    : <p class="hint">{t('focus.descriptionNone')}</p>
                ) : cl?.text ? <Translatable text={cl.text} md bar="top" /> : (
                  <p class="hint">{task.status === 'done' ? t('focus.closingNone') : t('focus.closingLoading')}</p>
                )}
                {cl?.truncated && <p class="hint">{t('focus.closingCut')}</p>}
              </>
            )}
          </div>
        </div>
        {task && (
          <div class="summary-foot">
            {attention(task.status) && (
              <>
                <div class={`req s-${task.status} summary-request`}><RequestHead task={task} /></div>
                <RequestActions task={task} input="summary-tell" />
              </>
            )}
            {!attention(task.status) && <div class="row"><TaskActionButtons task={task} /></div>}
            <TellBox task={task} input="summary-tell" />
          </div>
        )}
      </section>
    </div>
  );
}
