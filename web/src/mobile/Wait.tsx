/* «Ждёт тебя» on the phone: one card per request, with the answers a phone has (cards.ts) and no more. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, details, requests, screens } from '../store';
import { answerMenu, boardOk, fetchScreen, pickAttempt, runTaskAction, tellTask } from '../actions';
import { fetchDetails, onQueueAnswer, prepareAnswer } from '../nav';
import { send } from '../chat';
import { boardPlaceholder, questionText, taskPermission, type PermissionNote, type PermissionPrompt } from '../permission';
import { nextRequest } from '../request-queue';
import { requestType } from '../requests';
import { t } from '../i18n';
import { toast } from '../toast';
import type { MenuData, Request, Task } from '../types';
import { Gl } from '../ui/Gl';
import { MenuSlot } from '../ui/MenuCard';
import { planOf, listed, type CardPlan } from './cards';
import { drafts, dropDraft, setDraft, sheet, tab, listTab } from './state';

/** A text longer than this folds, with «Ещё». */
const FOLD = 260;
/** How often a card that waits on an agent's dialog reads the agent's screen again. */
const SCREEN_EVERY_MS = 4000;

function Fold(props: { text: string; cls?: string }): JSX.Element | null {
  const [open, setOpen] = useState(false);
  if (!props.text) return null;
  const long = props.text.length > FOLD;
  return (
    <>
      <div class={`cmsg ${props.cls ?? ''} ${long && !open ? 'clamp' : ''}`}>{props.text}</div>
      {long && <button class="linkbtn" type="button" onClick={() => setOpen(!open)}>{open ? t('m.less') : t('m.more')}</button>}
    </>
  );
}

/** The words a person writes to the agent: remarks to a plan, an answer, advice. Kept while the list is redrawn. */
function Tell(props: { task: Task; id: string; label: string; placeholder: string; rows?: number; go?: boolean }): JSX.Element {
  const key = `tell:${props.id}`;
  const text = drafts.value[key] ?? '';
  const [busy, setBusy] = useState(false);
  const dis = !boardOk() || busy;
  const submit = async (): Promise<void> => {
    if (!text.trim() || dis) return;
    setBusy(true);
    if (await tellTask(props.task, text)) dropDraft(key);
    setBusy(false);
  };
  return (
    <div class="tellbox">
      <textarea
        class="in" rows={props.rows ?? 2} value={text} placeholder={props.placeholder} aria-label={props.placeholder}
        onInput={(e) => setDraft(key, (e.target as HTMLTextAreaElement).value)}
      />
      <button class={`btn sm ${props.go ? 'go' : ''}`} type="button" data-act="tell" data-id={props.task.id} aria-disabled={dis || !text.trim()} onClick={() => void submit()}>{props.label}</button>
    </div>
  );
}

/** A button that asks twice (the plan, the merge): the first press says so on the button, the second goes. */
function ArmBtn(props: { task: Task; cmd: string; label: string; armLabel: string }): JSX.Element {
  const on = armed.value?.key === `${props.task.id}:${props.cmd}`;
  return (
    <button
      class={`btn go ${on ? 'arm' : ''}`} type="button" data-act={props.cmd} data-id={props.task.id} aria-disabled={!boardOk()}
      onClick={() => runTaskAction(props.cmd, props.task)}
    >{on ? props.armLabel : props.label}</button>
  );
}

