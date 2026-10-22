import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { api, post, onProgress } from '../api';
import { openAddUniverse, tab } from './AddUniverse';
import { persist, SEEN_KEY, snap } from '../store';
import type { Snapshot } from '../types';
import { t } from '../i18n';
import { Gl } from './Gl';
import { navigateTo } from '../nav';
import './firstrun.css';

interface Found { name: string; path: string; branch?: string; origin?: string }
const found = signal<Found[]>([]);
const picked = signal<Set<string>>(new Set());
const busy = signal(false);
const log = signal<{ name: string; state: 'wait' | 'run' | 'ok' | 'err'; note: string }[]>([]);
const dismissed = signal(false);
let scanned = '';

/** The empty galaxy's first-run card is on the screen. The crew wizard waits while it is. */
export function firstRunShown(s: Snapshot | null): boolean {
  return !dismissed.value && !!s && s.projects.length === 0 && s.tasks.length === 0 && s.board.ok;
}

async function scan(dir: string): Promise<void> {
  if (!dir || scanned === dir) return;
  scanned = dir;
  try {
    const r = await api<{ entries: { name: string; path: string; git: boolean; on_board: boolean; branch?: string; origin?: string }[] }>(`/api/fs/complete?path=${encodeURIComponent(dir.replace(/\/?$/, '/'))}`);
    found.value = r.entries.filter((e) => e.git && !e.on_board).map((e) => ({ name: e.name, path: e.path, branch: e.branch, origin: e.origin }));
    picked.value = new Set(found.value.map((f) => f.path));
  } catch { found.value = []; }
}

async function addPicked(): Promise<void> {
  const list = found.value.filter((f) => picked.value.has(f.path));
  if (!list.length) return;
  busy.value = true;
  log.value = list.map((f) => ({ name: f.name, state: 'wait', note: '' }));
  let first = '';
  for (let i = 0; i < list.length; i++) {
    log.value = log.value.map((x, k) => (k === i ? { ...x, state: 'run' } : x));
    try {
      const r = await post<{ job: string }>('/api/universe/add', { mode: 'folder', path: list[i].path, name: list[i].name, confirm: true });
      const done = await new Promise<{ ok: boolean; error?: string; result?: { name: string } }>((resolve) => {
        const off = onProgress((p) => { if (p.job === r.job && p.done) { off(); resolve(p); } });
        setTimeout(() => { off(); resolve({ ok: false, error: 'timeout' }); }, 120000);
      });
      log.value = log.value.map((x, k) => (k === i ? { ...x, state: done.ok ? 'ok' : 'err', note: done.error ?? '' } : x));
      if (done.ok && !first) first = done.result?.name ?? list[i].name;
    } catch (e) {
      log.value = log.value.map((x, k) => (k === i ? { ...x, state: 'err', note: (e as Error).message } : x));
    }
  }
  busy.value = false;
  if (first) { dismissed.value = true; setTimeout(() => { void navigateTo({ kind: 'universe', id: first }); }, 600); }
}

/** The empty Galaxy: nothing on the board yet. Offers the git folders it finds in the projects folder. */
export function FirstRun(): JSX.Element | null {
  const s = snap.value;
  const empty = !!s && s.projects.length === 0 && s.tasks.length === 0 && s.board.ok;
  useEffect(() => { if (empty && s) void scan(s.server.projects_dir); }, [empty, s?.server.projects_dir]);
  if (!empty || dismissed.value) return null;
  return (
    <section class="panel firstrun" id="first" role="dialog" aria-label={t('first.title')}>
      <h2>{t('first.title')}</h2>
      <p>{t('first.text')}</p>
      {found.value.length > 0 ? (
        <>
          <div class="hint">{t('first.found', { dir: s?.server.projects_dir ?? '' })}</div>
          <div class="repos">
            {found.value.map((f) => (
              <label key={f.path}>
                <input type="checkbox" checked={picked.value.has(f.path)} disabled={busy.value} onChange={(e) => {
                  const n = new Set(picked.value);
                  if ((e.target as HTMLInputElement).checked) n.add(f.path); else n.delete(f.path);
                  picked.value = n;
                }} />
                <span>{f.name}</span><span class="m">{f.branch ?? ''}{f.origin ? ` · ${f.origin}` : ''}</span>
              </label>
            ))}
          </div>
        </>
      ) : <div class="hint">{t('first.none', { dir: s?.server.projects_dir ?? '' })}</div>}
      {log.value.length > 0 && (
        <ul class="flog">{log.value.map((l) => <li class={l.state} key={l.name}><span>{l.state === 'ok' ? '✓' : l.state === 'err' ? '✕' : l.state === 'run' ? '…' : '·'}</span>{l.name} {l.note && <em>{l.note}</em>}</li>)}</ul>
      )}
      <div class="row">
        {found.value.length > 0 && <button class="btn primary" type="button" id="first-add" disabled={busy.value || picked.value.size === 0} onClick={() => void addPicked()}>{t('first.add', { n: picked.value.size })}</button>}
        <button class="btn" type="button" onClick={() => { tab.value = 'folder'; openAddUniverse(); }}><Gl n="folder" />{t('first.folder')}</button>
        <button class="btn" type="button" onClick={() => { tab.value = 'address'; openAddUniverse(); }}><Gl n="globe" />{t('first.address')}</button>
      </div>
      <div class="more">
        <button class="btn sm" type="button" onClick={() => { dismissed.value = true; }}>{t('first.later')}</button>
      </div>
    </section>
  );
}

const coachStep = signal(0);
const coachDone = signal(((): boolean => { try { return localStorage.getItem(SEEN_KEY) === 'coach'; } catch { return false; } })());

/** The coach tour is on the screen. The crew wizard waits until it is done. */
export function coachShown(s: Snapshot | null): boolean {
  return !coachDone.value && !!s && s.projects.length > 0 && !new URLSearchParams(location.search).has('nocoach');
}

/** Three steps for a newcomer, once, after the first universe exists. */
export function Coach(): JSX.Element | null {
  const s = snap.value;
  if (coachDone.value || !s || s.projects.length === 0 || new URLSearchParams(location.search).has('nocoach')) return null;
  const steps = [
    { n: 1, text: t('coach.1'), at: 'left' },
    { n: 2, text: t('coach.2'), at: 'top' },
    { n: 3, text: t('coach.3'), at: 'right' },
  ];
  const st = steps[coachStep.value];
  const finish = (): void => { coachDone.value = true; persist(SEEN_KEY, 'coach'); };
  return (
    <div class={`panel coach ${st.at}`} id="coach" role="dialog" aria-label={t('coach.title')}>
      <div class="n">{t('coach.step', { n: st.n, total: steps.length })}</div>
      <p>{st.text}</p>
      <div class="row">
        {coachStep.value < steps.length - 1
          ? <button class="btn primary sm" type="button" id="coach-next" onClick={() => { coachStep.value += 1; }}>{t('coach.next')}</button>
          : <button class="btn primary sm" type="button" id="coach-done" onClick={finish}>{t('coach.done')}</button>}
        <button class="btn sm" type="button" onClick={finish}>{t('coach.skip')}</button>
      </div>
    </div>
  );
}
