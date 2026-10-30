import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, details, screens, tellDraft } from '../store';
import { pickAttempt, runTaskAction, boardOk, tellTask, fetchScreen, answerMenu } from '../actions';
import { requestType, requestTitle, PERMISSION_ALLOW, PERMISSION_DENY } from '../requests';
import { taskPermission, questionText, boardPlaceholder, type PermissionNote, type PermissionPrompt } from '../permission';
import type { MenuData, Task } from '../types';
import { t } from '../i18n';
import { addDialogs } from '../dialogs';
import { toast } from '../toast';
import { Gl } from './Gl';
import { Translatable } from './Translatable';
import { safeUrl } from '../mobile/cards';

export function MergeHandoff(props: { task: Task }): JSX.Element {
  const url = safeUrl(props.task.merge_request ?? '');
  return <div class="merge-handoff" role="status">
    <p>{t('req.mergeViaMr')}</p>
    {url && <a class="btn" href={url} target="_blank" rel="noopener noreferrer" data-act="mr">{t('m.openMr')}<Gl n="ext" /></a>}
  </div>;
}

/** One attempt of a pick request, as the task's details carry it. */
interface Attempt {
  letter: string;
  agent: string;
  standing: string;
  stat: string;
  diff?: string;
  report?: string;
}

function ArmBtn(props: { id: string; cmd: string; label: string; armLabel: string; cls: string; task: Task }): JSX.Element {
  const key = `${props.id}:${props.cmd}`;
  const on = armed.value?.key === key;
  return (
    <button
      class={`btn ${props.cls} ${on ? 'arm' : ''}`} type="button" data-act={props.cmd} data-id={props.id}
      aria-disabled={!boardOk()} onClick={() => runTaskAction(props.cmd, props.task)}
    >{on ? props.armLabel : props.label}</button>
  );
}

function ScreenBtn(props: { task: Task }): JSX.Element {
  return (
    <button
      class="btn" type="button" data-act="screen" data-target={props.task.id}
      onClick={() => { addDialogs.screen.value = { task: props.task.id }; }}
    ><Gl n="screen" />{t('focus.agentScreen')}</button>
  );
}

/** What the prompt names: the tool, the command, the file the agent asks about. */
function DialogWhat(props: { prompt: PermissionPrompt }): JSX.Element | null {
  if (!props.prompt.question && !props.prompt.context) return null;
  return (
    <>
      {props.prompt.question && <p class="full dlg-q">{props.prompt.question}</p>}
      {props.prompt.context && <pre class="dlgctx">{props.prompt.context}</pre>}
    </>
  );
}

/** The rows of a permission prompt as buttons: each one sends the keys that choose that row. */
function DialogButtons(props: { task: Task; prompt: PermissionPrompt; menu: MenuData | null }): JSX.Element {
  const [busy, setBusy] = useState(false);
  const dis = !boardOk() || busy;
  const send = async (at: number, note: PermissionNote, label: string): Promise<void> => {
    if (!props.menu) return;
    setBusy(true);
    const r = await answerMenu(props.task.id, '', props.menu.sig, { kind: 'pick', at }, note);
    setBusy(false);
    if (r.ok) {
      toast(t('amenu.answered', { a: label }));
      setTimeout(() => { void fetchScreen(props.task.id); }, 600);
    } else {
      toast(r.gone ? t('amenu.gone') : t('amenu.failed', { why: r.error }), { icon: 'failed' });
    }
  };
  const p = props.prompt;
  const always = p.always;
  return (
    <div class="row">
      <button class="btn go" type="button" data-act="dlg-allow" data-id={props.task.id} aria-disabled={dis}
        onClick={() => void send(p.allow, 'allow', t('req.allow'))}>{t('req.allow')}</button>
      {always !== undefined && (
        <button class="btn" type="button" data-act="dlg-always" data-id={props.task.id} aria-disabled={dis}
          onClick={() => void send(always, 'always', t('req.allowAlways'))}>{t('req.allowAlways')}</button>
      )}
      <button class="btn danger" type="button" data-act="dlg-deny" data-id={props.task.id} aria-disabled={dis}
        onClick={() => void send(p.deny, 'deny', t('req.deny'))}>{t('req.deny')}</button>
      <ScreenBtn task={props.task} />
    </div>
  );
}

