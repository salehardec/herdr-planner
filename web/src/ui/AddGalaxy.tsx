/* «Добавить галактику»: a machine becomes a galaxy from SSH alone. Six steps, each with its own confirmation and
   progress: check the machine, herdr, the herdr-projects plugin, the crew's logins, the git repositories of its
   projects folder, and «Лететь». Nothing here stores or asks for a key or a password: the ssh-agent and the user's
   ssh config do that. The state lives in module signals, so closing the dialog during a job keeps the job; opening
   it again shows where it stands (the page's session, not a reload). */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { addDialogs } from '../dialogs';
import { ApiError, api, onProgress, post } from '../api';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { navigateTo } from '../nav';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import { openStation } from './station-api';
import './AddGalaxy.css';

/* ───────── shapes ───────── */

interface Probe {
  ok: boolean;
  target: string;
  label: string;
  saved: boolean;
  os: string;
  kernel: string;
  arch: string;
  supported: boolean;
  tools: { git: string | null; node: string | null; npm: string | null; curl: string | null };
  herdr: { installed: boolean; version: string };
  projects: { installed: boolean; version: string; home_version: string; home_ref: string | null; same: boolean };
  programs: { name: string; installed: boolean; version: string }[];
  projects_dir: string;
}

interface FarRepo { name: string; path: string; origin: string | null; branch: string | null; on_board: boolean }

interface Row { id: string; label: string; state: 'wait' | 'run' | 'ok' | 'err'; detail: string; percent: number | null }

interface JobState { id: string; rows: Row[]; done: boolean; ok: boolean; error: string; result: any }

type StepKey = 'check' | 'herdr' | 'plugin' | 'crew' | 'universes' | 'done';
const STEPS: StepKey[] = ['check', 'herdr', 'plugin', 'crew', 'universes', 'done'];
const STEP_TITLE: Record<StepKey, string> = {
  check: 'galaxy.wiz.stepCheck',
  herdr: 'galaxy.wiz.stepHerdr',
  plugin: 'galaxy.wiz.stepPlugin',
  crew: 'galaxy.wiz.stepCrew',
  universes: 'galaxy.wiz.stepUniverses',
  done: 'galaxy.wiz.stepDone',
};

/** The agent installers the planner runs (the table INSTALLERS in src/galaxy_setup.rs): what each needs and runs. */
const INSTALLERS: Record<string, { needs: 'curl' | 'npm'; command: string }> = {
  claude: { needs: 'curl', command: 'curl -fsSL https://claude.ai/install.sh | bash' },
  codex: { needs: 'npm', command: 'npm install -g @openai/codex' },
  omp: { needs: 'npm', command: 'npm install -g @oh-my-pi/pi-coding-agent' },
  pi: { needs: 'npm', command: 'npm install -g @mariozechner/pi-coding-agent' },
  opencode: { needs: 'npm', command: 'npm install -g opencode-ai' },
};

/* ───────── state (module level: it survives closing the dialog) ───────── */

const gTarget = signal('');
const gLabel = signal('');
const probe = signal<Probe | null>(null);
const probeErr = signal('');
const probing = signal(false);
const cur = signal<StepKey>('check');
/** Errors that come before a job exists (validation, a refused request), by step key. */
const errs = signal<Record<string, string>>({});
/** The jobs of the steps, by key (`herdr`, `plugin`, `install:<program>`, `check:<program>`, `login:<program>`). */
const jobs = signal<Record<string, JobState>>({});
const crew = signal<Record<string, { signedIn: boolean; line: string }>>({});
const loginCmd = signal<{ program: string; command: string; remote: boolean } | null>(null);
const repos = signal<FarRepo[] | null>(null);
const repoDir = signal('');
const repoErr = signal('');
const repoLoading = signal(false);
const picked = signal<string[]>([]);
const uniBusy = signal(false);
const uni = signal<Record<string, { state: 'wait' | 'run' | 'ok' | 'err'; note: string }>>({});
const uniRows = signal<Row[]>([]);
const addedNames = signal<string[]>([]);
const termCmd = signal('');

