import type { JSX } from 'preact';
import {
  armed, attention, currentView, galaxies, galaxyId, linkStatus, panels, requests, snap, taskOf,
} from '../store';
import { archiveAllDone, compactCoordinator, confirmStep, newCoordinator, removeLink, runTaskAction, sendAction } from '../actions';
import { enterGalaxy, goHome, navigateTo, openRequest } from '../nav';
import { t } from '../i18n';
import { dayTimeOf } from '../util/dates';
import { openStation } from './station-api';
import { Gl } from './Gl';
import { RichText } from './RichText';
import { Translatable } from './Translatable';
import { toast } from '../toast';
import { STATUS_ORDER } from '../types';
import type { Entity } from '../types';
import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { api, post } from '../api';
import { hostKind } from '../util/format';

export function contextLevel(p: number): 'ok' | 'warn' | 'crit' {
  return p > 90 ? 'crit' : p > 70 ? 'warn' : 'ok';
}

/** The Coordinator's memory: the context filled, and the two buttons that clear room. */
export function Memory(props: { compact?: boolean } = {}): JSX.Element {
  const c = snap.value?.coordinator;
  const far = galaxyId.value !== '';
  const ctx = c?.context;
  const pct = ctx?.known ? Math.round(ctx.percent) : 0;
  const level = contextLevel(pct);
  const compactOn = armed.value?.key === 'coordinator:compact';
  const newOn = armed.value?.key === 'coordinator:new';
  const canCompact = far ? true : !!c?.can_compact;
  return (
    <div class={`memory ${level} ${props.compact ? 'compact' : ''}`} id={props.compact ? 'memory-c' : 'memory'}>
      <div class="mhead">
        <span>{t('memory.title')}</span>
        <b>{far ? '—' : ctx?.known ? `${pct} %` : t('memory.unknown')}</b>
      </div>
      <div class="bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={t('memory.title')}><i style={`--w:${pct}%`} /></div>
      {ctx?.known && !far && <div class="hint">{ctx.used.toLocaleString()} / {ctx.window.toLocaleString()} · {ctx.model}</div>}
      {level !== 'ok' && !far && <div class={`hint ${level === 'crit' ? 'bad' : 'warn'}`}>{t('memory.time')}</div>}
      <div class="row">
        <button
          class={`btn ${compactOn ? 'arm' : ''}`} type="button" id="compact" data-act="compact" disabled={!canCompact}
          title={canCompact ? t('memory.compactTip') : t('memory.compactOff')}
          onClick={() => (far ? confirmStep('coordinator:compact', async () => { try { await sendAction('coordinator-compact', { confirm: true }); toast(t('toast.compacting')); } catch { /* shown */ } }) : compactCoordinator())}
        >{compactOn ? t('memory.confirm') : t('memory.compact')}</button>
        <button
          class={`btn danger ${newOn ? 'arm' : ''}`} type="button" id="newcoord" data-act="new-coordinator" disabled={far}
          title={far ? t('memory.newFar') : t('memory.newTip')}
          onClick={() => newCoordinator()}
        >{newOn ? t('memory.confirm') : t('memory.new')}</button>
      </div>
    </div>
  );
}

export function QuasarFocus(): JSX.Element {
  const s = snap.value;
  const far = galaxyId.value !== '';
  const g = far ? galaxies.value.find((x) => x.label === galaxyId.value) : null;
  const c = s?.coordinator;
  const list = requests.value.filter((r) => r.type !== 'coordinator-options');
  const moons = c?.subagents ?? [];
  return (
    <div class="fbody" data-focus="quasar">
      <div class="fhd">
        <span class="lbl">{t('quasar.label')}</span>
        <span class={`badge s-${c?.busy ? 'working' : c?.dialog ? 'waiting' : 'queued'}`}><Gl n="quasar" />{far ? (g?.coordinator.status || '—') : (c?.status || '—')}</span>
      </div>
      <h2>{t('quasar.voice')}</h2>
      <p class="hint">{far ? t('quasar.farText') : (c?.status_line ?? '')}</p>
      <Memory />
      <h3>{t('quasar.waits', { n: list.length })}</h3>
      {list.length === 0 ? <div class="hint">{t('wait.none')}</div> : list.map((r, i) => (
        <button class="qrow" type="button" key={r.key} onClick={() => void openRequest(i)}>
          <Gl n={r.task?.status ?? 'waiting'} /><b>{r.tag}</b><span>{r.text || r.title}</span>
        </button>
      ))}
      {!far && (
        <>
          <h3>{t('focus.subagents')} {moons.length > 0 && <span class="cnt">{moons.length}</span>}</h3>
          {moons.map((m) => (
            <div class={`moonl s-${m.state}`} key={m.id} role="button" tabIndex={0} onClick={() => void navigateTo({ kind: 'moon', galaxy: '', id: `quasar#${m.id}` })}>
              <span class="dotc" /><span class="role">{m.role}</span><span class="doing">{m.doing || m.title}</span>
            </div>
          ))}
          {moons.length === 0 && <div class="hint">{c?.subagents_note || t('quasar.noMoons')}</div>}
          <div class="row"><button class="btn" type="button" onClick={() => { panels.limits.value = true; }}><Gl n="limit" />{t('limits.title')}</button></div>
        </>
      )}
      {far && g && g.coordinator.reply && (
        <><h3>{t('quasar.lastWords')}</h3><div class="desc"><Translatable text={g.coordinator.reply} /></div></>
      )}
    </div>
  );
}