/** The answers that fit what the task asks of the human, wherever the request is drawn (the Focus, the whole-page result). */
export function RequestActions(props: { task: Task; input?: string }): JSX.Element | null {
  const task = props.task;
  const type = requestType(task);
  const attention = ['waiting', 'failed', 'ready', 'plan'].includes(task.status);
  if (!attention) return null;
  if (task.merge_via_mr) return <MergeHandoff task={task} />;
  const d = details.value[task.id];
  const dis = !boardOk();
  const input = props.input ?? 'tell';
  const run = (cmd: string): void => runTaskAction(cmd, task);
  const draft = (text: string): void => {
    tellDraft.value = { ...tellDraft.value, [task.id]: text };
    setTimeout(() => { const i = document.getElementById(input) as HTMLTextAreaElement | null; if (i) { i.value = text; i.focus(); i.select(); } }, 30);
  };
  const toInput = (): void => { setTimeout(() => (document.getElementById(input) as HTMLTextAreaElement | null)?.focus(), 30); };
  // «Вернуть с замечаниями»: the words in the field below go to the agent; an empty field asks for them first.
  const returnNotes = async (): Promise<void> => {
    const el = document.getElementById(input) as HTMLTextAreaElement | null;
    const text = el?.value ?? tellDraft.value[task.id] ?? '';
    if (!text.trim()) { el?.focus(); toast(t('req.notesRequired')); return; }
    await tellTask(task, text);
  };
  return (
    <>
      {type === 'permission' && (
        <>
          <div class="row">
            <button class="btn go" type="button" data-act="perm-allow" data-id={task.id} aria-disabled={dis} onClick={() => draft(PERMISSION_ALLOW())}>{t('req.allow')}</button>
            <button class="btn danger" type="button" data-act="perm-deny" data-id={task.id} aria-disabled={dis} onClick={() => draft(PERMISSION_DENY())}>{t('req.deny')}</button>
            <ScreenBtn task={task} />
            <span class="hint">{t('req.textBelow')}</span>
          </div>
          {task.stage === 'approval' && (
            <div class="row">
              <ArmBtn id={task.id} cmd="anyway" label={t('act.anyway')} armLabel={t('req.confirmAnyway')} cls="danger" task={task} />
              <ArmBtn id={task.id} cmd="review" label={t('req.returnReview')} armLabel={t('req.confirmReturn')} cls="" task={task} />
            </div>
          )}
        </>
      )}
      {type === 'auth' && (
        <>
          <div class="row">
            <button class="btn" type="button" data-act="answer" data-id={task.id} onClick={toInput}>{t('req.writeAgent')}</button>
            <button class="btn" type="button" data-act="stop" data-id={task.id} aria-disabled={dis} onClick={() => run('stop')}>{t('act.stop')}</button>
            <ScreenBtn task={task} />
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
          <ArmBtn id={task.id} cmd="approve" label={t('req.approvePlan')} armLabel={t('req.confirmApprovePlan')} cls="go" task={task} />
          <button class="btn" type="button" data-act="plan-return" data-id={task.id} aria-disabled={dis} onClick={() => void returnNotes()}>{t('req.returnWithNotes')}</button>
        </div>
      )}
      {type === 'approval' && (
        <div class="row">
          <ArmBtn id={task.id} cmd="approve" label={t('act.approve')} armLabel={t('req.confirmApprove')} cls="go" task={task} />
          <ArmBtn id={task.id} cmd="anyway" label={t('act.anyway')} armLabel={t('req.confirmAnyway')} cls="danger" task={task} />
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
      {type === 'reviewneeds' && (
        <div class="row">
          <button
            class="btn" type="button" data-act="screen" data-target={task.id} data-role="reviewer"
            onClick={() => { addDialogs.screen.value = { task: task.id, role: 'reviewer' }; }}
          ><Gl n="screen" />{t('req.showReviewer')}</button>
          <span class="hint">{t('req.screenHint')}</span>
        </div>
      )}
      {type === 'question' && (
        <div class="row">
          <button class="btn" type="button" data-act="answer" data-id={task.id} onClick={toInput}>{t('req.answer')}</button>
          <ScreenBtn task={task} />
        </div>
      )}
    </>
  );
}

/** What the task asks, in the words of the board: the title of the request and its message. */
export function RequestHead(props: { task: Task; menu?: MenuData | null }): JSX.Element {
  const task = props.task;
  const type = requestType(task);
  const text = type === 'question'
    ? questionText(task, props.menu) || t('req.questionNoText')
    : (task.msg.trim() && !boardPlaceholder(task.msg) ? task.msg : '') || task.board_label || '';
  return (
    <>
      <div class="rh"><Gl n={task.status} />{requestTitle(type)}</div>
      {text && <p class="full"><Translatable text={text} /></p>}
    </>
  );
}

/** A permission prompt that comes on top of another request (a plan that waits while the agent asks to do something). */
function DialogBlock(props: { task: Task; prompt: PermissionPrompt; menu: MenuData | null }): JSX.Element {
  return (
    <div class="req s-waiting second" data-type="dialog">
      <div class="rh"><Gl n="waiting" />{t('req.dialog')}</div>
      <DialogWhat prompt={props.prompt} />
      <DialogButtons task={props.task} prompt={props.prompt} menu={props.menu} />
    </div>
  );
}

/** What the task asks of the human, with the answers that fit it. */
export function RequestBlock(props: { task: Task; input?: string }): JSX.Element | null {
  const task = props.task;
  const type = requestType(task);
  const attention = ['waiting', 'failed', 'ready', 'plan'].includes(task.status);
  const shown = screens.value[task.id];
  const menu: MenuData | null = shown && !shown.error && shown.menu ? shown.menu : null;
  // The screen is read once for the cards that ask a permission or a question: what the agent says is not in the board.
  useEffect(() => {
    // A dialog may come as `kind: question` or with no kind at all (the board says «Needs you»): the screen decides.
    const wants = task.status === 'plan' || task.kind === 'permission' || task.kind === 'question' || task.kind === 'auth'
      || (task.status === 'waiting' && !task.kind);
    if (attention && wants && task.exited_at === 0 && task.status !== 'queued' && task.status !== 'done') void fetchScreen(task.id);
  }, [task.id, task.status, task.msg, task.stage, task.kind, attention]);
  if (!attention) return null;
  // A permission the screen makes plain: the board says so, or the prompt names a tool or a command.
  const prompt = taskPermission(task, menu);
  // The board calls a dialog a question (or gives it no kind): when the screen draws a permission prompt, that is the card.
  if (prompt && (type === 'permission' || type === 'question')) {
    return (
      <div class="req s-waiting" data-type="permission">
        <div class="rh"><Gl n="waiting" />{t('req.permission')}</div>
        {task.msg.trim() && !boardPlaceholder(task.msg) && <p class="full"><Translatable text={task.msg} /></p>}
        <DialogWhat prompt={prompt} />
        <DialogButtons task={task} prompt={prompt} menu={menu} />
      </div>
    );
  }
  if (type === 'plan') {
    return (
      <>
        <div class="req s-plan" data-type="plan">
          <div class="rh"><Gl n="plan" />{t('req.planReady')}</div>
          {task.msg && <p class="full"><Translatable text={task.msg} /></p>}
          <div class="row">
            <button
              class="btn" type="button" data-act="plan-open" data-id={task.id}
              onClick={() => { addDialogs.summary.value = { task: task.id, part: 'plan' }; }}
            ><Gl n="expand" />{t('req.planOpen')}</button>
            <span class="hint">{t('req.planNotesHint')}</span>
          </div>
          <RequestActions task={task} input={props.input} />
        </div>
        {prompt && <DialogBlock task={task} prompt={prompt} menu={menu} />}
      </>
    );
  }
  return (
    <>
      <div class={`req s-${task.status}`} data-type={type}>
        <RequestHead task={task} menu={menu} />
        <RequestActions task={task} input={props.input} />
      </div>
    </>
  );
}
