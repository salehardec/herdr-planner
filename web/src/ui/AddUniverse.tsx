import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { addDialogs } from '../dialogs';
import { api, onProgress, post } from '../api';
import { armed, galaxyId, snap, views } from '../store';
import { confirmStep } from '../actions';
import { navigateTo } from '../nav';
import { t } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import { hostKind } from '../util/format';

interface Entry { name: string; path: string; git: boolean; on_board: boolean; branch?: string; origin?: string }
interface Inspect {
  is_path: boolean; path?: string; git: boolean; branch?: string; origin?: string;
  address?: { host: string; kind: unknown; owner_path: string; name: string; clone_url: string };
  login?: { ok: boolean; user?: string; hint: string };
  can_write?: boolean; is_fork?: boolean; parent?: string; default_branch?: string; private?: boolean;
  suggested_name: string; suggested_key: string; clone_dir: string; exists_on_disk: boolean; on_board: boolean; board_name?: string; notes: string[];
}
interface Step { id: string; label: string; state: string; detail: string; percent?: number | null }

type Tab = 'folder' | 'address';
export const tab = signal<Tab>('folder');
const input = signal('');
const entries = signal<Entry[]>([]);
const cur = signal(0);
const info = signal<Inspect | null>(null);
const inspecting = signal(false);
const err = signal('');
const name = signal('');
const key = signal('');
const dir = signal('');
const mode = signal<'folder' | 'clone' | 'fork' | 'study'>('folder');
const job = signal<string | null>(null);
const steps = signal<Step[]>([]);
const result = signal<any | null>(null);
const failed = signal('');
let timer = 0;
let off: (() => void) | null = null;

const reset = (): void => {
  input.value = ''; entries.value = []; info.value = null; err.value = ''; name.value = ''; key.value = ''; dir.value = '';
  job.value = null; steps.value = []; result.value = null; failed.value = ''; mode.value = tab.value === 'folder' ? 'folder' : 'clone';
};

async function complete(galaxy: string): Promise<void> {
  try {
    const r = await api<{ entries: Entry[] }>(`/api/fs/complete?path=${encodeURIComponent(input.value)}&galaxy=${encodeURIComponent(galaxy)}`);
    entries.value = r.entries ?? [];
    cur.value = 0;
  } catch { entries.value = []; }
}

async function inspect(galaxy: string): Promise<void> {
  const text = input.value.trim();
  if (!text) return;
  inspecting.value = true;
  err.value = '';
  try {
    const r = await post<{ inspect: Inspect }>('/api/universe/inspect', { input: text, galaxy: galaxy || undefined });
    info.value = r.inspect;
    name.value = r.inspect.board_name || r.inspect.suggested_name;
    key.value = r.inspect.suggested_key;
    dir.value = r.inspect.clone_dir;
    if (r.inspect.is_path) mode.value = 'folder';
    else mode.value = r.inspect.can_write ? 'clone' : r.inspect.can_write === false ? 'fork' : 'clone';
  } catch (e) {
    info.value = null;
    err.value = (e as Error & { data?: { error?: string } }).data?.error ?? (e as Error).message;
  } finally { inspecting.value = false; }
}

/** Opens the dialog on a clean slate (the form is reset here, not after the first paint, so nothing typed is lost). */
export function openAddUniverse(galaxy: string): void {
  reset();
  addDialogs.universe.value = { galaxy };
}

