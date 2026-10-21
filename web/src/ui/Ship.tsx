/* The ship's computer («Бортовой компьютер»): the planner's own settings, the same wherever the ship flies.
   Device preferences (language, mouse, simple mode, notifications) act at once, as the menu's quick items do.
   Everything that changes the planner or its machines shows «было → стало» and asks twice (confirmStep). */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed, invertY, notifyOn, NOTIFY_KEY, INVERT_KEY, persist, QUERY, sensitivity, SENS_KEY, simple, SIMPLE_KEY, snap, galaxyId } from '../store';
import { addDialogs } from '../dialogs';
import { confirmStep } from '../actions';
import { lang, setLang, t } from '../i18n';
import { toast } from '../toast';
import { getWorld } from '../nav';
import { Gl } from './Gl';
import { callError, CallError, galaxyRemove, shipDlg, shipGet, shipPost, ShipData } from './station-api';
import './ship.css';

interface Row { label: string; was: string; now: string }
interface Pending { key: string; rows: Row[]; note: string; run: () => Promise<{ restart?: boolean } | void> }

/** The change waiting for its second press: the rows of «было → стало» and what runs. */
const pending = signal<Pending | null>(null);
const busy = signal(false);
const failure = signal<CallError | null>(null);
const restartNote = signal(false);
const info = signal<ShipData | null>(null);
const infoErr = signal<CallError | null>(null);

// Not the copy from the last visit: it may be for another client (can_edit) and would show editable fields at first. The reset
// runs the moment the dialog opens, before it renders (an effect would run after the first render, with the old data on screen).
shipDlg.subscribe((open) => { if (open) { info.value = null; infoErr.value = null; } });

async function loadInfo(): Promise<void> {
  try {
    info.value = await shipGet();
    infoErr.value = null;
  } catch (e) {
    infoErr.value = callError(e);
  }
}

/** Asks for the change: the card shows it, and the second press of its button runs it. */
function ask(p: Pending): void {
  failure.value = null;
  pending.value = p;
}

function fire(): void {
  const p = pending.value;
  if (!p || busy.value) return;
  confirmStep(p.key, async () => {
    busy.value = true;
    failure.value = null;
    try {
      const r = await p.run();
      if (r && r.restart) restartNote.value = true;
      toast(t('ship.saved'));
      pending.value = null;
      await loadInfo();
    } catch (e) {
      failure.value = callError(e);
    } finally {
      busy.value = false;
    }
  });
}

function ChangeCard(): JSX.Element | null {
  const p = pending.value;
  if (!p) return null;
  const on = armed.value?.key === p.key;
  const f = failure.value;
  return (
    <div class={`chg ${f ? 'bad' : ''}`} id="ship-change" data-key={p.key}>
      <div class="chg-h">{t('ship.changeTitle')}</div>
      {p.rows.map((r) => (
        <div class="ba" key={r.label}><span class="kl">{r.label}</span> <code class="was">{r.was || t('ship.empty')}</code><span class="arr" aria-hidden="true">→</span><code class="now">{r.now || t('ship.empty')}</code></div>
      ))}
      {p.note && <div class="hint">{p.note}</div>}
      {f && <div class="hint bad" id="ship-err">{f.message}</div>}
      <div class="act">
        <button class={`btn primary ${on ? 'arm' : ''}`} type="button" id="ship-confirm" disabled={busy.value} onClick={fire}>{on ? t('ship.confirm') : t('ship.apply')}</button>
        <button class="btn" type="button" id="ship-cancel" onClick={() => { pending.value = null; failure.value = null; }}>{t('cancel')}</button>
      </div>
    </div>
  );
}

/* ───────── the planner's own settings (device) ───────── */