/* ───────── progress routing (one listener for the page; events may come before the answer to the POST) ───────── */

const latest = new Map<string, any>();
const watchers = new Map<string, (p: any) => void>();

onProgress((p) => {
  if (!p || typeof p.job !== 'string') return;
  latest.set(p.job, p);
  if (latest.size > 60) latest.delete(latest.keys().next().value as string);
  const w = watchers.get(p.job);
  if (w) w(p);
  if (p.done) watchers.delete(p.job);
});

/** Follows a job: `fn` gets every event (the last one has `done`); the promise resolves at the end. */
function follow(id: string, fn: (p: any) => void): Promise<any> {
  return new Promise((resolve) => {
    const each = (p: any): void => { fn(p); if (p.done) resolve(p); };
    const seen = latest.get(id);
    if (seen?.done) { each(seen); return; }
    if (seen) fn(seen);
    watchers.set(id, each);
  });
}

const setErr = (key: string, msg: string): void => { errs.value = { ...errs.value, [key]: msg }; };

/* ───────── calls ───────── */

const validTarget = (s: string): boolean => /^[A-Za-z0-9._@:-]+$/.test(s.trim());

async function probeNow(quiet = false): Promise<void> {
  const target = gTarget.value.trim();
  if (!validTarget(target)) return;
  probing.value = true;
  if (!quiet) probeErr.value = '';
  try {
    const r = await post<Probe & { error?: string }>('/api/galaxy/probe', { target, label: gLabel.value.trim() || undefined });
    if (!r.ok) {
      if (quiet) return;
      probe.value = null;
      probeErr.value = r.error ?? t('galaxy.wiz.probeFailed');
      return;
    }
    if (probe.value && probe.value.target !== r.target) reset();
    probe.value = r;
    probeErr.value = '';
  } catch (e) {
    if (quiet) return;
    probe.value = null;
    probeErr.value = (e as ApiError).data?.error ?? (e as Error).message;
  } finally {
    probing.value = false;
  }
}

/** A step's job: posts it, then follows its progress until it ends. */
async function runStep(key: string, body: Record<string, unknown>, after?: (j: JobState) => void): Promise<void> {
  setErr(key, '');
  try {
    const r = await post<{ ok: boolean; job: string }>('/api/galaxy/step', body);
    jobs.value = { ...jobs.value, [key]: { id: r.job, rows: [], done: false, ok: false, error: '', result: null } };
    await follow(r.job, (p) => {
      jobs.value = {
        ...jobs.value,
        [key]: { id: r.job, rows: p.steps ?? jobs.value[key]?.rows ?? [], done: !!p.done, ok: !!p.ok, error: p.error ?? '', result: p.result ?? null },
      };
    });
    const j = jobs.value[key];
    if (j?.ok) after?.(j);
  } catch (e) {
    setErr(key, (e as ApiError).data?.error ?? (e as Error).message);
  }
}

function reset(): void {
  jobs.value = {};
  errs.value = {};
  crew.value = {};
  loginCmd.value = null;
  repos.value = null;
  repoErr.value = '';
  picked.value = [];
  uni.value = {};
  uniRows.value = [];
  addedNames.value = [];
  termCmd.value = '';
  cur.value = 'check';
}

async function loadRepos(): Promise<void> {
  const p = probe.value;
  if (!p) return;
  repoLoading.value = true;
  repoErr.value = '';
  try {
    const r = await api<{ ok: boolean; dir: string; repos: FarRepo[] }>(`/api/galaxy/projects?galaxy=${encodeURIComponent(p.label)}`);
    repos.value = r.repos;
    repoDir.value = r.dir;
    picked.value = r.repos.filter((x) => !x.on_board).map((x) => x.path);
  } catch (e) {
    repos.value = null;
    repoErr.value = (e as Error).message;
  } finally {
    repoLoading.value = false;
  }
}