export function AddUniverse(): JSX.Element | null {
  const d = addDialogs.universe.value;
  useEffect(() => {
    if (!d) return;
    off = onProgress((p) => {
      if (p.job !== job.value) return;
      if (p.steps) steps.value = p.steps;
      if (p.done) { if (p.ok) result.value = p.result; else failed.value = p.error ?? ''; }
    });
    return () => { off?.(); off = null; };
  }, [!!d]);
  if (!d) return null;
  const galaxy = d.galaxy;
  const view = views.value.find((v) => v.id === galaxy);
  const gname = galaxy === '' ? snap.value?.host ?? '' : galaxy;
  const i = info.value;
  const k = 'add-universe';
  const on = armed.value?.key === k;
  const busy = job.value !== null && !result.value && !failed.value;
  const taken = new Set((view?.projects ?? []).map((p) => p.name));
  const ready = !!i && !!name.value.trim() && /^[A-Za-z0-9._-]+$/.test(name.value.trim()) && !(i.on_board && mode.value === 'folder');
  const set = (patch: Partial<{ v: string }>): void => void patch;
  void set;

  const onType = (v: string): void => {
    input.value = v;
    info.value = null;
    window.clearTimeout(timer);
    if (tab.value === 'folder') timer = window.setTimeout(() => { void complete(galaxy); }, 120);
  };

  const pick = (e: Entry): void => {
    input.value = e.path.endsWith('/') ? e.path : e.git ? e.path : e.path + '/';
    entries.value = [];
    if (e.git) void inspect(galaxy); else void complete(galaxy);
  };

  const start = (): void => {
    if (!i || !ready) return;
    confirmStep(k, async () => {
      steps.value = []; failed.value = ''; result.value = null;
      try {
        const body: Record<string, unknown> = {
          mode: mode.value, name: name.value.trim(), key: key.value.trim() || undefined, galaxy: galaxy || undefined, confirm: true,
        };
        if (mode.value === 'folder') body.path = i.path ?? input.value.trim(); else { body.url = input.value.trim(); body.dir = dir.value.trim() ? dir.value.trim().replace(/\/[^/]+$/, '') : undefined; }
        const r = await post<{ job: string }>('/api/universe/add', body);
        job.value = r.job;
      } catch (e) { failed.value = (e as Error).message; toast((e as Error).message, { icon: 'failed' }); }
    });
  };

  const close = (): void => { addDialogs.universe.value = null; };

  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget && !busy) close(); }}>
      <section class="panel dlg wide" id="addu" role="dialog" aria-label={t('addu.title')}>
        <header><b>{t('addu.title', { g: gname })}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={close}><Gl n="close" /></button></header>
        <div class="bd">
          {!job.value && (
            <>
              <div class="tabs">
                <button type="button" class={`btn sm ${tab.value === 'folder' ? 'on' : ''}`} data-tab="folder" onClick={() => { tab.value = 'folder'; reset(); }}>{t('addu.tabFolder')}</button>
                <button type="button" class={`btn sm ${tab.value === 'address' ? 'on' : ''}`} data-tab="address" onClick={() => { tab.value = 'address'; reset(); }}>{t('addu.tabAddress')}</button>
              </div>
              <label>{tab.value === 'folder' ? t('addu.folderLabel') : t('addu.addressLabel')}
                <input
                  class="field" id="u-input" autoFocus value={input.value} spellcheck={false} autocomplete="off"
                  placeholder={tab.value === 'folder' ? '~/Projects/…' : 'https://github.com/owner/repo'}
                  onInput={(e) => onType((e.target as HTMLInputElement).value)}
                  onKeyDown={(e) => {
                    if (e.key === 'ArrowDown') { e.preventDefault(); cur.value = Math.min(entries.value.length - 1, cur.value + 1); }
                    else if (e.key === 'ArrowUp') { e.preventDefault(); cur.value = Math.max(0, cur.value - 1); }
                    else if (e.key === 'Tab' && entries.value[cur.value] && tab.value === 'folder') { e.preventDefault(); const en = entries.value[cur.value]; input.value = en.path + '/'; void complete(galaxy); }
                    else if (e.key === 'Enter') { e.preventDefault(); const en = entries.value[cur.value]; if (en && tab.value === 'folder' && !info.value) pick(en); else void inspect(galaxy); }
                  }}
                  onBlur={() => { if (tab.value === 'address' && input.value.trim() && !info.value) void inspect(galaxy); }}
                />
              </label>
              {tab.value === 'folder' && entries.value.length > 0 && (
                <div class="sug" role="listbox">
                  {entries.value.map((e, idx) => (
                    <div key={e.path} role="option" aria-selected={idx === cur.value} class={idx === cur.value ? 'cur' : ''} onClick={() => pick(e)}>
                      <span>{e.name}</span>
                      <span class="m">{e.on_board ? <span class="on">{t('addu.onBoard')}</span> : e.git ? <span class="ok">git{e.branch ? ` · ${e.branch}` : ''}</span> : <span class="no">{t('addu.notGit')}</span>}</span>
                    </div>
                  ))}
                </div>
              )}
              {inspecting.value && <div class="hint">{t('addu.checking')}</div>}
              {err.value && <div class="hint bad">{err.value}</div>}
              {i && <Details i={i} gname={gname} taken={taken} />}
              {i && (
                <div class="act">
                  <button class={`btn primary ${on ? 'arm' : ''}`} type="button" id="u-go" disabled={!ready} onClick={start}>{on ? t('addu.confirm') : t('addu.go')}</button>
                  <button class="btn" type="button" onClick={close}>{t('cancel')}</button>
                </div>
              )}
              {!i && <div class="act"><button class="btn" type="button" id="u-check" disabled={!input.value.trim() || inspecting.value} onClick={() => void inspect(galaxy)}>{t('addu.check')}</button></div>}
            </>
          )}
          {job.value && (
            <>
              <ul class="steps" id="u-steps">
                {steps.value.map((s) => (
                  <li class={s.state} key={s.id}>
                    <span class="i">{s.state === 'ok' ? '✓' : s.state === 'err' ? '✕' : s.state === 'run' ? '…' : '·'}</span>
                    <div>
                      <div>{s.label}</div>
                      {s.detail && <code>{s.detail}</code>}
                      {s.percent != null && s.state === 'run' && <div class="bar"><i style={`width:${s.percent}%`} /></div>}
                    </div>
                  </li>
                ))}
              </ul>
              {failed.value && <div class="hint bad">{failed.value}</div>}
              {result.value && (
                <div class="info" id="u-done">
                  <b>{t('addu.done')}</b> {result.value.name} · {result.value.key}
                  <div>{result.value.root}</div>
                  {result.value.fork_of && <div>{t('addu.forkOf', { o: result.value.fork_of })}</div>}
                  {result.value.settings?.length > 0 && <div>{t('addu.settings')}: <code>{result.value.settings.join(' · ')}</code></div>}
                  {(result.value.notes ?? []).map((n: string, idx: number) => <div class="hint" key={idx}>{n}</div>)}
                </div>
              )}
              <div class="act">
                {result.value && <button class="btn primary" type="button" id="u-fly" onClick={() => { close(); void navigateTo({ kind: 'universe', galaxy, id: result.value.name }); }}>{t('addu.fly')}</button>}
                {(result.value || failed.value) && <button class="btn" type="button" onClick={close}>{t('close')}</button>}
              </div>
            </>
          )}
        </div>
      </section>
    </div>
  );
}