function Device(): JSX.Element {
  const calm = QUERY.has('calm');
  return (
    <section class="sh-sec" id="ship-device">
      <h3>{t('ship.device')}</h3>
      <div class="sh-row">
        <span class="kl">{t('menu.language')}</span>
        <div class="row">
          <button type="button" class={`btn sm ${lang.value === 'ru' ? 'on' : ''}`} data-lang="ru" aria-pressed={lang.value === 'ru'} onClick={() => setLang('ru')}>{t('lang.ru')}</button>
          <button type="button" class={`btn sm ${lang.value === 'en' ? 'on' : ''}`} data-lang="en" aria-pressed={lang.value === 'en'} onClick={() => setLang('en')}>{t('lang.en')}</button>
        </div>
      </div>
      <div class="sh-row">
        <label for="ship-sens" class="kl">{t('menu.sensitivity')} <b>{sensitivity.value.toFixed(1)}</b></label>
        <input id="ship-sens" type="range" min="0.3" max="3" step="0.1" value={sensitivity.value}
          onInput={(e) => { sensitivity.value = Number((e.target as HTMLInputElement).value); persist(SENS_KEY, String(sensitivity.value)); getWorld()?.setSensitivity(sensitivity.value, invertY.value); }} />
      </div>
      <div class="sh-row">
        <span class="kl">{t('menu.invertY')}</span>
        <button type="button" class={`tog ${invertY.value ? 'on' : ''}`} id="ship-invert" aria-pressed={invertY.value} onClick={() => { invertY.value = !invertY.value; persist(INVERT_KEY, invertY.value ? '1' : '0'); getWorld()?.setSensitivity(sensitivity.value, invertY.value); }}>
          {invertY.value ? t('st.on') : t('st.off')}
        </button>
      </div>
      <div class="sh-row">
        <span class="kl">{t('menu.simple')}</span>
        <button type="button" class={`tog ${simple.value ? 'on' : ''}`} id="ship-simple" aria-pressed={simple.value} disabled={calm}
          title={calm ? t('toast.calmFixed') : ''} onClick={() => { if (calm) return; simple.value = !simple.value; persist(SIMPLE_KEY, simple.value ? 'on' : 'off'); }}>
          {simple.value ? t('st.on') : t('st.off')}
        </button>
      </div>
      <div class="sh-row">
        <span class="kl">{t('menu.notify')}</span>
        <button type="button" class={`tog ${notifyOn.value ? 'on' : ''}`} id="ship-notify" aria-pressed={notifyOn.value} onClick={() => {
          notifyOn.value = !notifyOn.value; persist(NOTIFY_KEY, notifyOn.value ? 'on' : 'off');
          if (notifyOn.value && typeof Notification !== 'undefined' && Notification.permission === 'default') void Notification.requestPermission().catch(() => {});
        }}>{notifyOn.value ? t('st.on') : t('st.off')}</button>
      </div>
      <div class="sh-row">
        <span class="kl">{t('ship.theme')}</span>
        <span class="v" id="ship-theme">{snap.value?.theme.name ?? '—'} · {snap.value?.theme.mode === 'light' ? t('ship.light') : t('ship.dark')}</span>
      </div>
      <p class="hint">{t('ship.themeHint')}</p>
    </section>
  );
}

/* ───────── the server's own settings (live or with a restart) ───────── */

function Folder({ d }: { d: ShipData }): JSX.Element {
  const [val, setVal] = useState(d.projects_dir);
  useEffect(() => setVal(d.projects_dir), [d.projects_dir]);
  const same = val.trim() === d.projects_dir || !val.trim();
  return (
    <section class="sh-sec" id="ship-folder">
      <h3>{t('ship.folder')}</h3>
      <p class="hint">{t('ship.folderHint')}</p>
      <form class="sh-form" onSubmit={(e) => {
        e.preventDefault();
        if (same) return;
        const next = val.trim();
        ask({
          key: `ship:folder:${next}`,
          rows: [{ label: t('ship.folderRow'), was: d.projects_dir, now: next }],
          note: t('ship.folderLive'),
          run: async () => { await shipPost({ projects_dir: next }); },
        });
      }}>
        <input class="field" id="ship-dir" value={val} aria-label={t('ship.folder')} onInput={(e) => setVal((e.target as HTMLInputElement).value)} />
        <button class="btn primary sm" type="submit" id="ship-dir-save" disabled={same}>{t('st.next')}</button>
      </form>
      {pending.value?.key.startsWith('ship:folder') && <ChangeCard />}
    </section>
  );
}

