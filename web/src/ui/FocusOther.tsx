import type { JSX } from 'preact';
import { armed, currentView, panels, requests, runners, snap, taskOf } from '../store';
import { archiveAllDone, compactCoordinator, confirmStep, newCoordinator, runTaskAction, sendAction } from '../actions';
import { navigateTo, openRequest } from '../nav';
import { t, tn } from '../i18n';
import { alertText } from '../requests';
import { dayTimeOf } from '../util/dates';
import { Gl } from './Gl';
import { MemoryPanel } from './MemoryPanel';
import { CleanCard } from './CleanCard';
import { DiskCard } from './DiskCard';
import { ContainerButtons, ContainerPorts } from './ContainerTools';
import { RunnerAuto } from './RunnerAuto';
import { RunnerLog } from './RunnerLog';
import { Spark, sparkStats } from './Spark';
import { DAY_MS, HOURS_MS, cpuTime, hist, histError, loadHistory, memSize, yardUsage } from './usage-api';
import { RichText } from './RichText';
import { toast } from '../toast';
import { STATUS_ORDER } from '../types';
import type { Entity, RunnerLimits } from '../types';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { api, post } from '../api';
import { duration, hostKind } from '../util/format';

export function contextLevel(p: number): 'ok' | 'warn' | 'crit' {
  return p > 90 ? 'crit' : p > 70 ? 'warn' : 'ok';
}

/** The Coordinator's memory: the context filled, and the two buttons that clear room. */
export function Memory(props: { compact?: boolean } = {}): JSX.Element {
  const c = snap.value?.coordinator;
  const ctx = c?.context;
  const pct = ctx?.known ? Math.round(ctx.percent) : 0;
  const level = contextLevel(pct);
  const compactOn = armed.value?.key === 'coordinator:compact';
  const newOn = armed.value?.key === 'coordinator:new';
  const canCompact = !!c?.can_compact;
  return (
    <div class={`memory ${level} ${props.compact ? 'compact' : ''}`} id={props.compact ? 'memory-c' : 'memory'}>
      <div class="mhead">
        <span>{t('memory.title')}</span>
        <b>{ctx?.known ? `${pct} %` : t('memory.unknown')}</b>
      </div>
      <div class="bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label={t('memory.title')}><i style={`--w:${pct}%`} /></div>
      {ctx?.known && <div class="hint">{ctx.used.toLocaleString()} / {ctx.window.toLocaleString()} · {ctx.model}</div>}
      {level !== 'ok' && <div class={`hint ${level === 'crit' ? 'bad' : 'warn'}`}>{t('memory.time')}</div>}
      <div class="row">
        <button
          class={`btn ${compactOn ? 'arm' : ''}`} type="button" id="compact" data-act="compact" disabled={!canCompact}
          title={canCompact ? t('memory.compactTip') : t('memory.compactOff')}
          onClick={() => compactCoordinator()}
        >{compactOn ? t('memory.confirm') : t('memory.compact')}</button>
        <button
          class={`btn danger ${newOn ? 'arm' : ''}`} type="button" id="newcoord" data-act="new-coordinator"
          title={t('memory.newTip')}
          onClick={() => newCoordinator()}
        >{newOn ? t('memory.confirm') : t('memory.new')}</button>
      </div>
    </div>
  );
}