/** A permission prompt as three buttons: each sends the keys that choose its row, after the server has read the screen again. */
function Permission(props: { task: Task; prompt: PermissionPrompt; menu: MenuData }): JSX.Element {
  const [busy, setBusy] = useState(false);
  const { task, prompt, menu } = props;
  const dis = !boardOk() || busy;
  const answer = async (at: number, note: PermissionNote, label: string): Promise<void> => {
    setBusy(true);
    const r = await answerMenu(task.id, '', menu.sig, { kind: 'pick', at }, note);
    setBusy(false);
    if (r.ok) {
      toast(t('amenu.answered', { a: label }));
      setTimeout(() => { void fetchScreen(task.id); }, 600);
    } else {
      toast(r.gone ? t('amenu.gone') : t('amenu.failed', { why: r.error }), { icon: 'failed' });
    }
  };
  return (
    <>
      {prompt.question && <p class="cmsg">{prompt.question}</p>}
      {prompt.context && <pre class="perm">{prompt.context}</pre>}
      <div class="acts">
        <button class="btn go" type="button" data-act="dlg-allow" data-id={task.id} aria-disabled={dis} onClick={() => void answer(prompt.allow, 'allow', t('req.allow'))}>{t('req.allow')}</button>
        {prompt.always !== undefined && (
          <button class="btn" type="button" data-act="dlg-always" data-id={task.id} aria-disabled={dis} onClick={() => void answer(prompt.always as number, 'always', t('req.allowAlways'))}>{t('m.always')}</button>
        )}
        <button class="btn danger" type="button" data-act="dlg-deny" data-id={task.id} aria-disabled={dis} onClick={() => void answer(prompt.deny, 'deny', t('req.deny'))}>{t('req.deny')}</button>
      </div>
      <div class="hintline">{t('m.permHint')}</div>
    </>
  );
}

/** The attempts of a «выбери» task, as the board's details carry them. */
interface Attempt { letter: string; agent: string; standing: string; stat: string }

function Picks(props: { task: Task }): JSX.Element {
  const task = props.task;
  const d = details.value[task.id];
  useEffect(() => { void fetchDetails(task.id); }, [task.id]);
  const list: Attempt[] = Array.isArray(d?.attempts) ? d.attempts : [];
  if (!list.length) return <div class="hintline">{t('req.attemptsLoading')}</div>;
  return (
    <div class="opts">
      {list.map((a) => (
        <button class="btn opt" type="button" key={a.letter} data-act="pick" data-letter={a.letter} aria-disabled={!boardOk()} onClick={() => void pickAttempt(task, a.letter)}>
          <b>{a.letter})</b>
          <span class="optx"><span>{a.agent}</span><span class={`st ${a.standing === 'passed' ? 'pass' : a.standing === 'failed' ? 'fail' : ''}`}>{a.standing} · {a.stat}</span></span>
        </button>
      ))}
    </div>
  );
}

/** The Coordinator's own question in a menu: read from its screen while the card is there. */
function CoordinatorMenu(): JSX.Element {
  const scr = screens.value.coordinator;
  const menu: MenuData | null = scr && !scr.error ? (scr.menu ?? null) : null;
  useEffect(() => {
    void fetchScreen('coordinator');
    const id = window.setInterval(() => { if (!document.hidden) void fetchScreen('coordinator'); }, 2000);
    return () => window.clearInterval(id);
  }, []);
  return (
    <MenuSlot
      target="coordinator" menu={menu} place="chat"
      onAnswered={() => { void fetchScreen('coordinator'); setTimeout(() => { void fetchScreen('coordinator'); }, 900); }}
      fallback={<pre class="perm">{scr?.error ? scr.error : scr?.text || t('chat.loading')}</pre>}
    />
  );
}