function Details(props: { i: Inspect; gname: string; taken: Set<string> }): JSX.Element {
  const i = props.i;
  const addr = i.address;
  const isGitlab = hostKind(addr?.kind).startsWith('gitlab');
  const modes: { v: 'clone' | 'fork' | 'study'; label: string; hint: string }[] = [];
  if (!i.is_path) {
    if (i.can_write !== false) modes.push({ v: 'clone', label: t('addu.modeClone'), hint: t('addu.modeCloneHint') });
    if (i.can_write !== true) modes.push({ v: 'fork', label: t('addu.modeFork'), hint: t('addu.modeForkHint') });
    modes.push({ v: 'study', label: t('addu.modeStudy'), hint: t('addu.modeStudyHint') });
  }
  return (
    <div class="info" id="u-info">
      {i.is_path ? (
        <div>{i.git ? t('addu.gitFolder') : t('addu.notGitFolder')}{i.branch ? ` · ${i.branch}` : ''}{i.origin ? ` · ${i.origin}` : ''}{i.on_board ? ` · ${t('addu.onBoard')}` : ''}</div>
      ) : (
        <>
          <div>{addr ? `${addr.host} · ${addr.owner_path}/${addr.name}` : ''}{i.private ? ` · ${t('addu.private')}` : ''}</div>
          {i.login && !i.login.ok && <div class="hint bad">{i.login.hint}</div>}
          {i.login?.ok && <div class="hint">{t('addu.loggedIn', { u: i.login.user ?? '' })}{i.can_write === true ? ` · ${t('addu.canWrite')}` : i.can_write === false ? ` · ${t('addu.readOnly')}` : ''}</div>}
          {i.is_fork && i.parent && <div class="hint">{t('addu.isFork', { p: i.parent })}</div>}
        </>
      )}
      {!i.is_path && (
        <div class="modes" role="radiogroup">
          {modes.map((m) => (
            <label class={`mode ${mode.value === m.v ? 'sel' : ''}`} key={m.v}>
              <input type="radio" name="umode" value={m.v} checked={mode.value === m.v} onChange={() => { mode.value = m.v; }} />
              <span><b>{m.label}</b><br /><span class="hint">{m.hint}</span></span>
            </label>
          ))}
        </div>
      )}
      <div class="grid2">
        <label>{t('addu.name')}<input class="field" id="u-name" value={name.value} onInput={(e) => { name.value = (e.target as HTMLInputElement).value; }} /></label>
        <label>{t('addu.key')}<input class="field" id="u-key" value={key.value} maxLength={6} onInput={(e) => { key.value = (e.target as HTMLInputElement).value.toUpperCase(); }} /></label>
      </div>
      {!i.is_path && (
        <label>{t('addu.cloneTo')}<input class="field" id="u-dir" value={dir.value} onInput={(e) => { dir.value = (e.target as HTMLInputElement).value; }} /></label>
      )}
      {props.taken.has(name.value.trim()) && <div class="hint bad">{t('addu.nameTaken')}</div>}
      {mode.value === 'study' && <div class="hint">{t('addu.studyNote')}</div>}
      {isGitlab && <div class="hint">{t('addu.gitlabNote')}</div>}
      {i.exists_on_disk && !i.is_path && <div class="hint bad">{t('addu.exists')}</div>}
      {i.notes.map((n, idx) => <div class="hint" key={idx}>{n}</div>)}
    </div>
  );
}

void galaxyId;