async function addPicked(): Promise<void> {
  const p = probe.value;
  if (!p || !repos.value) return;
  const list = repos.value.filter((r) => !r.on_board && picked.value.includes(r.path));
  uniBusy.value = true;
  const setUni = (path: string, v: { state: 'wait' | 'run' | 'ok' | 'err'; note: string }): void => {
    uni.value = { ...uni.value, [path]: v };
  };
  const done: string[] = [];
  for (const r of list) {
    setUni(r.path, { state: 'run', note: '' });
    try {
      const res = await post<{ job: string }>('/api/universe/add', { mode: 'folder', path: r.path, name: r.name, galaxy: p.label, confirm: true });
      const fin = await follow(res.job, (pp) => { uniRows.value = pp.steps ?? uniRows.value; });
      if (fin.ok) {
        setUni(r.path, { state: 'ok', note: '' });
        done.push(r.path);
        addedNames.value = [...addedNames.value, fin.result?.name ?? r.name];
      } else {
        setUni(r.path, { state: 'err', note: fin.error ?? '' });
      }
    } catch (e) {
      setUni(r.path, { state: 'err', note: (e as Error).message });
    }
  }
  uniRows.value = [];
  uniBusy.value = false;
  if (repos.value) {
    repos.value = repos.value.map((x) => (done.includes(x.path) ? { ...x, on_board: true } : x));
    picked.value = picked.value.filter((x) => !done.includes(x));
  }
}

/* ───────── the steps' state ───────── */

type Status = 'ok' | 'err' | 'todo';

function statusOf(k: StepKey): Status {
  const p = probe.value;
  switch (k) {
    case 'check':
      return p ? 'ok' : probeErr.value ? 'err' : 'todo';
    case 'herdr':
      if (p?.saved) return 'ok';
      return jobs.value.herdr?.done && !jobs.value.herdr.ok ? 'err' : 'todo';
    case 'plugin':
      if (p?.projects.same) return 'ok';
      return jobs.value.plugin?.done && !jobs.value.plugin.ok ? 'err' : 'todo';
    case 'crew':
      return Object.values(crew.value).some((c) => c.signedIn) ? 'ok' : 'todo';
    case 'universes':
      return addedNames.value.length > 0 || !!repos.value?.some((r) => r.on_board) ? 'ok' : 'todo';
    default:
      return 'todo';
  }
}

const ICON: Record<Status, string> = { ok: '✓', err: '!', todo: '·' };

function goTo(k: StepKey): void {
  if (!probe.value) return;
  cur.value = k;
  if (k === 'universes' && !repos.value && !repoLoading.value) void loadRepos();
}

const nextOf = (k: StepKey): StepKey => STEPS[Math.min(STEPS.indexOf(k) + 1, STEPS.length - 1)];

/** The label of a two-step button: the idle text, the armed text, or «Повторить» after a failure. */
function btn(key: string, idle: string, failed: boolean): string {
  if (armed.value?.key === key) return t('galaxy.addConfirm');
  return failed ? t('galaxy.retry') : idle;
}

/* ───────── pieces ───────── */

function Rows({ rows }: { rows: Row[] }): JSX.Element | null {
  if (rows.length === 0) return null;
  return (
    <ul class="steps wrows">
      {rows.map((r) => (
        <li class={r.state} key={r.id}>
          <span class="i">{r.state === 'ok' ? '✓' : r.state === 'err' ? '✕' : r.state === 'run' ? '…' : '·'}</span>
          <div>{r.label}{r.detail && <div class="hint wdetail">{r.detail}</div>}</div>
        </li>
      ))}
    </ul>
  );
}

/** A job's progress and its failure (plain text, with «Повторить» where the step offers it). */
function JobLine({ k }: { k: string }): JSX.Element | null {
  const j = jobs.value[k];
  const e = errs.value[k];
  if (!j && !e) return null;
  const failure = e || (j?.done && !j.ok ? j.error : '');
  return (
    <>
      {j && <Rows rows={j.rows} />}
      {failure && <p class="hint bad wfail">{failure}</p>}
    </>
  );
}