/** What the agent asks, read from its screen: a permission as buttons, a question as the menu card, nothing as a plain question. */
function Asking(props: { task: Task; plan: CardPlan; text: string }): JSX.Element {
  const { task, plan } = props;
  const shown = screens.value[task.id];
  const menu: MenuData | null = shown && !shown.error && shown.menu ? shown.menu : null;
  useEffect(() => {
    // as on the computer: a dialog may come as a question, or with no kind at all; the screen is the witness
    const wants = task.status === 'plan' || task.kind === 'permission' || task.kind === 'question' || task.kind === 'auth'
      || (task.status === 'waiting' && !task.kind);
    if (!(plan.screen && wants && task.exited_at === 0)) return;
    void fetchScreen(task.id);
    // the dialog may come after the card, and an answer given at the terminal takes it away: read the screen again now and then,
    // while this page is the one that is looked at
    const id = window.setInterval(() => { if (!document.hidden && tab.value !== 'board') void fetchScreen(task.id); }, SCREEN_EVERY_MS);
    return () => window.clearInterval(id);
  }, [task.id, task.status, task.msg, task.stage, task.kind]);
  const prompt = taskPermission(task, menu);
  if (prompt && menu && (plan.block === 'permission' || plan.block === 'question' || plan.block === 'plan')) {
    return <Permission task={task} prompt={prompt} menu={menu} />;
  }
  return (
    <>
      <Fold text={props.text} />
      {menu && plan.block !== 'plan' && (
        <MenuSlot target={task.id} menu={menu} place="chat" onAnswered={() => { setTimeout(() => { void fetchScreen(task.id); }, 700); }} />
      )}
    </>
  );
}

function Pills(props: { task: Task }): JSX.Element | null {
  const task = props.task;
  const out: [string, string][] = [];
  if (task.review) {
    const tally = task.review.tally;
    if (tally) out.push([`${t('m.review')} ${tally}`, task.review.pass === false ? 'bad' : task.review.pass ? 'ok' : '']);
  }
  if (task.review_failed) out.push([t('m.reviewFailed'), 'bad']);
  if (task.merge_blocked) out.push([t('m.mergeBlocked'), 'bad']);
  if (task.pr) out.push([t('m.pr'), '']);
  if (!out.length) return null;
  return <div class="meta">{out.map(([text, cls]) => <span class={`pill ${cls}`} key={text}>{text}</span>)}</div>;
}

/** The text a card shows under its title, in the board's words. */
export function bodyOf(r: Request, menu: MenuData | null): string {
  const task = r.task;
  if (!task) return r.msg;
  if (requestType(task) === 'question') return questionText(task, menu) || t('req.questionNoText');
  if (r.type === 'plan') return task.msg.trim() && !boardPlaceholder(task.msg) ? task.msg : '';
  return (task.msg.trim() && !boardPlaceholder(task.msg) ? task.msg : '') || task.board_label || '';
}

export function Card(props: { r: Request; inSheet?: boolean }): JSX.Element {
  const r = props.r;
  const plan = planOf(r);
  const task = r.task;
  const status = task ? task.status : r.type === 'runner-alert' ? 'alarm' : 'waiting';
  const shown = task ? screens.value[task.id] : undefined;
  const menu: MenuData | null = shown && !shown.error && shown.menu ? shown.menu : null;
  const text = bodyOf(r, menu);
  const open = (): void => { if (task) sheet.value = { kind: 'task', id: task.id, queue: r.key }; };
  return (
    <article class={`card s-${status}`} tabIndex={-1} data-card={r.key} data-type={r.type}>
      <div class="chead">
        <span class="tag">{r.tag}</span>
        {r.why && <span class="why">{r.why}</span>}
        <span class="repo">{r.repo}</span>
      </div>
      <div class="ctitle">{task ? task.title || r.title : r.title}</div>
      {task && plan.block !== 'pick' && <div class="cwhat">{r.title}</div>}
      {task && <Pills task={task} />}
      {plan.block === 'dialog' ? <CoordinatorMenu />
        : plan.block === 'options' ? <Options r={r} />
        : task && plan.screen ? <Asking task={task} plan={plan} text={text} />
        : <Fold text={text} />}
      {task && plan.block === 'pick' && <Picks task={task} />}
      <Actions r={r} plan={plan} />
      {plan.note && <div class="hintline">{t(plan.note)}</div>}
      {!props.inSheet && task && <button class="linkbtn" type="button" data-act="open-task" data-id={task.id} onClick={open}>{t('m.details')}</button>}
    </article>
  );
}

