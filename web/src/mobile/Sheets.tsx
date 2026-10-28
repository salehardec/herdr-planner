/* The sheets that slide up over the page: a task, a plan, the limits, the settings. */
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { armed, details, requests, snap, taskOf } from '../store';
import { confirmStep } from '../actions';
import { fetchDetails } from '../nav';
import { post } from '../api';
import { lang, setLang, t, type Lang } from '../i18n';
import { dayTimeOf } from '../util/dates';
import { Gl } from '../ui/Gl';
import { Markdown } from '../ui/Markdown';
import { LimitAgent } from '../ui/Limits';
import { toast } from '../toast';
import { safeUrl } from './cards';
import { Card } from './Wait';
import { session, setThemePref, sheet, themePref, type ThemePref } from './state';

const close = (): void => { sheet.value = null; };

function Review(props: { id: string }): JSX.Element | null {
  const task = taskOf(props.id);
  const d = details.value[props.id];
  if (!task) return null;
  const review = task.review;
  const checks: string[] = review?.checks ?? [];
  const failure = d?.failure ? (typeof d.failure === 'string' ? d.failure : d.failure.text || d.failure.message || d.failure.error || d.failure.detail || d.failure.summary || '') : '';
  return (
    <>
      {review?.text && <div class="plan"><h4>{t('focus.review')}{review.tally ? ` · ${review.tally}` : ''}</h4><Markdown text={review.text} chips /></div>}
      {!review?.text && task.review_failed && <div class="plan"><h4>{t('focus.review')}</h4>{t('m.reviewFailedNote')}</div>}
      {checks.length > 0 && <div class="plan"><h4>{t('focus.checks')}</h4><ul>{checks.map((c, i) => <li key={i}>{c}</li>)}</ul></div>}
      {failure && <div class="plan"><h4>{t('focus.failure')}</h4><Markdown text={String(failure)} /></div>}
      {!d && <div class="hintline">{t('focus.detailsLoading')}</div>}
    </>
  );
}

function TaskSheet(props: { id: string }): JSX.Element {
  const task = taskOf(props.id);
  useEffect(() => { void fetchDetails(props.id, true); }, [props.id, task?.status, task?.stage]);
  if (!task) return <p class="hintline">{t('focus.gone')}</p>;
  const req = requests.value.find((r) => r.task?.id === task.id);
  const pr = safeUrl(task.pr);
  const agent = task.worker_agent || task.app;
  return (
    <>
      <dl class="kv">
        <dt>{t('m.state')}</dt><dd><span class={`badge s-${task.status}`}>{task.label || t(`status.${task.status}`)}</span></dd>
        {agent && <><dt>{t('m.agent')}</dt><dd>{agent}{task.model ? ` · ${task.model}` : ''}</dd></>}
        {task.msg.trim() && <><dt>{t('m.message')}</dt><dd>{task.msg}</dd></>}
        {task.created > 0 && <><dt>{t('focus.created')}</dt><dd>{dayTimeOf(task.created)}</dd></>}
      </dl>
      {req && <Card r={req} inSheet />}
      <Review id={task.id} />
      {task.description && <div class="plan"><h4>{t('focus.description')}</h4><Markdown text={task.description} chips /></div>}
      {pr && <a class="btn" href={pr} target="_blank" rel="noopener noreferrer">{t('m.openPr')}<Gl n="ext" /></a>}
      <div class="hintline">{t('m.onComputer')}</div>
    </>
  );
}

function PlanSheet(props: { id: string }): JSX.Element {
  const task = taskOf(props.id);
  const d = details.value[props.id];
  useEffect(() => { void fetchDetails(props.id, true); }, [props.id]);
  const req = requests.value.find((r) => r.task?.id === props.id);
  const text: string = d?.plan?.text ?? '';
  return (
    <>
      {text ? <div class="plan"><Markdown text={text} chips /></div> : <div class="hintline">{d?.error ? d.error : t('focus.planLoading')}</div>}
      {req && task ? <Card r={req} inSheet /> : null}
    </>
  );
}

function Limits(): JSX.Element {
  const list = snap.value?.limits?.agents ?? [];
  return <div class="lim-list">{list.length ? list.map((a) => <LimitAgent key={a.id} a={a} />) : <div class="none">{t('limits.omarchyNone')}</div>}</div>;
}