function Checks({ p }: { p: Probe }): JSX.Element {
  const row = (ok: boolean | null, text: string, extra?: JSX.Element | null): JSX.Element => (
    <li class={ok === null ? 'w-todo' : ok ? 'w-ok' : 'w-miss'}>
      <span class="i">{ok === null ? '·' : ok ? '✓' : '✗'}</span>
      <div>{text}{extra}</div>
    </li>
  );
  const tool = (name: string, v: string | null): JSX.Element => row(!!v, v ? `${name} ${v}` : t('galaxy.wiz.noTool', { name }));
  const have = p.programs.filter((x) => x.installed);
  const board = p.projects.same
    ? row(true, t('galaxy.wiz.boardSame', { v: p.projects.version }))
    : p.projects.installed
      ? row(null, t('galaxy.wiz.boardOther', { v: p.projects.version, home: p.projects.home_version }))
      : row(false, t('galaxy.wiz.boardNo'));
  return (
    <ul class="steps wchecks">
      {row(true, t('galaxy.wiz.sshOk', { target: p.target }))}
      {row(p.supported, t('galaxy.wiz.osLine', { os: p.os || '?', arch: p.arch || '?' }), !p.supported ? <div class="hint bad">{t('galaxy.wiz.unsupported')}</div> : null)}
      {tool('git', p.tools.git)}
      {tool('node', p.tools.node)}
      {tool('npm', p.tools.npm)}
      {tool('curl', p.tools.curl)}
      {row(p.herdr.installed, p.herdr.installed ? t('galaxy.wiz.herdrYes', { v: p.herdr.version || '?' }) : t('galaxy.wiz.herdrNo'))}
      {board}
      {row(have.length > 0 ? true : null, have.length > 0 ? t('galaxy.wiz.agentsYes', { list: have.map((x) => (x.version ? `${x.name} ${x.version}` : x.name)).join(', ') }) : t('galaxy.wiz.agentsNone'))}
      {row(null, t('galaxy.wiz.projectsDir', { dir: p.projects_dir || '~/Projects' }))}
    </ul>
  );
}

function StepCheck({ p }: { p: Probe }): JSX.Element {
  return (
    <>
      <Checks p={p} />
      <div class="act"><button class="btn" type="button" onClick={() => void probeNow()} disabled={probing.value}>{t('galaxy.wiz.recheck')}</button>
        <button class="btn primary" type="button" onClick={() => goTo(nextOf('check'))}>{t('galaxy.wiz.next')}</button></div>
    </>
  );
}

function StepHerdr({ p }: { p: Probe }): JSX.Element {
  const key = 'herdr';
  const j = jobs.value[key];
  const failed = !!errs.value[key] || (!!j?.done && !j.ok);
  const busy = !!j && !j.done;
  const go = (): void => {
    confirmStep('gw:herdr', () => void runStep(key, { step: 'herdr', target: p.target, label: p.label, confirm: true }, () => void probeNow(true)));
  };
  if (p.saved) {
    return (
      <>
        <p class="hint">{t('galaxy.wiz.herdrSaved', { label: p.label })}</p>
        <div class="act"><button class="btn primary" type="button" onClick={() => goTo(nextOf('herdr'))}>{t('galaxy.wiz.next')}</button></div>
      </>
    );
  }
  return (
    <>
      <div class="info">
        <b>{t('galaxy.wiz.herdrWhat')}</b>
        <code>herdr machine add {p.target} --label {p.label}</code>
        <div class="hint">{t('galaxy.wiz.herdrHint')}</div>
      </div>
      <JobLine k={key} />
      <div class="act">
        <button id="gw-herdr" class={`btn primary ${armed.value?.key === 'gw:herdr' ? 'arm' : ''}`} type="button" disabled={busy || !p.supported} onClick={go}>
          {btn('gw:herdr', t('galaxy.wiz.herdrGo'), failed)}
        </button>
        {j?.done && j.ok && <button class="btn primary" type="button" onClick={() => goTo(nextOf('herdr'))}>{t('galaxy.wiz.next')}</button>}
      </div>
    </>
  );
}

