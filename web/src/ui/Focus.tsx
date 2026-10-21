import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import {
  currentView, details, galaxies, galaxyId, linkSel, panels, requests, screens, selected, snap, taskOf,
} from '../store';
import { api } from '../api';
import { farScreen, farYazi, openFiles, removeLink, archiveAllDone, fetchScreen } from '../actions';
import { fetchDetails, navigateTo, getWorld } from '../nav';
import { closing, loadClosing } from '../closing';
import { addDialogs } from '../dialogs';
import { t } from '../i18n';
import { Gl, Badge } from './Gl';
import { RichText } from './RichText';
import { Translatable } from './Translatable';
import { RequestBlock, Screen } from './Request';
import { TaskActionButtons, TellBox } from './AgentActions';
import { dayTimeOf } from '../util/dates';
import { duration } from '../util/format';
import type { Entity, Subagent, Task } from '../types';
import { UniverseFocus, QuasarFocus, GalaxyFocus, ArchiveFocus } from './FocusOther';
import './focus.css';

const linkLabel = (l: { from: string; label?: string; kind: string }, galaxy: string): string => l.label || (l.kind === 'far' ? l.from : taskOf(l.from, galaxy)?.tag ?? l.from);
const remoteLink = (l: { from: string; kind: string }): boolean => l.kind === 'far' && String(l.from).includes(':');

function SubagentRow(props: { task: Task; sa: Subagent }): JSX.Element {
  const sa = props.sa;
  const dur = duration(Math.max(0, (sa.state === 'done' ? sa.last_at_ms : Date.now()) - sa.started_at_ms), t);
  return (
    <div
      class={`moonl s-${sa.state}`} data-sub={sa.id} tabIndex={0} role="button"
      onClick={() => void navigateTo({ kind: 'moon', galaxy: galaxyId.value, id: `${props.task.id}#${sa.id}` })}
      onKeyDown={(e) => { if (e.key === 'Enter') void navigateTo({ kind: 'moon', galaxy: galaxyId.value, id: `${props.task.id}#${sa.id}` }); }}
    >
      <span class="dotc" />
      <span class="role">{sa.role}</span>
      <span class="doing">{sa.doing || sa.title}</span>
      <span class="when">{sa.state === 'running' ? t('moon.running', { d: dur }) : sa.state === 'quiet' ? t('moon.quiet', { d: dur }) : t('moon.done', { d: dur })}</span>
    </div>
  );
}