function Login({ d }: { d: ShipData }): JSX.Element {
  const [user, setUser] = useState(d.user);
  const [oldPw, setOld] = useState('');
  const [newPw, setNew] = useState('');
  const [again, setAgain] = useState('');
  const [msg, setMsg] = useState('');
  useEffect(() => setUser(d.user), [d.user]);
  const clearPw = (): void => { setOld(''); setNew(''); setAgain(''); };
  return (
    <section class="sh-sec" id="ship-login">
      <h3>{t('ship.login')}</h3>
      <p class="hint">{d.auth_required ? t('ship.loginOn') : t('ship.loginOff')}</p>
      <div class="sh-row">
        <span class="kl">{t('ship.user')}</span>
        {d.can_edit ? (
          <form class="sh-form" onSubmit={(e) => {
            e.preventDefault();
            const u = user.trim();
            if (!u || u === d.user) return;
            ask({
              key: `ship:user:${u}`, rows: [{ label: t('ship.user'), was: d.user, now: u }], note: t('ship.restartNote'),
              run: async () => shipPost({ user: u }),
            });
          }}>
            <input class="field" id="ship-user" value={user} autocomplete="username" aria-label={t('ship.user')} onInput={(e) => setUser((e.target as HTMLInputElement).value)} />
            <button class="btn sm" type="submit" disabled={!user.trim() || user.trim() === d.user}>{t('st.next')}</button>
          </form>
        ) : <b class="v">{d.user}</b>}
      </div>
      {pending.value?.key.startsWith('ship:user') && <ChangeCard />}
      <form class="sh-form col" id="ship-pw" autocomplete="off" onSubmit={(e) => {
        e.preventDefault();
        if (newPw.length < 8) { setMsg(t('ship.pwShort')); return; }
        if (newPw !== again) { setMsg(t('ship.pwMismatch')); return; }
        setMsg('');
        const o = oldPw;
        const n = newPw;
        ask({
          key: 'ship:password',
          rows: [{ label: t('ship.password'), was: d.auth_required ? t('ship.pwSet') : t('ship.pwNotSet'), now: t('ship.pwNew') }],
          note: t('ship.restartNote'),
          run: async () => {
            const r = await shipPost({ password: d.auth_required ? { old: o, new: n } : { new: n } });
            clearPw();
            return r;
          },
        });
      }}>
        <h4>{t('ship.pwChange')}</h4>
        {d.auth_required && (
          <label>{t('ship.pwOld')}<input class="field" type="password" id="ship-pw-old" autocomplete="current-password" value={oldPw} onInput={(e) => setOld((e.target as HTMLInputElement).value)} /></label>
        )}
        <label>{t('ship.pwNewLabel')}<input class="field" type="password" id="ship-pw-new" autocomplete="new-password" value={newPw} onInput={(e) => setNew((e.target as HTMLInputElement).value)} /></label>
        <label>{t('ship.pwAgain')}<input class="field" type="password" id="ship-pw-again" autocomplete="new-password" value={again} onInput={(e) => setAgain((e.target as HTMLInputElement).value)} /></label>
        {msg && <div class="hint bad" id="ship-pw-msg">{msg}</div>}
        <div class="act"><button class="btn primary sm" type="submit" id="ship-pw-go">{t('st.next')}</button></div>
      </form>
      {pending.value?.key === 'ship:password' && <ChangeCard />}
    </section>
  );
}

function Address({ d }: { d: ShipData }): JSX.Element {
  const [val, setVal] = useState(d.listen);
  useEffect(() => setVal(d.listen), [d.listen]);
  const same = !val.trim() || val.trim() === d.listen;
  return (
    <section class="sh-sec" id="ship-listen">
      <h3>{t('ship.listen')}</h3>
      <p class="hint">{d.can_edit ? t('ship.listenHint', { port: d.port }) : t('ship.listenRemote', { addr: d.listen, port: d.port })}</p>
      {d.can_edit && (
        <form class="sh-form" onSubmit={(e) => {
          e.preventDefault();
          if (same) return;
          const next = val.trim();
          ask({
            key: `ship:listen:${next}`, rows: [{ label: t('ship.listen'), was: d.listen, now: next }], note: t('ship.restartNote'),
            run: async () => shipPost({ listen: next }),
          });
        }}>
          <input class="field" id="ship-listen-in" value={val} aria-label={t('ship.listen')} onInput={(e) => setVal((e.target as HTMLInputElement).value)} />
          <button class="btn primary sm" type="submit" id="ship-listen-save" disabled={same}>{t('st.next')}</button>
        </form>
      )}
      {pending.value?.key.startsWith('ship:listen') && <ChangeCard />}
    </section>
  );
}