function StepPlugin({ p }: { p: Probe }): JSX.Element {
  const key = 'plugin';
  const j = jobs.value[key];
  const failed = !!errs.value[key] || (!!j?.done && !j.ok);
  const busy = !!j && !j.done;
  const need = !p.saved && !p.herdr.installed;
  const ref = p.projects.home_ref ? ` --ref ${p.projects.home_ref}` : '';
  const go = (): void => {
    confirmStep('gw:plugin', () => void runStep(key, { step: 'plugin', target: p.target, label: p.label, confirm: true }, () => void probeNow(true)));
  };
  if (p.projects.same) {
    return (
      <>
        <p class="hint">{t('galaxy.wiz.pluginSame', { v: p.projects.version })}</p>
        <div class="act"><button class="btn primary" type="button" onClick={() => goTo(nextOf('plugin'))}>{t('galaxy.wiz.next')}</button></div>
      </>
    );
  }
  return (
    <>
      <p class="hint">{t('galaxy.wiz.pluginHome', { home: p.projects.home_version, far: p.projects.installed ? p.projects.version : t('galaxy.wiz.none') })}</p>
      {need && <p class="hint bad">{t('galaxy.wiz.pluginNeedsHerdr')}</p>}
      <div class="info">
        <b>{t('galaxy.wiz.pluginWhat')}</b>
        <code>herdr plugin install eliasstravik/herdr-projects{ref} -y</code>
        <code>herdr-projects install</code>
        <code>herdr-projects version</code>
      </div>
      <JobLine k={key} />
      <div class="act">
        <button id="gw-plugin" class={`btn primary ${armed.value?.key === 'gw:plugin' ? 'arm' : ''}`} type="button" disabled={busy || need || !p.supported} onClick={go}>
          {btn('gw:plugin', t('galaxy.wiz.pluginGo'), failed)}
        </button>
        {j?.done && j.ok && <button class="btn primary" type="button" onClick={() => goTo(nextOf('plugin'))}>{t('galaxy.wiz.next')}</button>}
      </div>
    </>
  );
}