function TaskFocus(props: { task: Task }): JSX.Element {
  const task = props.task;
  const d = details.value[task.id];
  const home = galaxyId.value === '';
  const local = ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(location.hostname);
  const review = d?.review && typeof d.review === 'object' ? d.review : task.review;
  const checks: string[] = Array.isArray(review?.checks) ? review.checks : [];
  const verdict = review?.tally ? t('focus.reviewTally', { v: review.tally }) : review?.text ? t('focus.review') : '';
  const worktree = d?.worktree || task.worktree || '';
  const attention = ['waiting', 'failed', 'ready', 'plan'].includes(task.status);
  const deps = currentView.value.links.filter((l) => l.to === task.id);
  const cl = closing.value[task.id];
  const moons = task.subagents.filter((s) => s.state !== 'done' || Date.now() - s.last_at_ms < 5 * 60000);
  const agentLabel = [task.app, task.model || task.worker_agent.split('/').slice(1).join('/')].filter(Boolean).join(' · ');
  const linkCard = linkSel.value && linkSel.value.to === task.id ? linkSel.value : null;

  useEffect(() => { void fetchDetails(task.id); void loadClosing(task); }, [task.id, task.status, task.msg]);

  return (
    <div class="fbody" data-focus="task">
      <div class="fhd">
        <span class="lbl">{t('focus.title')} · <span class="tagk">{task.tag}</span></span>
        <Badge status={task.status} label={task.label || t(`status.${task.status}`)} />
        <span class="agent">{agentLabel}</span>
      </div>
      <h2>{task.title}</h2>
      <p class="line"><Translatable text={task.msg || task.board_label} /></p>
      {linkCard && (
        <div class="req s-ready">
          <div class="rh"><Gl n="link" />{t('focus.link')}</div>
          <p class="full">{t('focus.linkText', { a: task.tag, b: linkLabel(linkCard, galaxyId.value) })}</p>
          <div class="row">
            {remoteLink(linkCard) ? <span class="hint">{t('focus.linkRemote')}</span>
              : <button class="btn danger" type="button" data-act="after-remove" data-id={task.id} data-from={linkCard.from} onClick={() => { void removeLink(task.id, linkCard.from, galaxyId.value); linkSel.value = null; }}>{t('focus.unlink')}</button>}
          </div>
        </div>
      )}
      <RequestBlock task={task} />
      <div class="kv">
        {d?.branch && <><span>{t('focus.branch')}</span><b>{d.branch}</b></>}
        <span>{t('focus.worktree')}</span><b>{worktree || '—'}</b>
        {(d?.agent_model || task.worker_agent) && <><span>{t('focus.model')}</span><b>{d?.agent_model || task.worker_agent}</b></>}
        <span>{t('focus.stage')}</span><b>{task.stage || task.state || '—'}</b>
        {task.created > 0 && <><span>{t('focus.created')}</span><b>{dayTimeOf(task.created)}</b></>}
      </div>
      <div class="f-actions">
        {!attention && <TaskActionButtons task={task} />}
        {home && task.status !== 'queued' && (
          <>
            <button class="btn" type="button" data-act="files" data-id={task.id} onClick={() => void openFiles(task, 'worktree')}><Gl n="folder" />{t('focus.worktreeBtn')}</button>
            <button class="btn" type="button" data-act="reports" data-id={task.id} onClick={() => void openFiles(task, 'reports')}><Gl n="folder" />{t('focus.reports')}</button>
            <button class="btn" type="button" data-act="screen" data-target={task.id} onClick={() => void fetchScreen(task.id)}><Gl n="screen" />{t('focus.agentScreen')}</button>
          </>
        )}
      </div>
      {!home && task.status !== 'queued' && (
        <div class="f-actions">
          <button class="btn" type="button" data-act="far-screen" data-id={task.id} onClick={() => void farScreen(task)}><Gl n="screen" />{t('focus.agentScreen')}</button>
          <button class="btn" type="button" data-act="far-files" data-id={task.id} disabled={!task.worktree} onClick={() => void farYazi(task)}><Gl n="folder" />{t('far.files')}</button>
        </div>
      )}
      {home && !local && <div class="remote">{t('focus.remoteFiles', { path: worktree || d?.reports_dir || t('focus.none') })}</div>}
      {home && screens.value[task.id] && <Screen target={task.id} label={t('focus.agentScreen')} />}
      <TellBox task={task} />

      {(task.status === 'done' || cl?.text) && (
        <section class="closing" id="closing">
          <div class="block-hd">
            <h3>{t('focus.closing')}</h3>
            {cl?.text && (
              <button
                class="btn sm" type="button" data-act="summary-open" data-part="closing" data-id={task.id}
                aria-label={t('focus.expand')}
                onClick={() => { addDialogs.summary.value = { task: task.id, galaxy: galaxyId.value, part: 'closing' }; }}
              ><Gl n="expand" />{t('focus.expand')}</button>
            )}
          </div>
          {cl?.text ? (
            <div class="closing-text"><Translatable text={cl.text} /></div>
          ) : <div class="hint">{task.status === 'done' ? t('focus.closingNone') : t('focus.closingLoading')}</div>}
          {cl?.truncated && <div class="hint">{t('focus.closingCut')}</div>}
        </section>
      )}

      {(moons.length > 0 || task.subagents_note) && (
        <section class="moons">
          <h3>{t('focus.subagents')} {moons.length > 0 && <span class="cnt">{moons.length}</span>}</h3>
          {moons.map((sa) => <SubagentRow key={sa.id} task={task} sa={sa} />)}
          {task.subagents_note && moons.length === 0 && <div class="hint">{task.subagents_note}</div>}
        </section>
      )}

      {task.description && (
        <section>
          <div class="block-hd">
            <h3>{t('focus.description')}</h3>
            <button
              class="btn sm" type="button" data-act="summary-open" data-part="description" data-id={task.id}
              aria-label={t('focus.expand')}
              onClick={() => { addDialogs.summary.value = { task: task.id, galaxy: galaxyId.value, part: 'description' }; }}
            ><Gl n="expand" />{t('focus.expand')}</button>
          </div>
          <div class="desc"><Translatable text={task.description} /></div>
        </section>
      )}

      <h3>{t('focus.links')}</h3>
      {deps.length ? deps.map((l) => (
        <div class="row" key={l.from}>
          <span class="hint">{linkLabel(l, galaxyId.value)}</span>
          <button class="btn icon sm" type="button" data-act="after-remove" data-id={task.id} data-from={l.from} title={t('focus.unlink')} aria-label={t('focus.unlink')} onClick={() => void removeLink(task.id, l.from, galaxyId.value)}><Gl n="close" /></button>
        </div>
      )) : <div class="hint">{t('focus.noLinks')}</div>}

      {verdict && <h3>{verdict}</h3>}
      {review?.text ? <p class="review"><Translatable text={review.text} /></p>
        : (task.review_failed ? <><h3>{t('focus.review')}</h3><p>{t('focus.reviewFailed')}</p></> : null)}
      {checks.length > 0 && <><h3>{t('focus.checks')}</h3>{checks.map((c, i) => <div class="check" key={i}><RichText text={c} /></div>)}</>}
      {d?.failure && <><h3>{t('focus.failure')}</h3><p class="review"><Translatable text={failureText(d.failure)} /></p></>}
      {d?.plan?.text && <><h3>{t('focus.plan')}</h3><p class="review"><Translatable text={d.plan.text} /></p></>}
      {(d?.failure?.next || d?.review?.next) && <><h3>{t('focus.next')}</h3><p><RichText text={d?.failure?.next || d?.review?.next} /></p></>}
      {home && !d && <div class="hint">{t('focus.detailsLoading')}</div>}
      {d?.report && (
        <div class="row">
          <button class="btn" type="button" data-act="report" onClick={() => { const url = String(d.report.url || d.report); if (/^https?:\/\//.test(url)) window.open(url, '_blank', 'noopener'); }}><Gl n="ext" />{t('req.report')}</button>
        </div>
      )}
    </div>
  );
}

function failureText(f: any): string {
  if (!f) return '';
  if (typeof f === 'string') return f;
  return f.text || f.message || f.error || f.detail || f.summary || JSON.stringify(f);
}

function MoonFocus(props: { entity: Entity }): JSX.Element {
  const [taskId, sid] = props.entity.id.split('#');
  const s = snap.value;
  const task = taskId === 'quasar' ? null : taskOf(taskId, props.entity.galaxy);
  const list = task ? task.subagents : s?.coordinator.subagents ?? [];
  const sa = list.find((x) => x.id === sid);
  const card = fullCards.value[props.entity.id];
  useEffect(() => {
    if (!sa || props.entity.galaxy !== '') return;
    void (async () => {
      try {
        const r = await api<{ subagent: Subagent }>(`/api/subagent?task=${encodeURIComponent(taskId)}&id=${encodeURIComponent(sid)}`);
        fullCards.value = { ...fullCards.value, [props.entity.id]: r.subagent };
      } catch { /* the snapshot's copy stays */ }
    })();
  }, [props.entity.id, sa?.state, sa?.last_at_ms]);
  if (!sa) return <div class="fbody"><h2>{t('moon.gone')}</h2><p class="hint">{t('moon.goneHint')}</p></div>;
  const shown = card ?? sa;
  const dur = duration(Math.max(0, (sa.state === 'done' ? sa.last_at_ms : Date.now()) - sa.started_at_ms), t);
  return (
    <div class="fbody" data-focus="moon">
      <div class="fhd">
        <span class="lbl">{t('moon.title')}</span>
        <span class={`badge s-${sa.state}`}><Gl n="moon" />{t(`moon.${sa.state}.short`)}</span>
        <span class="agent">{sa.kind}</span>
      </div>
      <h2>{sa.role}</h2>
      {sa.title && <p class="line">{sa.title}</p>}
      <div class="kv">
        <span>{t('moon.parent')}</span><b>{task ? `${task.tag} ${task.title}` : t('quasar.label')}</b>
        <span>{sa.state === 'done' ? t('moon.took') : t('moon.duration')}</span><b>{dur}</b>
        <span>{t('moon.started')}</span><b>{dayTimeOf(sa.started_at_ms)}</b>
      </div>
      <h3>{t('moon.doing')}</h3>
      <div class="desc">{sa.doing || t('moon.noDoing')}</div>
      <h3>{t('moon.result')}</h3>
      {shown.result ? <div class="closing-text"><Translatable text={shown.result} /></div> : <div class="hint">{sa.state === 'running' ? t('moon.resultLater') : t('moon.resultNone')}</div>}
      {sa.note && <div class="hint">{sa.note}</div>}
      <div class="row">
        {task && <button class="btn" type="button" onClick={() => void navigateTo({ kind: 'planet', galaxy: props.entity.galaxy, id: task.id })}>{t('moon.toParent')}</button>}
      </div>
    </div>
  );
}
const fullCards = signal<Record<string, Subagent>>({});

export function Focus(): JSX.Element {
  const e = selected.value;
  const open = panels.focus.value;
  let body: JSX.Element;
  if (panels.archive.value) body = <ArchiveFocus />;
  else if (!e) body = <Welcome />;
  else if (e.kind === 'planet') {
    const task = taskOf(e.id, e.galaxy);
    body = task ? <TaskFocus task={task} /> : <div class="fbody"><h2>{t('focus.gone')}</h2></div>;
  } else if (e.kind === 'universe') body = <UniverseFocus entity={e} />;
  else if (e.kind === 'quasar') body = <QuasarFocus />;
  else if (e.kind === 'moon') body = <MoonFocus entity={e} />;
  else body = <GalaxyFocus entity={e} />;
  return (
    <aside class={`panel focus ${open ? '' : 'off'}`} id="focus" aria-label={t('focus.aria')} aria-hidden={!open}>
      <header>
        <span>{t('focus.title')} <kbd>F</kbd></span>
        <button class="btn icon sm" type="button" id="fclose" data-act="focus-close" aria-label={t('focus.close')} onClick={() => { panels.focus.value = false; }}><Gl n="close" /></button>
      </header>
      <div class="fscroll">{body}</div>
    </aside>
  );
}

function Welcome(): JSX.Element {
  const s = snap.value;
  const waits = requests.value.length;
  return (
    <div class="fbody" data-focus="none">
      <h2>{t('focus.welcome')}</h2>
      <p>{t('focus.welcomeText')}</p>
      <div class="kv">
        <span>{t('focus.universes')}</span><b>{s?.summary.active_projects ?? 0}</b>
        <span>{t('focus.tasks')}</span><b>{s?.summary.tasks ?? 0}</b>
        <span>{t('focus.waiting')}</span><b>{waits}</b>
      </div>
      <p class="hint">{t('focus.welcomeKeys')}</p>
    </div>
  );
}

void archiveAllDone; void galaxies; void getWorld;