export function QuasarFocus(): JSX.Element {
  const s = snap.value;
  const c = s?.coordinator;
  const list = requests.value.filter((r) => r.type !== 'coordinator-options');
  const moons = c?.subagents ?? [];
  return (
    <div class="fbody" data-focus="quasar">
      <div class="fhd">
        <span class="lbl">{t('quasar.label')}</span>
        <span class={`badge s-${c?.busy ? 'working' : c?.dialog ? 'waiting' : 'queued'}`}><Gl n="quasar" />{c?.status || '—'}</span>
      </div>
      <h2>{t('quasar.voice')}</h2>
      <p class="hint">{c?.status_line ?? ''}</p>
      <Memory />
      <MemoryPanel repo="" />
      <h3>{t('quasar.waits', { n: list.length })}</h3>
      {list.length === 0 ? <div class="hint">{t('wait.none')}</div> : list.map((r, i) => (
        <button class="qrow" type="button" key={r.key} onClick={() => void openRequest(i)}>
          <Gl n={r.type === 'memory' ? 'memory' : r.task?.status ?? 'waiting'} /><b>{r.tag}</b><span>{r.type === 'memory' ? `${r.title}: ` : ''}{r.text || r.title}</span>
        </button>
      ))}
      <h3>{t('focus.subagents')} {moons.length > 0 && <span class="cnt">{moons.length}</span>}</h3>
      {moons.map((m) => (
        <div class={`moonl s-${m.state}`} key={m.id} role="button" tabIndex={0} onClick={() => void navigateTo({ kind: 'moon', id: `quasar#${m.id}` })}>
          <span class="dotc" /><span class="role">{m.role}</span><span class="doing">{m.doing || m.title}</span>
        </div>
      ))}
      {moons.length === 0 && <div class="hint">{c?.subagents_note || t('quasar.noMoons')}</div>}
      <div class="row"><button class="btn" type="button" onClick={() => { panels.limits.value = true; }}><Gl n="limit" />{t('limits.title')}</button></div>
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
  useEffect(() => { void loadInfo(props.entity.id); }, [props.entity.id]);
  const view = currentView.value;
  const p = view.projects.find((x) => x.name === props.entity.id);
  const tasks = view.tasks.filter((x) => x.repo === props.entity.id);
  if (!p) return <div class="fbody"><h2>{props.entity.id}</h2><p class="hint">{t('universe.gone')}</p></div>;
  const counts = STATUS_ORDER.map((s) => [s, tasks.filter((x) => x.status === s).length] as const).filter(([, n]) => n > 0);
  const lag = p.lag;
  const here = snap.value?.host ?? '';
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
      <div class="chips">{counts.map(([s, n]) => <span class={`badge s-${s}`} key={s} title={t(`legend.${s}`)}><Gl n={s} />{t(`status.${s}`)} {n}</span>)}</div>
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
      <ForkBlock repo={p.name} />
      <MemoryPanel repo={p.name} />
    </div>
  );
}

/* ───────── a shipyard (верфь) ───────── */

const GB = 1024 * 1024 * 1024;