/** The Coordinator's own choices: a tap says the option as the next message of the chat. */
function Options(props: { r: Request }): JSX.Element {
  const [busy, setBusy] = useState(false);
  return (
    <>
      <Fold text={props.r.msg} />
      <div class="opts">
        {(props.r.options ?? []).map((o) => (
          <button
            class="btn opt" type="button" key={o.label} data-act="variant" aria-disabled={busy}
            onClick={() => { const done = prepareAnswer(props.r.id); setBusy(true); void send(o.value).then((ok) => { if (ok) void done(); }).finally(() => setBusy(false)); }}
          ><b>{o.label}</b><span class="optx">{o.value}</span></button>
        ))}
      </div>
    </>
  );
}

function Actions(props: { r: Request; plan: CardPlan }): JSX.Element | null {
  const { r, plan } = props;
  const task = r.task;
  if (!task) return null;
  const remarks = plan.block === 'plan' || plan.block === 'approval';
  const currentSheet = sheet.value;
  const queueOrigin = currentSheet ? ('queue' in currentSheet ? currentSheet.queue : undefined) : r.key;
  return (
    <>
      {(plan.approve || plan.retry || plan.plan || plan.pr) && (
        <div class="acts">
          {plan.plan && <button class="btn" type="button" data-act="plan-open" data-id={task.id} onClick={() => { sheet.value = { kind: 'plan', id: task.id, queue: queueOrigin }; }}>{t('req.planOpen')}</button>}
          {plan.approve && plan.block === 'plan' && <ArmBtn task={task} cmd="approve" label={t('req.approvePlan')} armLabel={t('m.again')} />}
          {plan.approve && plan.block === 'approval' && <ArmBtn task={task} cmd="approve" label={t('m.approveMerge')} armLabel={t('m.again')} />}
          {plan.retry && <button class="btn go" type="button" data-act="retry" data-id={task.id} aria-disabled={!boardOk()} onClick={() => runTaskAction('retry', task)}>{plan.block === 'limit' ? t('req.continue') : t('act.retry')}</button>}
          {plan.pr && <a class="btn" href={plan.pr} target="_blank" rel="noopener noreferrer" data-act="pr">{t('m.openPr')}<Gl n="ext" /></a>}
        </div>
      )}
      {plan.tell && (
        <Tell
          task={task} id={r.key}
          label={remarks ? t('req.returnWithNotes') : plan.block === 'failed' ? t('m.writeTask') : t('m.sendAnswer')}
          placeholder={remarks ? t('m.remarks') : plan.block === 'failed' ? t('m.advice') : t('m.ownAnswer')}
          rows={remarks ? 3 : 2} go={!remarks && plan.block !== 'failed'}
        />
      )}
    </>
  );
}

export function WaitPage(): JSX.Element {
  useEffect(() => onQueueAnswer((r, order) => {
    const origin = sheet.value;
    const page = tab.value;
    if (listTab.value !== 'wait' || (origin && (!('queue' in origin) || !origin.queue))) return;
    return () => {
      if (sheet.value !== origin || tab.value !== page) return;
      if (origin) sheet.value = null;
      const next = nextRequest(order, r.key, listed(requests.value));
      requestAnimationFrame(() => {
        if (sheet.value || tab.value !== page || listTab.value !== 'wait') return;
        const el = [...document.querySelectorAll<HTMLElement>('#p-wait [data-card]')].find((el) => el.dataset.card === next?.key)
          ?? document.querySelector<HTMLElement>('#p-wait .empty');
        el?.focus({ preventScroll: true });
        el?.scrollIntoView({ block: 'start', behavior: 'instant' });
      });
    };
  }), []);
  const list = listed(requests.value);
  return (
    <section class="page" id="p-wait" aria-label={t('wait.title')}>
      {list.length > 0 && <div class="sect">{t('wait.title')} · {list.length}<i /></div>}
      {list.map((r) => <Card key={r.key} r={r} />)}
      {list.length === 0 && (
        <div class="empty" tabIndex={-1} role="status"><b>{t('wait.done')}</b>{t('m.nothingSub')}</div>
      )}
    </section>
  );
}

