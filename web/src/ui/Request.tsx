import type { JSX } from 'preact';
import { armed, details, screens, tellDraft } from '../store';
import { fetchScreen, pickAttempt, runTaskAction, sendKey, boardOk } from '../actions';
import { requestType, requestTitle } from '../requests';
import type { Task } from '../types';
import { t } from '../i18n';
import { Gl } from './Gl';
import { Translatable } from './Translatable';
import { PERMISSION_ALLOW, PERMISSION_DENY } from '../requests';

/** One attempt of a pick request, as the task's details carry it. */
interface Attempt {
  letter: string;
  agent: string;
  standing: string;
  stat: string;
  diff?: string;
  report?: string;
}

function Screen(props: { target: string; label: string; role?: string }): JSX.Element {
  const s = screens.value[props.target];
  const role = props.role ?? '';
  if (!s) {
    return (
      <div class="row">
        <button class="btn" type="button" data-act="screen" data-target={props.target} onClick={() => void fetchScreen(props.target, role)}>
          <Gl n="screen" />{props.label}
        </button>
        <span class="hint">{t('req.screenHint')}</span>
      </div>
    );
  }
  if (s.busy && !s.text) return <div class="hint">{t('req.reading')}</div>;
  if (s.error) {
    return (
      <div class="screen">
        <div class="hint">{s.error}</div>
        <div class="row"><button class="btn" type="button" onClick={() => void fetchScreen(props.target, role)}><Gl n="refresh" />{t('req.again')}</button></div>
      </div>
    );
  }
  return (
    <div class="screen">
      <pre>{s.text}</pre>
      <div class="keys">
        {['up', 'down', 'left', 'right', 'enter', 'esc', '1', '2', '3', 'y', 'n'].map((k) => (
          <button class="btn sm" type="button" key={k} data-act="keys" data-target={props.target} data-keys={k} onClick={() => void sendKey(props.target, k, role)}>
            {k === 'enter' ? '↵ Enter' : k === 'esc' ? 'Esc' : k}
          </button>
        ))}
      </div>
      <div class="row"><button class="btn sm" type="button" onClick={() => void fetchScreen(props.target, role)}><Gl n="refresh" />{t('req.refreshScreen')}</button></div>
    </div>
  );
}

function ArmBtn(props: { id: string; cmd: string; label: string; armLabel: string; cls: string; task: Task; galaxy?: string }): JSX.Element {
  const key = `${props.id}:${props.cmd}`;
  const on = armed.value?.key === key;
  return (
    <button
      class={`btn ${props.cls} ${on ? 'arm' : ''}`} type="button" data-act={props.cmd} data-id={props.id}
      aria-disabled={!boardOk()} onClick={() => runTaskAction(props.cmd, props.task, props.galaxy ? { kind: 'planet', galaxy: props.galaxy, id: props.id } : undefined)}
    >{on ? props.armLabel : props.label}</button>
  );
}