function ProgramRow({ p, name, version, installed }: { p: Probe; name: string; version: string; installed: boolean }): JSX.Element {
  const inst = INSTALLERS[name];
  const ik = `install:${name}`;
  const ck = `check:${name}`;
  const ij = jobs.value[ik];
  const cj = jobs.value[ck];
  const lk = `login:${name}`;
  const missingTool = inst ? (inst.needs === 'curl' ? !p.tools.curl : !p.tools.npm) : false;
  const c = crew.value[name];
  const installFailed = !!errs.value[ik] || (!!ij?.done && !ij.ok);
  const install = (): void => {
    confirmStep(`gw:${ik}`, () => void runStep(ik, { step: 'agent-install', target: p.target, label: p.label, program: name, confirm: true }, () => void probeNow(true)));
  };
  const check = async (): Promise<void> => {
    setErr(ck, '');
    await runStep(ck, { step: 'agent-check', target: p.target, label: p.label, program: name }, (j) => {
      crew.value = { ...crew.value, [name]: { signedIn: !!j.result?.signed_in, line: j.result?.line ?? '' } };
    });
  };
  const login = async (): Promise<void> => {
    setErr(lk, '');
    try {
      // The press on «Войти» is the confirmation the step asks for (the terminal is the person's own login).
      const r = await post<{ command: string }>('/api/galaxy/step', { step: 'agent-login', target: p.target, label: p.label, program: name, confirm: true });
      loginCmd.value = { program: name, command: r.command, remote: false };
      toast(t('galaxy.wiz.loginOpened'));
    } catch (e) {
      const d = (e as ApiError).data;
      if (d?.remote && d.command) loginCmd.value = { program: name, command: d.command, remote: true };
      else setErr(lk, (e as Error).message);
    }
  };
  return (
    <li class={installed ? 'w-ok' : 'w-todo'}>
      <span class="i">{installed ? '✓' : '·'}</span>
      <div class="wprog">
        <div><b>{name}</b> <span class="hint">{installed ? (version || t('galaxy.wiz.installedNoVersion')) : t('galaxy.wiz.missing')}</span></div>
        {installed && (
          <div class="act">
            <button id={`gw-login-${name}`} class="btn sm" type="button" onClick={() => void login()}>{t('galaxy.wiz.login')}</button>
            <button id={`gw-check-${name}`} class="btn sm" type="button" disabled={!!cj && !cj.done} onClick={() => void check()}>{t('galaxy.wiz.loginCheck')}</button>
            {c && <span class={`hint ${c.signedIn ? 'ok' : 'bad'}`}>{c.signedIn ? t('galaxy.wiz.signedIn') : t('galaxy.wiz.notSignedIn')}</span>}
          </div>
        )}
        {!installed && inst && (
          <>
            <div class="hint">{t('galaxy.wiz.installCmd')} <code>{inst.command}</code></div>
            {missingTool && <div class="hint bad">{inst.needs === 'curl' ? t('galaxy.wiz.needCurl') : t('galaxy.wiz.needNpm')}</div>}
            <div class="act">
              <button id={`gw-install-${name}`} class={`btn sm ${armed.value?.key === `gw:${ik}` ? 'arm' : ''}`} type="button" disabled={missingTool || !p.supported || (!!ij && !ij.done)} onClick={install}>
                {btn(`gw:${ik}`, t('galaxy.wiz.install'), installFailed)}
              </button>
            </div>
          </>
        )}
        {!installed && !inst && <div class="hint">{t('galaxy.wiz.manual')}</div>}
        {loginNoteFor(name, p)}
        <JobLine k={ik} />
        <JobLine k={ck} />
        <JobLine k={lk} />
        {cj?.done && cj.ok && cj.result?.line && <div class="hint wdetail">{cj.result.line}</div>}
        {ij?.done && ij.ok && <div class="hint wdetail">{t('galaxy.wiz.installed')}</div>}
      </div>
    </li>
  );
}

/** The login's answer: the terminal is open (local), or the command to run here (a remote client). */
function loginNoteFor(name: string, p: Probe): JSX.Element | null {
  const l = loginCmd.value;
  if (!l || l.program !== name) return null;
  return (
    <div class="info wlogin">
      <div class="hint">{l.remote ? t('galaxy.wiz.loginRemote') : t('galaxy.wiz.loginLocal', { target: p.target })}</div>
      <code>{l.command}</code>
    </div>
  );
}

function StepCrew({ p }: { p: Probe }): JSX.Element {
  return (
    <>
      <p class="hint">{t('galaxy.wiz.crewIntro')}</p>
      <ul class="steps wcrew">
        {p.programs.map((x) => (
          <ProgramRow key={x.name} p={p} name={x.name} version={x.version} installed={x.installed} />
        ))}
      </ul>
      <div class="info">
        <div class="hint">{t('galaxy.wiz.rolesHint')}</div>
        <div class="act">
          <button id="gw-roles" class="btn" type="button" onClick={() => { addDialogs.galaxy.value = false; openStation(p.label); }}>
            <Gl n="planner" />{t('galaxy.wiz.roles')}
          </button>
        </div>
      </div>
      <div class="act"><button class="btn primary" type="button" onClick={() => goTo(nextOf('crew'))}>{t('galaxy.wiz.next')}</button></div>
    </>
  );
}

