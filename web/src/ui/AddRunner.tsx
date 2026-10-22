/* «Добавить верфь…»: an ssh address becomes a shipyard of this board's agents. Three steps: what is on the machine
   and whether this machine can enter it the way the agents will, the tags and the protected masks, and the summary
   that writes the list and offers to connect the agents. Nothing here stores a key: ssh agent and ~/.ssh/config do. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { addDialogs } from '../dialogs';
import { ApiError, post } from '../api';
import { armed, snap } from '../store';
import { confirmStep } from '../actions';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import type { Runner } from '../types';
import './AddRunner.css';

interface Local {
  docker_client: boolean;
  ssh_ok: boolean;
  docker_via_ssh: string;
  hint: string;
}
interface Probe {
  ok: boolean;
  label: string;
  target: string;
  ports_host: string;
  probe: Runner;
  local?: Local;
  suggested?: { tags: string[]; protected: string[]; ports_host: string };
  error?: string;
  step?: string;
}

type StepKey = 'check' | 'tags' | 'done';
const STEPS: StepKey[] = ['check', 'tags', 'done'];
const STEP_TITLE: Record<StepKey, string> = {
  check: 'runner.wiz.check',
  tags: 'runner.wiz.tags',
  done: 'runner.wiz.done',
};

const target = signal('');
const label = signal('');
const probe = signal<Probe | null>(null);
const probing = signal(false);
const probeErr = signal('');
const cur = signal<StepKey>('check');
const busy = signal(false);
const tags = signal('');
const masks = signal('');
const ports = signal('');
const note = signal('');
const makeDefault = signal(true);
const added = signal(false);
const err = signal('');

export function openAddRunner(): void {
  // A second visit starts clean: no probe of the yard added last time, no half-filled tags.
  reset();
  addDialogs.runner.value = true;
}

const validTarget = (s: string): boolean => /^[A-Za-z0-9._@:-]+$/.test(s.trim());

function reset(): void {
  probe.value = null;
  probeErr.value = '';
  cur.value = 'check';
  tags.value = '';
  masks.value = '';
  ports.value = '';
  note.value = '';
  makeDefault.value = true;
  added.value = false;
  err.value = '';
}

const localOk = (p: Probe | null): boolean =>
  !!p?.local && p.local.docker_client && p.local.ssh_ok && p.local.docker_via_ssh.length > 0;

async function probeNow(): Promise<void> {
  const addr = target.value.trim();
  if (!validTarget(addr)) return;
  probing.value = true;
  probeErr.value = '';
  try {
    const r = await post<Probe>('/api/runner/probe', { target: addr, label: label.value.trim() || undefined });
    if (!r.ok) {
      probe.value = r.probe ? r : null;
      probeErr.value = r.error ?? t('runner.wiz.probeFailed');
      return;
    }
    probe.value = r;
    tags.value = (r.suggested?.tags ?? []).join(', ');
    makeDefault.value = (snap.value?.runners?.length ?? 0) === 0;
    masks.value = (r.suggested?.protected ?? []).join(', ');
    ports.value = r.suggested?.ports_host ?? r.ports_host;
    label.value = r.label;
  } catch (e) {
    probe.value = null;
    probeErr.value = (e as ApiError).data?.error ?? (e as Error).message;
  } finally {
    probing.value = false;
  }
}

async function addNow(): Promise<void> {
  const p = probe.value;
  if (!p || !p.ok) return;
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
  try {
    await post('/api/runner/connect', { on: true, drop_old: true, confirm: true });
    toast(t('runner.connectedTo', { n: 1 }));
  } catch (e) {
    err.value = (e as ApiError).data?.error ?? (e as Error).message;
  }
}

function stepStatus(k: StepKey): 'ok' | 'err' | 'todo' {
  if (k === 'check') return localOk(probe.value) ? 'ok' : probeErr.value ? 'err' : 'todo';
  if (k === 'tags') return probe.value?.ok ? 'ok' : probeErr.value ? 'err' : 'todo';
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
      {line(p.ok || p.local?.ssh_ok === true, t('runner.wiz.sshOk', { target: p.target }))}
      {line(true, `${h.os || '?'} · ${h.arch || '?'} · ${t('runner.wiz.cpus', { n: h.cpus })}`)}
      {line(!!h.docker_version, h.docker_version ? `Docker ${h.docker_version}${h.compose_version ? ` · compose ${h.compose_version}` : ''}` : t('runner.wiz.noDocker'))}
      {line(h.gpu.length > 0, h.gpu.length > 0 ? h.gpu.map((g) => `${g.name} ${Math.round((g.mem_total_mib ?? 0) / 1024)} GB`).join(', ') : t('runner.wiz.noGpu'))}
      {line(p.probe.containers.length > 0, tn('runner.wiz.containersSeen', p.probe.containers.length))}
    </ul>
  );
}

function LocalList({ p }: { p: Probe }): JSX.Element | null {
  const l = p.local;
  if (!l) return null;
  const row = (ok: boolean, text: string): JSX.Element => (
    <li class={ok ? 'w-ok' : 'w-miss'}><span class="i">{ok ? '✓' : '✗'}</span><div>{text}</div></li>
  );
  return (
    <ul class="steps wchecks" id="r-local">
      {row(l.docker_client, t('runner.wiz.thisDocker'))}
      {row(l.ssh_ok, t('runner.wiz.thisToYard', { target: p.target }))}
      {row(l.docker_via_ssh.length > 0, l.docker_via_ssh ? `Docker ${l.docker_via_ssh}` : t('runner.wiz.noDockerViaSsh'))}
      {l.hint && <li class="w-todo"><span class="i">!</span><div class="hint">{l.hint}</div></li>}
    </ul>
  );
}

export function AddRunner(): JSX.Element | null {
  if (!addDialogs.runner.value) return null;
  const p = probe.value;
  const valid = validTarget(target.value);
  const ready = localOk(p);
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
                        <LocalList p={p} />
                        <div class="act"><button class="btn primary" type="button" id="r-check-next" disabled={!ready} onClick={() => { cur.value = 'tags'; }}>{t('runner.wiz.next')}</button></div>
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