interface UInfo { origin: string | null; upstream: string | null; upstream_behind: number | null; settings: any; host: { host: string; kind: unknown; path: string } | null }
const infos = signal<Record<string, UInfo | null>>({});
const prOpen = signal<string | null>(null);
const prTitle = signal('');

/** `research.only` from `show --repo --json`: settings come as an object or as a list of {name, value}. */
const researchOnly = (s: any): boolean => {
  const st = s?.settings;
  const v = Array.isArray(st) ? st.find((x: any) => x?.name === 'research.only')?.value : st?.['research.only'] ?? s?.['research.only'];
  return v === true || v === 'true' || v?.value === true || v?.value === 'true';
};

async function loadInfo(repo: string): Promise<void> {
  try {
    const r = await api<UInfo & { ok: boolean }>('/api/universe/info?repo=' + encodeURIComponent(repo));
    infos.value = { ...infos.value, [repo]: r };
  } catch { infos.value = { ...infos.value, [repo]: null }; }
}

async function forkOp(repo: string, op: 'pull' | 'pr' | 'fork', extra: Record<string, unknown> = {}): Promise<void> {
  try {
    const r = await post<{ url?: string }>('/api/universe/fork', { repo, op, confirm: true, ...extra });
    toast(op === 'pr' ? t('fork.prDone', { url: r.url ?? '' }) : op === 'pull' ? t('fork.pullDone') : t('fork.forkDone'));
    void loadInfo(repo);
  } catch (e) { toast((e as Error).message, { icon: 'failed' }); }
}