function Galaxies(): JSX.Element {
  const list = snap.value?.galaxies ?? [];
  return (
    <section class="sh-sec" id="ship-galaxies">
      <h3>{t('ship.galaxies')}</h3>
      <p class="hint">{t('ship.galaxiesHint')}</p>
      {list.length === 0 && <div class="hint">{t('ship.noGalaxies')}</div>}
      <ul class="gals">
        {list.map((g) => {
          const key = `ship:remove:${g.label}`;
          const showing = pending.value?.key === key;
          return (
            <li key={g.label} data-galaxy={g.label}>
              <div class="gl">
                <Gl n="galaxy" />
                <b>{g.label}</b>
                <span class={`badge s-${g.ok ? 'working' : 'failed'}`}>{g.ok ? t('galaxy.online') : t('galaxy.off')}</span>
                {g.version && <span class="m">herdr-projects {g.version}</span>}
                {galaxyId.value === g.label && <span class="hint">{t('ship.here')}</span>}
                <button class="btn sm danger" type="button" data-remove={g.label} onClick={() => ask({
                  key,
                  rows: [{ label: t('ship.galaxy'), was: `${g.label} (${g.ok ? t('galaxy.online') : t('galaxy.off')})`, now: t('ship.removed') }],
                  note: t('ship.removeNote'),
                  run: async () => { await galaxyRemove(g.label); },
                })}>{t('ship.remove')}</button>
              </div>
              {!g.ok && g.why && <div class="hint">{g.why.line}</div>}
              {showing && <ChangeCard />}
            </li>
          );
        })}
      </ul>
      <div class="row">
        <button class="btn" type="button" id="ship-add-galaxy" onClick={() => { shipDlg.value = false; addDialogs.galaxy.value = true; }}>
          <Gl n="plus" />{t('ship.addGalaxy')}
        </button>
      </div>
    </section>
  );
}

function Tls({ d }: { d: ShipData }): JSX.Element {
  return (
    <section class="sh-sec" id="ship-tls">
      <h3>{t('ship.tls')}</h3>
      <p class="hint">{d.tls ? t('ship.tlsOn') : t('ship.tlsLater')}</p>
    </section>
  );
}

/** The ship's computer. Opened by `openShip()`. */
export function Ship(): JSX.Element | null {
  const open = shipDlg.value;
  useEffect(() => {
    if (!open) return;
    restartNote.value = false;
    void loadInfo();
    return () => { pending.value = null; failure.value = null; };
  }, [open]);
  if (!open) return null;
  const d = info.value;
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) shipDlg.value = false; }}>
      <section class="panel dlg wide ship" id="ship" role="dialog" aria-label={t('ship.title')}>
        <header><b>{t('ship.title')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { shipDlg.value = false; }}><Gl n="close" /></button></header>
        <div class="bd sh-bd">
          <p class="hint">{t('ship.intro')}</p>
          <p class="hint" id="ship-version">{t('ship.version', { v: snap.value?.version ?? '—' })}</p>
          {restartNote.value && <div class="notice-in" id="ship-restart">{t('ship.restartNote')}</div>}
          <Device />
          {infoErr.value && !d && <div class="hint bad" id="ship-load-err">{infoErr.value.message}</div>}
          {d && (
            <>
              <Folder d={d} />
              <Login d={d} />
              <Address d={d} />
              <Tls d={d} />
              <Galaxies />
              <p class="hint">{t('ship.configFile', { path: d.config_file })}</p>
            </>
          )}
          {!d && !infoErr.value && <div class="hint">{t('st.loading')}</div>}
        </div>
      </section>
    </div>
  );
}