function StepUniverses({ p }: { p: Probe }): JSX.Element {
  const list = repos.value;
  const addable = list ? list.filter((r) => !r.on_board && picked.value.includes(r.path)) : [];
  const toggle = (path: string, on: boolean): void => {
    picked.value = on ? [...picked.value, path] : picked.value.filter((x) => x !== path);
  };
  const go = (): void => { confirmStep('gw:uni', () => void addPicked()); };
  return (
    <>
      {repoLoading.value && <p class="hint">{t('galaxy.wiz.reposLoading')}</p>}
      {repoErr.value && (
        <>
          <p class="hint bad">{repoErr.value}</p>
          <div class="act"><button class="btn" type="button" onClick={() => void loadRepos()}>{t('galaxy.retry')}</button></div>
        </>
      )}
      {list && list.length === 0 && <p class="hint">{t('galaxy.wiz.noRepos', { dir: repoDir.value || p.projects_dir })}</p>}
      {list && list.length > 0 && (
        <>
          <div class="hint">{t('galaxy.wiz.reposIn', { dir: repoDir.value || p.projects_dir })}</div>
          <div class="wrepos">
            {list.map((r) => {
              const u = uni.value[r.path];
              return (
                <label class={`wrepo ${r.on_board ? 'on' : ''}`} key={r.path}>
                  <input type="checkbox" checked={r.on_board ? false : picked.value.includes(r.path)} disabled={r.on_board || uniBusy.value} onChange={(e) => toggle(r.path, (e.target as HTMLInputElement).checked)} />
                  <span class="wname">{r.name}</span>
                  <span class="m">{r.on_board ? t('galaxy.wiz.onBoard') : [r.branch, r.origin].filter(Boolean).join(' · ')}</span>
                  {u && <span class={`wst ${u.state}`}>{u.state === 'ok' ? '✓' : u.state === 'err' ? '✕' : u.state === 'run' ? '…' : ''}{u.note && <em>{u.note}</em>}</span>}
                </label>
              );
            })}
          </div>
          <Rows rows={uniRows.value} />
          <div class="act">
            <button id="gw-uni" class={`btn primary ${armed.value?.key === 'gw:uni' ? 'arm' : ''}`} type="button" disabled={addable.length === 0 || uniBusy.value || !p.supported} onClick={go}>
              {armed.value?.key === 'gw:uni' ? t('galaxy.addConfirm') : tn('galaxy.wiz.addN', addable.length)}
            </button>
            <button class="btn" type="button" onClick={() => goTo(nextOf('universes'))} disabled={uniBusy.value}>{t('galaxy.wiz.next')}</button>
          </div>
        </>
      )}
    </>
  );
}

function StepDone({ p }: { p: Probe }): JSX.Element {
  const plug = jobs.value.plugin;
  const ver = plug?.result?.version ?? (p.projects.same ? p.projects.version : '');
  const agents = crew.value;
  const signed = Object.entries(agents).filter(([, c]) => c.signedIn).map(([n]) => n);
  const added = addedNames.value.length;
  return (
    <>
      <div class="info wsum">
        <b>{t('galaxy.wiz.sumTitle', { label: p.label })}</b>
        <div>{t('galaxy.wiz.sumHerdr', { v: p.herdr.version || (p.herdr.installed ? '?' : t('galaxy.wiz.none')) })}</div>
        <div>{ver ? t('galaxy.wiz.sumBoard', { v: ver }) : t('galaxy.wiz.sumBoardNo')}</div>
        <div>{signed.length ? t('galaxy.wiz.sumCrew', { list: signed.join(', ') }) : t('galaxy.wiz.sumCrewNo')}</div>
        <div>{added ? tn('galaxy.wiz.sumUni', added) : t('galaxy.wiz.sumUniNo')}</div>
      </div>
      <div class="act">
        <button id="gw-fly" class="btn primary" type="button" onClick={() => {
          addDialogs.galaxy.value = false;
          void navigateTo({ kind: 'galaxy', galaxy: p.label, id: p.label });
        }}><Gl n="galaxy" />{t('galaxy.wiz.fly')}</button>
      </div>
    </>
  );
}

function stepBody(k: StepKey, p: Probe): JSX.Element | null {
  switch (k) {
    case 'check': return <StepCheck p={p} />;
    case 'herdr': return <StepHerdr p={p} />;
    case 'plugin': return <StepPlugin p={p} />;
    case 'crew': return <StepCrew p={p} />;
    case 'universes': return <StepUniverses p={p} />;
    case 'done': return <StepDone p={p} />;
  }
}

