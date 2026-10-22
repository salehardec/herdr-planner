/* «Добавить верфь…»: an ssh address becomes a shipyard every galaxy can use. Four steps: what is on the machine,
   whether this machine and the galaxies reach it, the tags and the protected masks, and the summary that writes the
   list and offers to connect the agents. Nothing here stores a key: the ssh agent and ~/.ssh/config do that. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { addDialogs } from '../dialogs';
import { ApiError, post, onProgress } from '../api';
import { armed, galaxies, snap } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import type { Runner } from '../types';
import './AddGalaxy.css';

interface Probe {
  ok: boolean;
  label: string;
  target: string;
  ports_host: string;
  probe: Runner;
  suggested: { tags: string[]; protected: string[]; ports_host: string };
  error?: string;
  step?: string;
}
interface Row { id: string; label: string; state: 'wait' | 'run' | 'ok' | 'err'; detail: string }
interface ReachRow { galaxy: string; docker_client: boolean; reach: string; hint: string }

type StepKey = 'check' | 'reach' | 'tags' | 'done';
const STEPS: StepKey[] = ['check', 'reach', 'tags', 'done'];
const STEP_TITLE: Record<StepKey, string> = {
  check: 'runner.wiz.check',
  reach: 'runner.wiz.reach',
  tags: 'runner.wiz.tags',
  done: 'runner.wiz.done',
};

const target = signal('');
const label = signal('');
const probe = signal<Probe | null>(null);
const probing = signal(false);
const probeErr = signal('');
const cur = signal<StepKey>('check');
const rows = signal<Row[]>([]);
const reach = signal<ReachRow[] | null>(null);
const reachErr = signal('');
const busy = signal(false);
const tags = signal('');
const masks = signal('');
const ports = signal('');
const note = signal('');
const makeDefault = signal(true);
const added = signal(false);
const err = signal('');

export function openAddRunner(): void {
  addDialogs.runner.value = true;
}

const validTarget = (s: string): boolean => /^[A-Za-z0-9._@:-]+$/.test(s.trim());

function reset(): void {
  probe.value = null;
  probeErr.value = '';
  cur.value = 'check';
  rows.value = [];
  reach.value = null;
  reachErr.value = '';
  tags.value = '';
  masks.value = '';
  ports.value = '';
  note.value = '';
  makeDefault.value = true;
  added.value = false;
  err.value = '';
}

const latest = new Map<string, any>();
const watchers = new Map<string, (p: any) => void>();
onProgress((p) => {
  if (!p || typeof p.job !== 'string') return;
  latest.set(p.job, p);
  if (latest.size > 60) latest.delete(latest.keys().next().value as string);
  watchers.get(p.job)?.(p);
  if (p.done) watchers.delete(p.job);
});
function follow(id: string, fn: (p: any) => void): Promise<any> {
  return new Promise((resolve) => {
    const each = (p: any): void => { fn(p); if (p.done) resolve(p); };
    const seen = latest.get(id);
    if (seen?.done) { each(seen); return; }
    if (seen) fn(seen);
    watchers.set(id, each);
  });
}

async function probeNow(): Promise<void> {
  const addr = target.value.trim();
  if (!validTarget(addr)) return;
  probing.value = true;
  probeErr.value = '';
  try {
    const r = await post<Probe>('/api/runner/probe', { target: addr, label: label.value.trim() || undefined });
    if (!r.ok) {
      probe.value = null;
      probeErr.value = r.error ?? t('runner.wiz.probeFailed');
      return;
    }
    probe.value = r;
    tags.value = r.suggested.tags.join(', ');
    makeDefault.value = (snap.value?.runners?.length ?? 0) === 0;
    masks.value = r.suggested.protected.join(', ');
    ports.value = r.suggested.ports_host;
    label.value = r.label;
  } catch (e) {
    probe.value = null;
    probeErr.value = (e as ApiError).data?.error ?? (e as Error).message;
  } finally {
    probing.value = false;
  }
}

async function reachNow(): Promise<void> {
  const p = probe.value;
  if (!p) return;
  reachErr.value = '';
  rows.value = [
    { id: 'here', label: t('runner.wiz.thisDocker'), state: 'wait', detail: '' },
    { id: 'here-runner', label: t('runner.wiz.thisToYard'), state: 'wait', detail: '' },
  ];
  busy.value = true;
  try {
    const r = await post<{ job: string }>('/api/runner/reach', { target: p.target });
    const out = await follow(r.job, (pp) => { rows.value = pp.steps ?? rows.value; });
    if (out.ok) {
      reach.value = out.result?.rows ?? [];
    } else {
      reachErr.value = out.error ?? t('runner.wiz.reachFailed');
    }
  } catch (e) {
    reachErr.value = (e as Error).message;
  } finally {
    busy.value = false;
  }
}

async function addNow(): Promise<void> {
  const p = probe.value;
  if (!p) return;
  err.value = '';
  try {
    await post('/api/runner/add', {
      target: p.target, label: label.value.trim() || p.label, confirm: true,
      ports_host: ports.value.trim(), note: note.value,
      tags: tags.value.split(',').map((x) => x.trim()).filter(Boolean),
      protected: masks.value.split(',').map((x) => x.trim()).filter(Boolean),
      default: makeDefault.value,
    });
    added.value = true;
    cur.value = 'done';
    toast(t('runner.wiz.added', { id: label.value.trim() || p.label }));
  } catch (e) {
    err.value = (e as ApiError).data?.error ?? (e as Error).message;
  }
}

async function connectNow(): Promise<void> {
  err.value = '';
  const labels = ['', ...galaxies.value.map((g) => g.label)];
  let n = 0;
  for (const galaxy of labels) {
    try {
      await post('/api/runner/connect', { galaxy, on: true, drop_old: true, confirm: true });
      n += 1;
    } catch { /* the summary says how many answered */ }
  }
  toast(t('runner.connectedTo', { n }));
}