/** An image's size as docker prints it; a volume's is unknown until the disk count is asked for. */
function sharedSize(bytes: number | null | undefined): string {
  if (bytes == null) return '—';
  // Docker prints image sizes in decimal units; the page shows the same number it shows.
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.round(bytes / 1e6)} MB`;
}

function yardGauge(label: string, pct: number, text: string): JSX.Element {
  return (
    <div class="rf-g">
      <span class="kv">{label}</span>
      <div class="bar"><i class={pct > 80 ? 'hot' : ''} style={`width:${Math.max(0, Math.min(100, pct))}%`} /></div>
      <span>{text}</span>
    </div>
  );
}

function yardAge(ms: number): string {
  return duration(Math.max(0, ms), t);
}

/** What the yard did over the hours and the day (PLN-46): three lines, a switch of the window, and the numbers. */
function YardHistory(props: { r: { id: string }; cpu: number; mem: number; disk: number }): JSX.Element {
  const [win, setWin] = useState<'hours' | 'day'>('hours');
  const data = hist.value;
  const points = data?.runners?.[props.r.id]?.points ?? [];
  // The samples are stamped in seconds; the page works in milliseconds.
  const to = data?.at ?? Date.now();
  const from = to - (win === 'hours' ? HOURS_MS : DAY_MS);
  const samples = points.map((p) => ({ at: p.at * 1000, cpu: p.cpu, mem: p.mem, disk: p.disk }));
  const live = { cpu: props.cpu, mem: props.mem, disk: props.disk };
  const row = (key: 'cpu' | 'mem' | 'disk', label: string): JSX.Element => {
    const pts = samples.map((p) => ({ at: p.at, v: p[key] }));
    const { shown, peak } = sparkStats(pts, from);
    return (
      <div class="rf-spark" data-metric={key}>
        <span class="kv">{label}</span>
        <Spark points={shown} from={from} to={to} label={t('runner.histNow', { v: Math.round(live[key]) })} cls={key} id={`rf-spark-${key}`} />
        <span class="num">
          {t('runner.histNow', { v: Math.round(live[key]) })}
          {shown.length > 0 ? ` · ${t('runner.histPeak', { v: Math.round(peak) })}` : ''}
        </span>
      </div>
    );
  };
  const visible = samples.some((p) => p.at >= from && (p.cpu != null || p.mem != null || p.disk != null));
  return (
    <section class="rf-hist" id="rf-hist" data-window={win}>
      <div class="rf-hist-hd">
        <h3>{t('runner.hist')}</h3>
        <div class="seg" role="group" aria-label={t('runner.hist')}>
          <button class={`btn sm ${win === 'hours' ? 'on' : ''}`} type="button" data-hist-window="hours" aria-pressed={win === 'hours'} onClick={() => setWin('hours')}>{t('runner.histHours')}</button>
          <button class={`btn sm ${win === 'day' ? 'on' : ''}`} type="button" data-hist-window="day" aria-pressed={win === 'day'} onClick={() => setWin('day')}>{t('runner.histDay')}</button>
        </div>
      </div>
      {histError.value && <div class="hint bad" id="rf-hist-error">{t('runner.histFailed', { e: histError.value })}</div>}
      {!visible ? <div class="hint" id="rf-hist-empty">{t('runner.histEmpty')}</div> : (
        <div class="rf-sparks">
          {row('cpu', t('runner.histCpu'))}
          {row('mem', t('runner.histMem'))}
          {row('disk', t('runner.histDisk'))}
        </div>
      )}
      <p class="hint">{t('runner.histHint')}</p>
    </section>
  );
}

export function RunnerFocus(props: { entity: Entity }): JSX.Element {
  const r = runners.value.find((x) => x.id === props.entity.id);
  const [cleanOpen, setCleanOpen] = useState(false);
  const [diskOpen, setDiskOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [connectOpen, setConnectOpen] = useState(false);
  const [warned, setWarned] = useState<string[]>([]);
  const [tags, setTags] = useState('');
  const [masks, setMasks] = useState('');
  const [ports, setPorts] = useState('');
  const [note, setNote] = useState('');
  const [alertDiskPct, setAlertDiskPct] = useState('10');
  const [alertOfflineMin, setAlertOfflineMin] = useState('10');
  const [limCpus, setLimCpus] = useState('0');
  const [limMem, setLimMem] = useState('0');
  const [limPer, setLimPer] = useState('0');
  const [chkCpus, setChkCpus] = useState('6');
  const [chkSlots, setChkSlots] = useState('2');
  const [busyPct, setBusyPct] = useState('');
  const [makeDefault, setMakeDefault] = useState(false);
  const [err, setErr] = useState('');
  // The history is read again whenever a yard has been read (PLN-46); the calls are shared by every Focus.
  const stamp = runners.value.map((x) => x.fetched_at_ms).join(',');
  useEffect(() => { void loadHistory(); }, [stamp]);
  const local = ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(location.hostname);
  if (!r) return <div class="fbody"><h2>{props.entity.id}</h2><p class="hint">{t('runner.gone')}</p></div>;
  const active = r.containers.filter((c) => c.class === 'active');
  const unsure = r.containers.filter((c) => c.class === 'unsure');
  const forgotten = r.containers.filter((c) => c.class === 'forgotten');
  const foreign = r.containers.filter((c) => c.class === 'foreign');
  const limits: RunnerLimits = r.limits ?? { cpus: 0, mem_mb: 0, per_task: 0 };
  const host = r.host;
  const cpuPct = host.cpus > 0 ? (host.load1 / host.cpus) * 100 : 0;
  const memUsed = Math.max(0, host.mem_total - host.mem_avail);
  const memPct = host.mem_total > 0 ? (memUsed / host.mem_total) * 100 : 0;
  const diskUsed = Math.max(0, host.disk_size - host.disk_avail);
  const diskPct = host.disk_size > 0 ? (diskUsed / host.disk_size) * 100 : 0;
  const usage = yardUsage(hist.value?.runners?.[r.id]?.tasks ?? [], r.id);
  const block = snap.value?.runner_block;
  const openEdit = (): void => {
    setTags(r.tags.join(', '));
    setMasks(r.protected.join(', '));
    setPorts(r.ports_host);
    setNote(r.note);
    setAlertDiskPct(String(r.alert_rules?.disk_free_pct ?? 10));
    setAlertOfflineMin(String(r.alert_rules?.offline_min ?? 10));
    setLimCpus(String(limits.cpus));
    // The form takes GiB; the file keeps MiB.
    setLimMem(String(Math.round((limits.mem_mb / 1024) * 100) / 100));
    setLimPer(String(limits.per_task));
    setChkCpus(String(r.check_cpus));
    setChkSlots(String(r.check_slots));
    setBusyPct(r.busy_pct == null ? '' : String(r.busy_pct));
    setMakeDefault(false);
    setErr('');
    setEditOpen(true);
  };
  const saveEdit = (): void => {
    // An emptied field is «leave it as it is»; 0 is the human's own «no limit».
    const cpus = limCpus.trim() === '' ? limits.cpus : Number(limCpus);
    const memGb = limMem.trim() === '' ? limits.mem_mb / 1024 : Number(limMem);
    const per = limPer.trim() === '' ? limits.per_task : Number(limPer);
    const chkC = chkCpus.trim() === '' ? r.check_cpus : Number(chkCpus);
    const chkS = chkSlots.trim() === '' ? r.check_slots : Number(chkSlots);
    const busy = busyPct.trim();
    if (!Number.isInteger(cpus) || cpus < 0 || cpus > 1024
      || !Number.isFinite(memGb) || memGb < 0 || memGb > 1024
      || !Number.isInteger(per) || per < 0 || per > 1024
      || !Number.isInteger(chkC) || chkC < 0 || chkC > 1024
      || !Number.isInteger(chkS) || chkS < 0 || chkS > 64
      || (busy !== '' && (!Number.isInteger(Number(busy)) || Number(busy) < 0 || Number(busy) > 100))) {
      setErr(t('runner.limitsBad'));
      return;
    }
    confirmStep(`rset:${r.id}`, async () => {
      setErr('');
      try {
        await post('/api/runner/set', {
          id: r.id, confirm: true,
          tags: tags.split(',').map((x) => x.trim()).filter(Boolean),
          protected: masks.split(',').map((x) => x.trim()).filter(Boolean),
          ports_host: ports.trim(), note, default: makeDefault || undefined,
          disk_free_pct: alertDiskPct.trim() === '' ? undefined : Number(alertDiskPct),
          offline_min: alertOfflineMin.trim() === '' ? undefined : Number(alertOfflineMin),
          limits: { cpus, mem_mb: Math.round(memGb * 1024), per_task: per },
          check_cpus: chkC,
          check_slots: chkS,
          busy_pct: busy === '' ? undefined : Number(busy),
          busy_reset: busy === '',
        });
        toast(t('runner.saved'));
        setEditOpen(false);
      } catch (e) {
        setErr((e as Error).message);
      }
    });
  };
  const remove = (force: boolean): void => {
    const key = `rrm:${r.id}:${force ? 'f' : 'n'}`;
    confirmStep(key, async () => {
      setErr('');
      try {
        const res = await post<{ ok: boolean; warned: string[] }>('/api/runner/remove', { id: r.id, force, confirm: true });
        if (res.warned?.length) setWarned(res.warned);
        toast(t('runner.removed', { id: r.id }));
        if (!force) setWarned([]);
        void navigateTo({ kind: 'quasar', id: 'quasar' });
      } catch (e) {
        const data = (e as { data?: { active?: string[]; error?: string } }).data;
        if (data?.active) setWarned(data.active);
        setErr(data?.error ?? (e as Error).message);
      }
    });
  };
  const connectAll = (): void => {
    confirmStep(`rcn:${r.id}`, async () => {
      setErr('');
      try {
        await post('/api/runner/connect', { on: true, drop_old: false, confirm: true });
        toast(t('runner.connectedTo', { n: 1 }));
        setConnectOpen(false);
      } catch (e) {
        setErr((e as Error).message);
      }
    });
  };
  const cleanIds = [...forgotten, ...unsure].map((c) => c.id);
  return (
    <div class="fbody" data-focus="runner">
      <div class="fhd">
        <span class="lbl">{t('runner.title')}</span>
        <span class={`badge s-${r.ok ? 'working' : 'failed'}`}><Gl n="yard" />{r.ok ? t('runner.online') : t('runner.offline')}</span>
        {r.tags.map((x) => <span class="chip" key={x}>{x}</span>)}
      </div>
      <h2><Gl n="yard" /> {r.id}</h2>
      <p class="line">{r.target} · {host.os || '—'} · {t('runner.docker', { v: host.docker_version || '—' })}{host.compose_version ? ` · compose ${host.compose_version}` : ''} · {r.ok ? t('runner.readAt', { when: dayTimeOf(r.fetched_at_ms) }) : t('runner.lastRead', { when: dayTimeOf(r.fetched_at_ms) })}</p>
      {r.note && <p class="hint">{r.note}</p>}
      {!r.ok && r.why && (
        <div class="req s-failed"><div class="rh"><Gl n="failed" />{r.why.line}</div><ul class="why">{r.why.detail.map((d, i) => <li key={i}>{d}</li>)}</ul></div>
      )}
      <section class="rf-gauges">
        {yardGauge(t('runner.cpu'), cpuPct, `${host.load1.toFixed(1)} / ${host.cpus || '—'}`)}
        {yardGauge(t('runner.mem'), memPct, `${(memUsed / GB).toFixed(1)} / ${(host.mem_total / GB).toFixed(1)} GB`)}
        {yardGauge(t('runner.disk'), diskPct, t('runner.diskFree', { n: Math.round(host.disk_avail / GB) }))}
        {host.gpu.map((g) => yardGauge(`GPU ${g.name}`, g.mem_total_mib ? ((g.mem_used_mib ?? 0) / g.mem_total_mib) * 100 : 0,
          `${(((g.mem_used_mib ?? 0) / 1024)).toFixed(1)} / ${(((g.mem_total_mib ?? 0) / 1024)).toFixed(0)} GB`))}
      </section>
      <p class="hint" id="rf-limits">
        {t('runner.limitsLine', {
          cpus: limits.cpus > 0 ? String(limits.cpus) : t('runner.noLimit'),
          mem: limits.mem_mb > 0
            ? (limits.mem_mb % 1024 === 0 ? t('runner.gb', { n: limits.mem_mb / 1024 }) : t('runner.mb', { n: limits.mem_mb }))
            : t('runner.noLimit'),
          per: limits.per_task > 0 ? tn('runner.perTask', limits.per_task, { n: limits.per_task }) : t('runner.noLimit'),
        })}
      </p>
      <p class="hint" id="rf-check-line">
        {t('runner.checkLine', {
          cpus: r.check_cpus > 0 ? String(r.check_cpus) : t('runner.noLimit'),
          slots: r.check_slots > 0 ? tn('runner.checkCount', r.check_slots, { n: r.check_slots }) : t('runner.noQueue'),
        })}
      </p>
      <p class="hint" id="rf-busy-line">
        {(r.busy_pct ?? snap.value?.runner_rules?.busy_pct ?? 85) === 0
          ? t('runner.busyLineOff')
          : t('runner.busyLine', { pct: r.busy_pct ?? snap.value?.runner_rules?.busy_pct ?? 85 })}
      </p>
      <YardHistory r={r} cpu={cpuPct} mem={memPct} disk={diskPct} />
      {r.alerts.length > 0 && (
        <section class="rf-alerts" id="rf-alerts">
          <h3>{t('runner.alerts')} <span class="cnt">{r.alerts.length}</span></h3>
          {r.alerts.map((a) => (
            <div class="rf-alert" key={a.key} data-alert={a.kind}>
              <Gl n={a.kind === 'forgotten' ? 'yard' : 'failed'} />
              <span class="tx">{alertText(a)}</span>
              {a.task && a.tag && (
                <button class="k" type="button" onClick={() => void navigateTo({ kind: 'planet', id: a.task! })}>{a.tag}</button>
              )}
            </div>
          ))}
        </section>
      )}
      <h3>{t('runner.taskContainers')} <span class="cnt">{active.length}</span></h3>
      {active.length === 0 ? <div class="hint">{t('runner.noContainers')}</div> : (
        <table class="rft" id="rf-tasks">
          <thead><tr><th>{t('runner.colContainer')}</th><th>{t('runner.colTask')}</th><th>{t('runner.colLives')}</th><th>CPU</th><th>{t('runner.colMemory')}</th><th>{t('runner.colPorts')}</th></tr></thead>
          <tbody>
            {active.map((c) => (
              <tr key={c.id} data-class={c.class}>
                <td>
                  <code>{c.name}</code>
                  {c.no_limits && <span class="chip hot" data-nolimit="1" title={t('runner.noLimitHint')}>{t('runner.noLimitMark')}</span>}
                  <div class="ctl-row"><ContainerButtons runner={r.id} c={c} /></div>
                </td>
                <td>{c.task
                  ? <button class="k" type="button" onClick={() => void navigateTo({ kind: 'planet', id: c.task!.id })}>{c.task.tag}</button>
                  : <span class="hint">{t('runner.unsureShort')}</span>}</td>
                <td>{c.created_ms ? yardAge(Date.now() - c.created_ms) : '—'}</td>
                <td>{c.cpu_pct == null ? '—' : `${c.cpu_pct.toFixed(1)} %`}</td>
                <td>{c.mem_bytes == null ? '—' : `${(c.mem_bytes / (1024 * 1024)).toFixed(0)} MB`}{c.gpu_mem_mib ? ` · GPU ${c.gpu_mem_mib} MB` : ''}</td>
                <td><ContainerPorts c={c} host={r.ports_host} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3>{t('runner.usageTitle')} <span class="cnt">{usage.length}</span></h3>
      <p class="hint">{t('runner.usageHint')}</p>
      {usage.length === 0 ? <div class="hint" id="rf-usage">{t('runner.usageNone')}</div> : (
        <table class="rft" id="rf-usage">
          <thead><tr><th>{t('runner.usageTag')}</th><th>{t('runner.usageCpu')}</th><th>{t('runner.usageMem')}</th><th>{t('runner.usageSince')}</th></tr></thead>
          <tbody>
            {usage.map((u) => (
              <tr key={u.tag} data-usage={u.tag}>
                <td>{u.task
                  ? <button class="k" type="button" onClick={() => void navigateTo({ kind: 'planet', id: u.task })}>{taskOf(u.task)?.tag ?? u.tag}</button>
                  : <span>{u.tag}</span>}</td>
                <td>{cpuTime(u.cpu_sec)}</td>
                <td>{memSize(u.mem_peak)}</td>
                <td class="rf-use-since">{u.first_ms ? dayTimeOf(u.first_ms) : '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {r.over.length > 0 && (
        <section class="rf-over" id="rf-over">
          <h3>{t('runner.overTitle')} <span class="cnt">{r.over.length}</span></h3>
          {r.over.map((o) => (
            <div class="rf-over-row" key={o.task} data-over={o.task}>
              <button class="k" type="button" onClick={() => void navigateTo({ kind: 'planet', id: o.task })}>{o.tag}</button>
              <span>{t('runner.overLine', { n: o.containers, max: o.max })}</span>
            </div>
          ))}
        </section>
      )}
      <h3>{t('runner.shared')} <span class="cnt">{r.shared.length}</span></h3>
      <p class="hint">{t('runner.sharedHint')}</p>
      {r.shared.length === 0 ? <div class="hint">{t('runner.sharedNone')}</div> : (
        <table class="rft" id="rf-shared">
          <thead><tr><th>{t('runner.sharedColRepo')}</th><th>{t('runner.sharedColItem')}</th><th>{t('runner.sharedSize')}</th></tr></thead>
          <tbody>
            {r.shared.map((s) => (
              <tr key={`${s.kind}:${s.name}`} data-shared={s.kind}>
                <td>{s.repo}</td>
                <td><span class="hint">{t(`runner.sharedKind.${s.kind}`)}</span> <code>{s.name}</code></td>
                <td>{sharedSize(s.size_bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3>{t('runner.forgotten')} <span class="cnt">{forgotten.length + unsure.length}</span></h3>
      <p class="hint">{t('runner.forgottenHint')}</p>
      {forgotten.length + unsure.length === 0 ? <div class="hint">{t('runner.forgottenNone')}</div> : (
        <table class="rft" id="rf-forgotten">
          <thead><tr><th>{t('runner.colContainer')}</th><th>{t('runner.colTask')}</th><th>{t('runner.colLives')}</th><th>{t('runner.colMemory')}</th></tr></thead>
          <tbody>
            {[...forgotten, ...unsure].map((c) => (
              <tr key={c.id} class={c.class === 'unsure' ? 'dim' : ''} data-class={c.class}>
                <td><code>{c.name}</code><div class="ctl-row"><ContainerButtons runner={r.id} c={c} /></div></td>
                <td>{c.task ? <button class="k" type="button" onClick={() => void navigateTo({ kind: 'planet', id: c.task!.id })}>{c.task.tag}</button> : <span class="hint">{t('runner.unsureShort')}</span>}</td>
                <td>{c.created_ms ? yardAge(Date.now() - c.created_ms) : '—'}</td>
                <td>{c.mem_bytes == null ? '—' : `${(c.mem_bytes / (1024 * 1024)).toFixed(0)} MB`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {forgotten.length + unsure.length > 0 && !cleanOpen && (
        <div class="act"><button class="btn danger" type="button" id="rf-clean" onClick={() => setCleanOpen(true)}>{t('runner.cleanOpen')}</button></div>
      )}
      {cleanOpen && <CleanCard runner={r.id} ids={cleanIds} onClose={() => setCleanOpen(false)} onDone={() => setCleanOpen(false)} />}
      <RunnerAuto runner={r} />
      <RunnerLog key={r.id} runner={r.id} stamp={stamp} />
      <details class="rf-foreign" id="rf-foreign">
        <summary>{t('runner.foreign', { n: foreign.length })}{foreign.length > 0 ? ` · ${t('runner.foreignHint')}` : ''}</summary>
        <div class="hint">{foreign.map((c) => c.name).join(' · ')}</div>
      </details>
      <div class="hint">
        {t('runner.blocks')}: {!block || !block.connected ? t('runner.blocksNone') : `${block.stale ? '↻' : '✓'} ${dayTimeOf(block.at)}`}
      </div>
      <div class="act">
        <button class="btn" type="button" onClick={() => void post('/api/runner/ask', { id: r.id })}><Gl n="refresh" />{t('runner.refresh')}</button>
        {local && <button class="btn" type="button" id="rf-terminal" onClick={async () => { try { await post('/api/runner/terminal', { id: r.id }); } catch (e) { toast((e as Error).message, { icon: 'failed' }); } }}><Gl n="screen" />{t('runner.terminal')}</button>}
        <button class="btn" type="button" id="rf-disk-open" onClick={() => setDiskOpen(true)}>{t('runner.df')}</button>
      </div>
      <div class="act">
        <button class="btn" type="button" id="rf-edit-open" onClick={openEdit}>{t('runner.edit')}</button>
        <button class="btn" type="button" onClick={() => setConnectOpen(!connectOpen)}>{t('runner.connect')}</button>
        <button class="btn danger" type="button" onClick={() => remove(false)}>{armed.value?.key === `rrm:${r.id}:n` ? t('runner.removeConfirm') : t('runner.remove')}</button>
      </div>
      {diskOpen && <DiskCard runner={r.id} onClose={() => setDiskOpen(false)} />}
      {err && <p class="hint bad">{err}</p>}
      {warned.length > 0 && (
        <div class="req s-failed">
          <div class="rh">{t('runner.removeActive')}</div>
          <div class="hint">{warned.join(', ')}</div>
          <button class={`btn danger ${armed.value?.key === `rrm:${r.id}:f` ? 'arm' : ''}`} type="button" onClick={() => remove(true)}>{armed.value?.key === `rrm:${r.id}:f` ? t('runner.removeConfirm') : t('runner.removeAnyway')}</button>
        </div>
      )}
      {editOpen && (
        <section class="rf-edit" id="rf-edit">
          <h3>{t('runner.editTitle')}</h3>
          <label>{t('runner.tags')}<input class="field" value={tags} onInput={(e) => setTags((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.masks')}<input class="field" value={masks} onInput={(e) => setMasks((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.portsHost')}<input class="field" value={ports} onInput={(e) => setPorts((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.note')}<input class="field" value={note} onInput={(e) => setNote((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.limitsCpus')}<input class="field" id="rf-lim-cpus" inputMode="numeric" value={limCpus} onInput={(e) => setLimCpus((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.limitsMem')}<input class="field" id="rf-lim-mem" inputMode="decimal" value={limMem} onInput={(e) => setLimMem((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.limitsPer')}<input class="field" id="rf-lim-per" inputMode="numeric" value={limPer} onInput={(e) => setLimPer((e.target as HTMLInputElement).value)} /></label>
          <p class="hint" id="rf-lim-hint">{t('runner.limitsHint')}</p>
          <label>{t('runner.checkCpus')}<input class="field" id="rf-check-cpus" inputMode="numeric" value={chkCpus} onInput={(e) => setChkCpus((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.checkSlots')}<input class="field" id="rf-check-slots" inputMode="numeric" value={chkSlots} onInput={(e) => setChkSlots((e.target as HTMLInputElement).value)} /></label>
          <p class="hint" id="rf-check-hint">{t('runner.checkHint')}</p>
          <label>{t('runner.busyPct')}<input class="field" id="rf-busy" inputMode="numeric" value={busyPct} placeholder={t('runner.busyPlaceholder', { pct: snap.value?.runner_rules?.busy_pct ?? 85 })} onInput={(e) => setBusyPct((e.target as HTMLInputElement).value)} /></label>
          <p class="hint" id="rf-busy-hint">{t('runner.busyHint')}</p>
          <label>{t('runner.diskPct')}<input class="field" id="rf-disk-pct" inputMode="numeric" value={alertDiskPct} onInput={(e) => setAlertDiskPct((e.target as HTMLInputElement).value)} /></label>
          <label>{t('runner.offlineMin')}<input class="field" id="rf-offline-min" inputMode="numeric" value={alertOfflineMin} onInput={(e) => setAlertOfflineMin((e.target as HTMLInputElement).value)} /></label>
          <label class="rf-check"><input type="checkbox" checked={makeDefault} onChange={(e) => setMakeDefault((e.target as HTMLInputElement).checked)} />{t('runner.makeDefault')}</label>
          <div class="act">
            <button class={`btn primary ${armed.value?.key === `rset:${r.id}` ? 'arm' : ''}`} type="button" id="rf-set" onClick={saveEdit}>{armed.value?.key === `rset:${r.id}` ? t('runner.saveConfirm') : t('runner.save')}</button>
            <button class="btn" type="button" onClick={() => setEditOpen(false)}>{t('cancel')}</button>
          </div>
        </section>
      )}
      {connectOpen && (
        <section class="rf-connect" id="rf-connect">
          <h3>{t('runner.connectTitle')}</h3>
          <p class="hint">{t('runner.connectText')}</p>
          <ul class="steps">
            <li class={block?.connected ? 'w-ok' : 'w-todo'}><span class="i">{block?.connected ? (block.stale ? '↻' : '✓') : '·'}</span><div>{snap.value?.host || ''}{block?.why ? <div class="hint">{block.why}</div> : null}</div></li>
          </ul>
          <div class="act">
            <button class={`btn primary ${armed.value?.key === `rcn:${r.id}` ? 'arm' : ''}`} type="button" id="rf-connect-go" onClick={connectAll}>{armed.value?.key === `rcn:${r.id}` ? t('runner.saveConfirm') : t('runner.connectGo')}</button>
          </div>
        </section>
      )}
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