function ForkBlock(props: { repo: string }): JSX.Element | null {
  const i = infos.value[props.repo];
  if (!i) return null;
  const isGitlab = hostKind(i.host?.kind).startsWith('gitlab');
  const study = researchOnly(i.settings);
  const fork = !!i.upstream;
  const up = i.upstream ? i.upstream.replace(/^[a-z+]+:\/\//i, '').replace(/^[^@/]+@/, '').replace(/\.git$/, '') : '';
  const behind = i.upstream_behind ?? 0;
  if (!fork && !study) return null;
  const k = (op: string): string => `fork:${props.repo}:${op}`;
  return (
    <section class="forkblock" id="forkblock">
      <h3>{fork ? t('fork.title') : t('fork.study')}</h3>
      {fork && <div class="kv"><span>{t('fork.of')}</span><b>{up}</b><span>upstream</span><b>{behind > 0 ? `+${behind}` : t('universe.level')}</b></div>}
      {study && <div class="hint">{t('fork.studyNote')}</div>}
      {isGitlab && <div class="hint">{t('fork.gitlab')}</div>}
      <div class="row">
        {fork && behind > 0 && (
          <button class={`btn ${armed.value?.key === k('pull') ? 'arm' : ''}`} type="button" id="fork-pull" onClick={() => confirmStep(k('pull'), () => forkOp(props.repo, 'pull'))}>
            {armed.value?.key === k('pull') ? t('memory.confirm') : t('fork.pull')}
          </button>
        )}
        {fork && (
          <button class="btn" type="button" id="fork-pr" onClick={() => { prOpen.value = prOpen.value === props.repo ? null : props.repo; }}>
            {isGitlab ? t('fork.mr') : t('fork.pr')}
          </button>
        )}
        {!fork && study && (
          <button class={`btn ${armed.value?.key === k('fork') ? 'arm' : ''}`} type="button" id="fork-make" onClick={() => confirmStep(k('fork'), () => forkOp(props.repo, 'fork'))}>
            {armed.value?.key === k('fork') ? t('memory.confirm') : t('fork.make')}
          </button>
        )}
      </div>
      {prOpen.value === props.repo && (
        <div class="row">
          <input class="field" id="pr-title" placeholder={t('fork.prTitle')} value={prTitle.value} onInput={(e) => { prTitle.value = (e.target as HTMLInputElement).value; }} />
          <button class={`btn primary ${armed.value?.key === k('pr') ? 'arm' : ''}`} type="button" id="pr-go" disabled={!prTitle.value.trim()}
            onClick={() => confirmStep(k('pr'), () => forkOp(props.repo, 'pr', { title: prTitle.value.trim() }))}>
            {armed.value?.key === k('pr') ? t('memory.confirm') : t('fork.prGo')}
          </button>
        </div>
      )}
    </section>
  );
}

export function UniverseFocus(props: { entity: Entity }): JSX.Element {
  useEffect(() => { if (galaxyId.value === '') void loadInfo(props.entity.id); }, [props.entity.id]);
  const view = currentView.value;
  const p = view.projects.find((x) => x.name === props.entity.id);
  const tasks = view.tasks.filter((x) => x.repo === props.entity.id);
  if (!p) return <div class="fbody"><h2>{props.entity.id}</h2><p class="hint">{t('universe.gone')}</p></div>;
  const counts = STATUS_ORDER.map((s) => [s, tasks.filter((x) => x.status === s).length] as const).filter(([, n]) => n > 0);
  const lag = p.lag;
  const here = galaxyId.value === '' ? snap.value?.host ?? '' : galaxyId.value;
  const key = `suggest:${p.name}`;
  const on = armed.value?.key === key;
  const behind = lag?.behind ?? 0;
  const suggest = (): void => {
    confirmStep(key, async () => {
      try {
        await sendAction('suggest-update', { repo: p.name, behind });
        toast(t('universe.suggested'));
      } catch { /* shown */ }
    });
  };
  const cmd = lag ? `git -C ${p.root || t('universe.folderPh')} pull --ff-only` : '';
  return (
    <div class="fbody" data-focus="universe">
      <div class="fhd">
        <span class="lbl">{t('universe.title')} · <span class="tagk">{p.key}</span></span>
        {behind > 0 && <span class="chip hot">main −{behind}</span>}
      </div>
      <h2>{p.name}</h2>
      <div class="kv">
        {p.root && <><span>{t('universe.folder')}</span><b title={p.root}>{p.root}</b></>}
        {p.origin && <><span>origin</span><b title={p.origin}>{p.origin}</b></>}
        <span>{t('universe.tasks')}</span><b>{tasks.length} · {t('universe.live', { n: tasks.filter((x) => x.status !== 'done').length })}</b>
      </div>
      <div class="chips">{counts.map(([s, n]) => <span class={`badge s-${s}`} key={s}><Gl n={s} />{t(`status.${s}`)} {n}</span>)}</div>
      {lag && (
        <section class="lag">
          <h3>{t('universe.lag')}</h3>
          <table class="lagt">
            <thead><tr><th>{t('universe.clone')}</th><th>main</th><th>{t('universe.state')}</th></tr></thead>
            <tbody>
              <tr>
                <td>{here}</td><td>{lag.main}</td>
                <td>{!lag.known ? t('universe.lagUnknown') : lag.behind === 0 ? t('universe.level') : t('universe.behind', { n: lag.behind })}{lag.ahead > 0 ? ` · +${lag.ahead}` : ''}</td>
              </tr>
              {lag.vs.map((v) => (
                <tr key={v.galaxy}>
                  <td>{v.galaxy || here}</td><td>—</td>
                  <td>{v.state === 'same' ? t('universe.level') : v.state === 'behind' ? t('universe.behind', { n: v.behind }) : v.state === 'ahead' ? t('universe.ahead', { n: v.ahead }) : v.state === 'diverged' ? t('universe.diverged') : t('universe.aheadOrDiverged')}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {lag.as_of_ms > 0 && <div class="hint">{t('universe.asOf', { when: dayTimeOf(lag.as_of_ms) })}</div>}
          {behind > 0 && (
            <div class="row">
              <button class={`btn ${on ? 'arm' : ''}`} type="button" id="suggest-update" data-act="suggest-update" onClick={suggest}>{on ? t('universe.confirmSuggest') : t('universe.suggest')}</button>
              <code class="cmd" title={t('universe.copy')} onClick={() => { void navigator.clipboard?.writeText(cmd); toast(t('toast.copied')); }}>{cmd}</code>
            </div>
          )}
          <div class="hint">{t('universe.noSilent')}</div>
        </section>
      )}
      {galaxyId.value === '' && <ForkBlock repo={p.name} />}
    </div>
  );
}

export function GalaxyFocus(props: { entity: Entity }): JSX.Element {
  const label = props.entity.id;
  const home = label === '';
  const g = galaxies.value.find((x) => x.label === label);
  const ls = linkStatus.value;
  if (home) {
    const s = snap.value;
    return (
      <div class="fbody" data-focus="galaxy">
        <div class="fhd"><span class="lbl">{t('galaxy.title')}</span><span class="badge s-working"><Gl n="galaxy" />{t('galaxy.here')}</span></div>
        <h2>{s?.host}</h2>
        <div class="kv">
          <span>{t('focus.universes')}</span><b>{s?.summary.projects ?? 0}</b>
          <span>{t('focus.tasks')}</span><b>{s?.summary.tasks ?? 0}</b>
          <span>{t('focus.waiting')}</span><b>{attention.value}</b>
        </div>
        <p class="hint">{t('galaxy.homeHint')}</p>
        <div class="row"><button class="btn" type="button" onClick={() => void goHome()}>{t('galaxy.toCentre')}</button></div>
      </div>
    );
  }
  const why = g?.why ?? (ls.label === label && ls.state === 'offline' ? { line: ls.why ?? '', detail: ls.detail ?? [] } : null);
  const ok = !!g?.ok;
  return (
    <div class="fbody" data-focus="galaxy">
      <div class="fhd">
        <span class="lbl">{t('galaxy.title')}</span>
        <span class={`badge s-${ok ? 'working' : 'failed'}`}><Gl n="galaxy" />{ok ? t('galaxy.online') : t('galaxy.off')}</span>
      </div>
      <h2>{label}</h2>
      <div class="kv">
        <span>{t('galaxy.target')}</span><b>{g?.target ?? '—'}</b>
        {g?.version && <><span>herdr-projects</span><b>{g.version}</b></>}
        {g && <><span>{t('focus.tasks')}</span><b>{g.tasks.length}</b></>}
        {g && g.fetched_at_ms > 0 && <><span>{t('galaxy.read')}</span><b>{dayTimeOf(g.fetched_at_ms)}</b></>}
      </div>
      {!ok && why && (
        <div class="req s-failed">
          <div class="rh"><Gl n="failed" />{why.line}</div>
          <ul class="why">{(why.detail ?? []).map((d, i) => <li key={i}>{d}</li>)}</ul>
        </div>
      )}
      <div class="row">
        <button class="btn primary" type="button" id="galaxy-fly" disabled={ls.state === 'connecting'} onClick={() => void enterGalaxy(label)}>
          {ls.state === 'connecting' && ls.label === label ? t('galaxy.connecting') : ok ? t('galaxy.fly') : t('galaxy.retry')}
        </button>
        <button class="btn" type="button" id="galaxy-station" onClick={() => openStation(label)}><Gl n="universe" />{t('menu.stationOf', { g: label })}</button>
      </div>
      <p class="hint">{t('galaxy.sshHint')}</p>
    </div>
  );
}

export function ArchiveFocus(): JSX.Element {
  const s = snap.value;
  const list = s?.archived ?? [];
  return (
    <div class="fbody" data-focus="archive">
      <div class="fhd"><span class="lbl">{t('archive.title')}</span><span class="hint">{list.length}</span></div>
      <h2>{t('archive.heading')}</h2>
      <p>{t('archive.text')}</p>
      <div class="f-actions">
        <button class="btn" type="button" data-act="archive-close" onClick={() => { panels.archive.value = false; }}><Gl n="close" />{t('archive.close')}</button>
        {(s?.summary.done ?? 0) > 0 && <button class="btn" type="button" data-act="archive-done-all" onClick={() => confirmStep('archive-done-all', archiveAllDone)}>{armed.value?.key === 'archive-done-all' ? t('archive.confirmAll') : t('archive.doneAll')}</button>}
      </div>
      <div class="arc-list">
        {list.length === 0 ? <div class="arc-empty">{t('archive.empty')}</div> : list.map((tk) => (
          <div class="arc-row" key={tk.id}>
            <span class="k">{tk.tag}</span>
            <div><div class="t">{tk.title}</div><div class="m"><RichText text={tk.msg || tk.board_label} /></div></div>
            <button class="btn sm" type="button" data-act="restore" data-id={tk.id} onClick={() => runTaskAction('restore', tk)}>{t('archive.restore')}</button>
          </div>
        ))}
      </div>
    </div>
  );
}

void taskOf; void removeLink;