function stepStatus(k: StepKey): 'ok' | 'err' | 'todo' {
  if (k === 'check') return probe.value ? 'ok' : probeErr.value ? 'err' : 'todo';
  if (k === 'reach') return reach.value ? 'ok' : reachErr.value ? 'err' : 'todo';
  if (k === 'tags') return probe.value ? 'ok' : 'todo';
  return added.value ? 'ok' : 'todo';
}

const ICON: Record<string, string> = { ok: '✓', err: '!', todo: '·' };

function ProbeList({ p }: { p: Probe }): JSX.Element {
  const h = p.probe.host;
  const line = (ok: boolean | null, text: string): JSX.Element => (
    <li class={ok === null ? 'w-todo' : ok ? 'w-ok' : 'w-miss'}><span class="i">{ok === null ? '·' : ok ? '✓' : '✗'}</span><div>{text}</div></li>
  );
  return (
    <ul class="steps wchecks">
      {line(true, t('runner.wiz.sshOk', { target: p.target }))}
      {line(true, `${h.os || '?'} · ${h.arch || '?'} · ${t('runner.wiz.cpus', { n: h.cpus })}`)}
      {line(!!h.docker_version, h.docker_version ? `Docker ${h.docker_version}${h.compose_version ? ` · compose ${h.compose_version}` : ''}` : t('runner.wiz.noDocker'))}
      {line(h.gpu.length > 0, h.gpu.length > 0 ? h.gpu.map((g) => `${g.name} ${Math.round((g.mem_total_mib ?? 0) / 1024)} GB`).join(', ') : t('runner.wiz.noGpu'))}
      {line(p.probe.containers.length > 0, tn('runner.wiz.containersSeen', p.probe.containers.length))}
    </ul>
  );
}