/** The answers that fit what the task asks of the human, wherever the request is drawn (the Focus, the whole-page result). */
export function RequestActions(props: { task: Task; input?: string; galaxy?: string }): JSX.Element | null {
  const task = props.task;
  const type = requestType(task);
  const attention = ['waiting', 'failed', 'ready', 'plan'].includes(task.status);
  if (!attention) return null;
  const d = details.value[task.id];
  const dis = !boardOk();
  const input = props.input ?? 'tell';
  const run = (cmd: string): void => runTaskAction(cmd, task, props.galaxy ? { kind: 'planet', galaxy: props.galaxy, id: task.id } : undefined);
  const draft = (text: string): void => {
    tellDraft.value = { ...tellDraft.value, [task.id]: text };
    setTimeout(() => { const i = document.getElementById(input) as HTMLInputElement | null; if (i) { i.value = text; i.focus(); i.select(); } }, 30);
  };
  const toInput = (): void => { setTimeout(() => (document.getElementById(input) as HTMLInputElement | null)?.focus(), 30); };
  return (
    <>
      {type === 'permission' && (
        <>
          <div class="row">
            <button class="btn go" type="button" data-act="perm-allow" data-id={task.id} aria-disabled={dis} onClick={() => draft(PERMISSION_ALLOW())}>{t('req.allow')}</button>
            <button class="btn danger" type="button" data-act="perm-deny" data-id={task.id} aria-disabled={dis} onClick={() => draft(PERMISSION_DENY())}>{t('req.deny')}</button>
            <span class="hint">{t('req.textBelow')}</span>
          </div>
          {task.stage === 'approval' && (
            <div class="row">
              <ArmBtn id={task.id} cmd="anyway" label={t('act.anyway')} armLabel={t('req.confirmAnyway')} cls="danger" task={task} galaxy={props.galaxy} />
              <ArmBtn id={task.id} cmd="review" label={t('req.returnReview')} armLabel={t('req.confirmReturn')} cls="" task={task} galaxy={props.galaxy} />
            </div>
          )}
        </>
      )}
      {type === 'auth' && (
        <>
          <div class="row">
            <button class="btn" type="button" data-act="answer" data-id={task.id} onClick={toInput}>{t('req.writeAgent')}</button>
            <button class="btn" type="button" data-act="stop" data-id={task.id} aria-disabled={dis} onClick={() => run('stop')}>{t('act.stop')}</button>
          </div>
          <div class="hint">{t('req.authHint')}</div>
        </>
      )}
      {type === 'limit' && (
        <>
          <div class="row">
            <button class="btn go" type="button" data-act="retry" data-id={task.id} aria-disabled={dis} onClick={() => run('retry')}>{t('req.continue')}</button>
            <button class="btn" type="button" data-act="finish" data-id={task.id} aria-disabled={dis} onClick={() => run('finish')}>{t('act.finish')}</button>
          </div>
          <div class="hint">{t('req.limitHint')}</div>
        </>
      )}
      {type === 'failed' && (
        <div class="row">
          <button class="btn go" type="button" data-act="retry" data-id={task.id} aria-disabled={dis} onClick={() => run('retry')}>{t('act.retry')}</button>
          <button class="btn" type="button" data-act="fix" data-id={task.id} aria-disabled={dis} onClick={() => run('fix')}>{t('act.fix')}</button>
          <button class="btn" type="button" data-act="finish" data-id={task.id} aria-disabled={dis} onClick={() => run('finish')}>{t('act.finish')}</button>
          <button class="btn danger" type="button" data-act="stop" data-id={task.id} aria-disabled={dis} onClick={() => run('stop')}>{t('act.stop')}</button>
        </div>
      )}
      {type === 'plan' && (
        <div class="row">
          <ArmBtn id={task.id} cmd="approve" label={t('req.approvePlan')} armLabel={t('req.confirmApprovePlan')} cls="go" task={task} galaxy={props.galaxy} />
          <button class="btn" type="button" data-act="answer" data-id={task.id} onClick={toInput}>{t('req.returnWithNotes')}</button>
        </div>
      )}
      {type === 'approval' && (
        <div class="row">
          <ArmBtn id={task.id} cmd="approve" label={t('act.approve')} armLabel={t('req.confirmApprove')} cls="go" task={task} galaxy={props.galaxy} />
          <ArmBtn id={task.id} cmd="anyway" label={t('act.anyway')} armLabel={t('req.confirmAnyway')} cls="danger" task={task} galaxy={props.galaxy} />
        </div>
      )}
      {type === 'pick' && (
        <div class="picks">
          {Array.isArray(d?.attempts) && d.attempts.length ? (d.attempts as Attempt[]).map((a) => (
            <div class="pick" key={a.letter}>
              <span class="l">{a.letter}</span>
              <div>
                <div>{a.agent}</div>
                <div class={`st ${a.standing === 'passed' ? 'pass' : a.standing === 'failed' ? 'fail' : ''}`}>{a.standing} · {a.stat}</div>
              </div>
              <button class="btn" type="button" data-act="pick" data-id={task.id} data-letter={a.letter} aria-disabled={dis} onClick={() => void pickAttempt(task, a.letter)}>{t('req.pick', { l: a.letter })}</button>
              <div class="links">
                {a.diff && <a href={a.diff} target="_blank" rel="noopener">{t('universe.diff')}</a>}
                {a.report && <a href={a.report} target="_blank" rel="noopener">{t('req.report')}</a>}
              </div>
            </div>
          )) : <div class="hint">{t('req.attemptsLoading')}</div>}
        </div>
      )}
      {type === 'reviewneeds' && <Screen target={task.id} label={t('req.showReviewer')} role="reviewer" />}
      {(type === 'question') && (
        <div class="row"><button class="btn" type="button" data-act="answer" data-id={task.id} onClick={toInput}>{t('req.answer')}</button></div>
      )}
    </>
  );
}

/** What the task asks, in the words of the board: the title of the request and its message. */
export function RequestHead(props: { task: Task }): JSX.Element {
  const task = props.task;
  return (
    <>
      <div class="rh"><Gl n={task.status} />{requestTitle(requestType(task))}</div>
      <p class="full"><Translatable text={task.msg || task.board_label} /></p>
    </>
  );
}

/** What the task asks of the human, with the answers that fit it. */
export function RequestBlock(props: { task: Task; input?: string }): JSX.Element | null {
  const task = props.task;
  const type = requestType(task);
  const attention = ['waiting', 'failed', 'ready', 'plan'].includes(task.status);
  if (!attention) return null;
  return (
    <div class={`req s-${task.status}`} data-type={type}>
      <RequestHead task={task} />
      <RequestActions task={task} input={props.input} />
    </div>
  );
}

export { Screen };