function Seg<T extends string>(props: { value: T; items: [T, string][]; onPick: (v: T) => void; label: string }): JSX.Element {
  return (
    <div class="seg" role="group" aria-label={props.label}>
      {props.items.map(([v, label]) => (
        <button key={v} type="button" class={props.value === v ? 'on' : ''} aria-pressed={props.value === v} data-v={v} onClick={() => props.onPick(v)}>{label}</button>
      ))}
    </div>
  );
}

const hours = (ms: number): string => {
  const left = Math.max(0, ms - Date.now());
  const h = Math.floor(left / 3600000);
  const m = Math.floor(left / 60000) % 60;
  return h > 0 ? t('chat.elapsedHm', { h, m }) : t('dur.min', { n: Math.max(1, m) });
};

function Settings(): JSX.Element {
  const s = session.value;
  const [leaving, setLeaving] = useState(false);
  const key = 'close-gate';
  const closing = armed.value?.key === key;
  const gate = s?.kind === 'device';
  const logout = async (): Promise<void> => {
    setLeaving(true);
    try { await post('/api/logout', {}); } catch { /* the cookie is gone either way */ }
    location.replace('/m');
  };
  return (
    <>
      <div class="setrow">
        <span class="l">{t('m.thisDevice')}</span>
        <div class="plan">
          {gate ? <>{s?.label}<br /><span class="faint">{t('m.sessionTo', { at: dayTimeOf(s?.ends_ms ?? 0) })}</span></> : t('m.local')}
        </div>
      </div>
      <div class="setrow">
        <span class="l">{t('m.language')}</span>
        <Seg<Lang> label={t('m.language')} value={lang.value} items={[['ru', 'Русский'], ['en', 'English']]} onPick={setLang} />
      </div>
      <div class="setrow">
        <span class="l">{t('m.theme')}</span>
        <Seg<ThemePref> label={t('m.theme')} value={themePref.value} items={[['system', t('m.theme.system')], ['dark', t('m.theme.dark')], ['light', t('m.theme.light')]]} onPick={setThemePref} />
      </div>
      {gate && (
        <>
          <button class="btn" type="button" data-s="logout" aria-disabled={leaving} onClick={() => void logout()}>{t('m.logout')}</button>
          <button
            class={`btn danger ${closing ? 'arm' : ''}`} type="button" data-s="shut"
            onClick={() => confirmStep(key, async () => {
              try { await post('/api/tunnel/close', {}); } catch (e) { toast((e as Error).message, { icon: 'failed' }); return; }
              location.replace('/m');
            })}
          >{closing ? t('m.shutAgain') : t('m.shut')}</button>
          <div class="hintline">
            {s?.gate_closes_ms ? t('m.autoClose', { left: hours(s.gate_closes_ms) }) : ''} {t('m.cloudflare')}
          </div>
        </>
      )}
    </>
  );
}

export function Sheets(): JSX.Element {
  const sh = sheet.value;
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!sh) return;
    const onKey = (e: KeyboardEvent): void => { if (e.key === 'Escape') close(); };
    window.addEventListener('keydown', onKey);
    ref.current?.focus();
    return () => window.removeEventListener('keydown', onKey);
  }, [sh?.kind]);
  const task = sh && (sh.kind === 'task' || sh.kind === 'plan') ? taskOf(sh.id) : undefined;
  const title = !sh ? '' : sh.kind === 'limits' ? t('limits.title') : sh.kind === 'settings' ? t('m.settings')
    : sh.kind === 'plan' ? `${task?.tag ?? sh.id} · ${t('focus.plan')}` : `${task?.tag ?? sh.id} · ${task?.title ?? ''}`;
  return (
    <>
      <div class={`scrim ${sh ? 'on' : ''}`} onClick={close} aria-hidden="true" />
      <div class={`sheet ${sh ? 'on' : ''}`} role="dialog" aria-modal="true" aria-label={title} aria-hidden={!sh} tabIndex={-1} ref={ref} id="sheet">
        {sh && (
          <>
            <div class="grab" />
            <div class="sh"><b>{title}</b><button class="ibtn" type="button" data-s="close" aria-label={t('close')} onClick={close}><Gl n="close" /></button></div>
            <div class="sb">
              {sh.kind === 'task' && <TaskSheet id={sh.id} />}
              {sh.kind === 'plan' && <PlanSheet id={sh.id} />}
              {sh.kind === 'limits' && <Limits />}
              {sh.kind === 'settings' && <Settings />}
            </div>
          </>
        )}
      </div>
    </>
  );
}