export function AddRunner(): JSX.Element | null {
  if (!addDialogs.runner.value) return null;
  const p = probe.value;
  const valid = validTarget(target.value);
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) addDialogs.runner.value = false; }}>
      <section class="panel dlg wide wiz" id="addr" role="dialog" aria-label={t('runner.addTitle')}>
        <header><b>{t('runner.addTitle')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { addDialogs.runner.value = false; }}><Gl n="close" /></button></header>
        <div class="bd">
          <p class="hint">{t('runner.wiz.hint')}</p>
          <label>{t('runner.addTarget')}<input class="field" id="r-target" value={target.value} placeholder="user@host" autoFocus onInput={(e) => { target.value = (e.target as HTMLInputElement).value; }} /></label>
          <label>{t('runner.addLabel')}<input class="field" id="r-label" value={label.value} placeholder={t('runner.addLabelPh')} onInput={(e) => { label.value = (e.target as HTMLInputElement).value; }} /></label>
          <div class="act">
            <button class="btn primary" type="button" id="r-go" disabled={!valid || probing.value} onClick={() => void probeNow()}>
              {probing.value ? t('runner.wiz.probing') : p ? t('runner.wiz.recheck') : t('runner.wiz.check')}
            </button>
          </div>
          {probeErr.value && (
            <div class="info wfail-box">
              <p class="hint bad">{probeErr.value}</p>
              <div class="act"><button class="btn sm" type="button" onClick={() => void probeNow()}>{t('runner.retry')}</button></div>
            </div>
          )}
          {p && (
            <ol class="wsteps" id="r-steps">
              {STEPS.map((k, i) => {
                const st = stepStatus(k);
                const open = cur.value === k;
                return (
                  <li class={`wstep w-${st} ${open ? 'w-cur' : ''}`} key={k} data-step={k}>
                    <button class="wh" type="button" aria-expanded={open} onClick={() => { if (p) cur.value = k; }}>
                      <span class="wn">{i + 1}</span><span class="wt">{t(STEP_TITLE[k])}</span><span class="wi">{ICON[st]}</span>
                    </button>
                    {open && k === 'check' && (
                      <div class="wbody">
                        <ProbeList p={p} />
                        <div class="act"><button class="btn primary" type="button" onClick={() => void reachNow()}>{t('runner.wiz.next')}</button></div>
                      </div>
                    )}
                    {open && k === 'reach' && (
                      <div class="wbody">
                        <ul class="steps wrows">
                          {rows.value.map((r) => <li class={r.state} key={r.id}><span class="i">{r.state === 'ok' ? '✓' : r.state === 'err' ? '✕' : r.state === 'run' ? '…' : '·'}</span><div>{r.label}{r.detail && <div class="hint wdetail">{r.detail}</div>}</div></li>)}
                        </ul>
                        {reach.value && (
                          <table class="rft reach">
                            <thead><tr><th>{t('runner.wiz.machine')}</th><th>{t('runner.dockerShort')}</th><th>{t('runner.wiz.reaches')}</th></tr></thead>
                            <tbody>{reach.value.map((r) => (
                              <tr key={r.galaxy || 'home'}>
                                <td>{r.galaxy || snap.value?.host}</td>
                                <td>{r.docker_client ? '✓' : '✗'}</td>
                                <td>{r.reach || <span class="hint">{r.hint || '—'}</span>}</td>
                              </tr>
                            ))}</tbody>
                          </table>
                        )}
                        {reachErr.value && <p class="hint bad">{reachErr.value}</p>}
                        <div class="act">
                          <button class="btn primary" type="button" id="r-reach" disabled={busy.value} onClick={() => void reachNow()}>{busy.value ? t('runner.wiz.reaching') : t('runner.wiz.checkReach')}</button>
                          {reach.value && <button class="btn" type="button" id="r-reach-next" onClick={() => { cur.value = 'tags'; }}>{t('runner.wiz.next')}</button>}
                        </div>
                      </div>
                    )}
                    {open && k === 'tags' && (
                      <div class="wbody">
                        <label>{t('runner.tags')}<input class="field" id="r-tags" value={tags.value} onInput={(e) => { tags.value = (e.target as HTMLInputElement).value; }} /></label>
                        <label>{t('runner.masks')}<input class="field" id="r-masks" value={masks.value} onInput={(e) => { masks.value = (e.target as HTMLInputElement).value; }} /></label>
                        <label>{t('runner.portsHost')}<input class="field" id="r-ports" value={ports.value} onInput={(e) => { ports.value = (e.target as HTMLInputElement).value; }} /></label>
                        <label>{t('runner.note')}<input class="field" id="r-note" value={note.value} onInput={(e) => { note.value = (e.target as HTMLInputElement).value; }} /></label>
                        <label class="rf-check"><input type="checkbox" checked={makeDefault.value} onChange={(e) => { makeDefault.value = (e.target as HTMLInputElement).checked; }} />{t('runner.makeDefault')}</label>
                        <p class="hint">{t('runner.wiz.masksHint')}</p>
                        <div class="act"><button class={`btn primary ${armed.value?.key === 'radd' ? 'arm' : ''}`} type="button" id="r-add" onClick={() => confirmStep('radd', () => void addNow())}>{armed.value?.key === 'radd' ? t('runner.wiz.addConfirm') : t('runner.wiz.add')}</button></div>
                      </div>
                    )}
                    {open && k === 'done' && (
                      <div class="wbody">
                        <div class="info wsum">
                          <b>{t('runner.wiz.sumTitle', { id: label.value || p.label })}</b>
                          <div>{t('runner.wiz.sumFile', { path: t('runner.wiz.filePath') })}</div>
                          <div>{tn('runner.wiz.sumGalaxies', 1 + galaxies.value.length)}</div>
                        </div>
                        <div class="act">
                          <button class={`btn primary ${armed.value?.key === 'rconn' ? 'arm' : ''}`} type="button" id="r-connect" onClick={() => confirmStep('rconn', () => void connectNow())}>{armed.value?.key === 'rconn' ? t('runner.wiz.connectConfirm') : t('runner.wiz.connect')}</button>
                          <button class="btn" type="button" onClick={() => { addDialogs.runner.value = false; reset(); }}>{t('runner.wiz.close')}</button>
                        </div>
                      </div>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
          {err.value && <p class="hint bad">{err.value}</p>}
        </div>
      </section>
    </div>
  );
}