/** The stepper: six steps, the current one open. Every step that is done can be opened again. */
function Stepper({ p }: { p: Probe }): JSX.Element {
  return (
    <ol class="wsteps" id="g-steps">
      {STEPS.map((k, i) => {
        const st = statusOf(k);
        const open = cur.value === k;
        return (
          <li class={`wstep w-${st} ${open ? 'w-cur' : ''}`} key={k} data-step={k}>
            <button class="wh" type="button" aria-expanded={open} onClick={() => goTo(k)}>
              <span class="wn">{i + 1}</span>
              <span class="wt">{t(STEP_TITLE[k])}</span>
              <span class="wi">{ICON[st]}</span>
            </button>
            {open && <div class="wbody">{stepBody(k, p)}</div>}
          </li>
        );
      })}
    </ol>
  );
}

/* ───────── the dialog ───────── */

export function AddGalaxy(): JSX.Element | null {
  if (!addDialogs.galaxy.value) return null;
  const p = probe.value;
  const valid = validTarget(gTarget.value);
  const running = Object.values(jobs.value).some((j) => !j.done) || uniBusy.value;
  const term = (): void => {
    const target = gTarget.value.trim();
    const label = gLabel.value.trim() || undefined;
    confirmStep('gw:term', async () => {
      try {
        const r = await post<{ command: string }>('/api/galaxy/add', { target, label, confirm: true });
        termCmd.value = r.command;
        toast(t('galaxy.addOpened'));
      } catch (e) {
        toast((e as Error).message, { icon: 'failed' });
      }
    });
  };
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) addDialogs.galaxy.value = false; }}>
      <section class="panel dlg wide wiz" id="addg" role="dialog" aria-label={t('galaxy.addTitle')}>
        <header><b>{t('galaxy.addTitle')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { addDialogs.galaxy.value = false; }}><Gl n="close" /></button></header>
        <div class="bd">
          <p class="hint">{t('galaxy.wiz.hint')}</p>
          <label>{t('galaxy.addTarget')}<input class="field" id="g-target" value={gTarget.value} placeholder="user@host" autoFocus onInput={(e) => { gTarget.value = (e.target as HTMLInputElement).value; }} /></label>
          <label>{t('galaxy.addLabel')}<input class="field" id="g-label" value={gLabel.value} placeholder={t('galaxy.addLabelPh')} onInput={(e) => { gLabel.value = (e.target as HTMLInputElement).value; }} /></label>
          <p class="hint">{t('galaxy.wiz.keys')}</p>
          <div class="act">
            <button class="btn primary" type="button" id="g-go" disabled={!valid || probing.value} onClick={() => void probeNow()}>
              {probing.value ? t('galaxy.wiz.probing') : p ? t('galaxy.wiz.recheck') : t('galaxy.wiz.check')}
            </button>
          </div>
          {probeErr.value && (
            <div class="info wfail-box">
              <p class="hint bad">{probeErr.value}</p>
              <div class="act"><button class="btn sm" type="button" onClick={() => void probeNow()}>{t('galaxy.retry')}</button></div>
            </div>
          )}
          {running && <p class="hint">{t('galaxy.wiz.running')}</p>}
          {p && <Stepper p={p} />}
          <div class="wterm">
            <button id="gw-term" class={`btn sm ${armed.value?.key === 'gw:term' ? 'arm' : ''}`} type="button" disabled={!valid} onClick={term}>
              {armed.value?.key === 'gw:term' ? t('galaxy.addConfirm') : t('galaxy.wiz.termInstead')}
            </button>
            <p class="hint">{t('galaxy.wiz.termHint')}</p>
            {termCmd.value && <p class="hint">{t('galaxy.addRan')} <code>{termCmd.value}</code></p>}
          </div>
        </div>
      </section>
    </div>
  );
}
