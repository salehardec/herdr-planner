import type { JSX } from 'preact';
import { signal } from '@preact/signals';
import { useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import { t, tn, locale } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import {
  checkGate, checkText, checked, closeGate, closeTunnelCard, decideDevice, draftConfig, draftHost, loadTunnel, openGate, openTunnelGuide,
  rotateLink, saveDrafts, saveGateConfig, setGateSettings, shownConfig, shownHost, tunnelCard, tunnelView,
  type TunnelCheck, type TunnelDevice, type TunnelView,
} from './tunnel-api';
import { PACKAGES_URL, RELEASES_URL } from './tunnel-guide';
import './tunnel.css';

/** The clock of the open card: seconds of a link, of a request, of a start. */
const now = signal(Date.now());

export type Kind = 'closed' | 'starting' | 'open' | 'pending' | 'reconnecting' | 'failed' | 'missing' | 'external' | 'closing';

/** What the plaque shows: the server's state, with «a device waits» and «no cloudflared» told apart. */
export function kindOf(v: TunnelView): Kind {
  if (!v.installed && (v.state === 'closed' || v.state === 'failed')) return 'missing';
  if (v.state === 'open' && v.sessions.some((d) => d.state === 'pending')) return 'pending';
  return v.state;
}

const devicesIn = (v: TunnelView): TunnelDevice[] => v.sessions.filter((d) => d.state === 'active');
const waitingIn = (v: TunnelView): TunnelDevice[] => v.sessions.filter((d) => d.state === 'pending');

export function copyText(text: string): void {
  navigator.clipboard?.writeText(text).catch(() => {});
  toast(t('toast.copied'));
}

const clock = (ms: number): string => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};
/** A longer wait in hours and minutes (the idle time of a gate is hours): `3 h 59 min`. */
const long = (ms: number): string => {
  const m = Math.max(0, Math.ceil(ms / 60000));
  return m < 60 ? clock(ms) : m % 60 ? `${t('dur.hour', { n: Math.floor(m / 60) })} ${t('dur.min', { n: m % 60 })}` : t('dur.hour', { n: m / 60 });
};
const hhmm = (ms: number): string => new Date(ms).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
function ago(ms: number): string {
  const m = Math.floor((now.value - ms) / 60000);
  if (m < 1) return t('tun.ago.now');
  if (m < 60) return t('tun.ago.min', { n: m });
  return t('tun.ago.hour', { n: Math.floor(m / 60) });
}

/** The second line of the card's head. */
function subOf(v: TunnelView, k: Kind): string {
  if (k === 'starting') return t('tun.sub.starting', { n: Math.max(0, Math.floor((now.value - v.since_ms) / 1000)) });
  if (k === 'open' && devicesIn(v).length) return t('tun.sub.devices', { list: tn('tun.devices', devicesIn(v).length) });
  return t(`tun.sub.${k}`);
}

/** A command or a file with the button that copies it. */
export function CopyCmd(props: { text: string; block?: boolean; label?: string }): JSX.Element {
  return (
    <div class={`tn-cmd${props.block ? ' block' : ''}`}>
      {props.block ? <pre><code>{props.text}</code></pre> : <code>{props.text}</code>}
      <button class="btn sm" type="button" aria-label={props.label ?? t('tun.link.copy')} title={t('tun.link.copy')} onClick={() => copyText(props.text)}><Gl n="copy" /></button>
    </div>
  );
}

export function CheckList(props: { checks: TunnelCheck[] }): JSX.Element {
  return (
    <ul class="tn-checks">
      {props.checks.map((c) => (
        <li key={c.id} class={c.ok ? 'ok' : c.warn ? 'warn' : 'bad'}>
          <span class="m" aria-hidden="true">{c.ok ? '✓' : c.warn ? '!' : '✕'}</span>
          <span>{checkText(c)}</span>
        </li>
      ))}
    </ul>
  );
}

/* ───────── the parts of the card ───────── */

function Options(props: { v: TunnelView }): JSX.Element {
  const s = props.v.settings;
  const pw = props.v.password_set;
  return (
    <div class="tn-opts">
      <label class={`tn-opt${pw ? '' : ' off'}`}>
        <input type="checkbox" id="tn-full" checked={s.full_allowed && pw} disabled={!pw} onChange={(e) => { void setGateSettings({ full_allowed: (e.currentTarget as HTMLInputElement).checked }); }} />
        <span>{t('tun.opt.full')}<small>{pw ? t('tun.opt.fullNote') : <>{t('tun.opt.fullNoPw')} <code>scripts/install.sh password</code></>}</small></span>
      </label>
      <label class="tn-opt">
        <input type="checkbox" id="tn-confirm" checked={s.confirm_devices} onChange={(e) => { void setGateSettings({ confirm_devices: (e.currentTarget as HTMLInputElement).checked }); }} />
        <span>{t('tun.opt.confirm')}<small>{t('tun.opt.confirmNote')}</small></span>
      </label>
      <label class="tn-row">
        <span class="note">{t('tun.opt.idle')}</span>
        <span class="grow" />
        <select class="field sm" id="tn-idle" value={String(s.idle_close_h)} onChange={(e) => { void setGateSettings({ idle_close_h: Number((e.currentTarget as HTMLSelectElement).value) }); }}>
          {[1, 4, 12, 0].map((h) => <option key={h} value={String(h)}>{t(`tun.idle.${h}`)}</option>)}
        </select>
      </label>
    </div>
  );
}

function Foot(props: { v: TunnelView; extra?: JSX.Element }): JSX.Element {
  const left = props.v.idle_closes_ms ? props.v.idle_closes_ms - now.value : 0;
  return (
    <>
      <hr />
      <div class="tn-row">{props.extra}<span class="grow" /><button class="btn danger" type="button" id="tn-close" onClick={() => { void closeGate(); }}>{t('tun.closeGate')}</button></div>
      {left > 0 && <div class="note faint">{t('tun.idleLeft', { t: long(left) })}</div>}
      <div class="note faint">{t('tun.cf.note')}</div>
    </>
  );
}

function Devices(props: { list: TunnelDevice[] }): JSX.Element | null {
  if (!props.list.length) return null;
  return (
    <>
      {props.list.map((d) => (
        <div class="tn-dev" key={d.id} data-id={d.id}>
          <span class="ic"><Gl n={d.touch ? 'phone' : 'pc'} /></span>
          <div>
            <div class="tt">{d.label} <span class={`tn-badge ${d.scope}`}>{t(`tun.dev.${d.scope}`)}</span></div>
            <div class="ss">{[d.country, t('tun.dev.in', { at: hhmm(d.since_ms) }), t('tun.dev.last', { ago: ago(d.last_ms) })].filter(Boolean).join(' · ')}</div>
          </div>
          <button class="btn sm danger" type="button" data-act="revoke" onClick={() => { void decideDevice(d.id, 'revoke'); }}>{t('tun.dev.revoke')}</button>
        </div>
      ))}
    </>
  );
}

function Waiting(props: { list: TunnelDevice[] }): JSX.Element | null {
  if (!props.list.length) return null;
  return (
    <>
      {props.list.map((d) => (
        <div class="tn-box warn" key={d.id} data-id={d.id} role="group" aria-label={t('tun.pend.title')}>
          <div class="tn-row"><b class="ttl">{t('tun.pend.title')}</b></div>
          <div class="tn-dev bare">
            <span class="ic"><Gl n={d.touch ? 'phone' : 'pc'} /></span>
            <div><div class="tt">{d.label}</div><div class="ss">{[d.country, t('tun.pend.used')].filter(Boolean).join(' · ')}</div></div>
          </div>
          <div class="note">{t('tun.pend.ask')}</div>
          <div class="tn-row gap">
            <button class="btn go sm" type="button" data-act="allow" onClick={() => { void decideDevice(d.id, 'allow'); }}>{t('tun.pend.allow')}</button>
            <button class="btn sm danger" type="button" data-act="deny" onClick={() => { void decideDevice(d.id, 'deny'); }}>{t('tun.pend.deny')}</button>
            <span class="grow" />
            {d.answer_by_ms && <span class="note faint">{t('tun.pend.auto', { t: clock(d.answer_by_ms - now.value) })}</span>}
          </div>
        </div>
      ))}
    </>
  );
}

/** The link with its key hidden, the button that copies the whole, and the one that opens it here. */
function LinkRow(props: { link: string }): JSX.Element {
  const i = props.link.indexOf('#k=');
  return (
    <div class="tn-link">
      <code id="tn-link" title={t('tun.link.tip')}>{i < 0 ? props.link : props.link.slice(0, i)}{i >= 0 && <span class="tk">#k=••••••••••••</span>}</code>
      <button class="btn sm" type="button" id="tn-copy" onClick={() => copyText(props.link)}><Gl n="copy" /><span>{t('tun.link.copy')}</span></button>
      <a class="btn sm icon" id="tn-here" href={props.link} target="_blank" rel="noopener noreferrer" title={t('tun.link.here')} aria-label={t('tun.link.here')}><Gl n="ext" /></a>
    </div>
  );
}

function Link(props: { v: TunnelView }): JSX.Element | null {
  const v = props.v;
  if (!v.link) return null;
  const fresh = v.link_state === 'fresh' && (v.link_expires_ms ?? 0) > now.value;
  const expired = v.link_state === 'expired' || (v.link_state === 'fresh' && !fresh);
  return (
    <>
      <LinkRow link={v.link} />
      {fresh && v.qr_svg && (
        <div class="tn-qrbox">
          <div class="tn-qr" role="img" aria-label={t('tun.qr.aria')} dangerouslySetInnerHTML={{ __html: v.qr_svg }} />
          <div>
            <div class="note">{t('tun.qr.hint')}</div>
            <div class="cnt" id="tn-left">{clock((v.link_expires_ms ?? 0) - now.value)}</div>
            <div class="note faint">{t('tun.qr.left')}</div>
            <div class="mt"><button class="btn sm" type="button" id="tn-rotate" onClick={() => { void rotateLink(); }}>{t('tun.rotate')}</button></div>
          </div>
        </div>
      )}
      {!fresh && <div class="note faint">{expired ? t('tun.link.expired') : t('tun.link.used')}</div>}
    </>
  );
}

function ModeSwitch(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  return (
    <div class="tn-mode" role="group" aria-label={t('tun.mode')}>
      {(['quick', 'named'] as const).map((m) => (
        <button key={m} type="button" data-mode={m} class={v.mode === m ? 'on' : ''} aria-pressed={v.mode === m} onClick={() => { if (v.mode !== m) void saveGateConfig({ mode: m }); }}>{t(`tun.mode.${m}`)}</button>
      ))}
    </div>
  );
}

function NamedFields(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  const cc = v.config_check;
  const dirty = draftConfig.value !== null || draftHost.value !== null;
  return (
    <div class="tn-named">
      <label class="tn-f">
        <span>{t('tun.cfg')}</span>
        <input class="field" id="tn-config" type="text" spellcheck={false} autocomplete="off" value={shownConfig()} placeholder="~/.cloudflared/herdr-planner.yml"
          onInput={(e) => { draftConfig.value = (e.currentTarget as HTMLInputElement).value; checked.value = null; }} />
      </label>
      <label class="tn-f">
        <span>{t('tun.host')}</span>
        <input class="field" id="tn-host" type="text" spellcheck={false} autocomplete="off" value={shownHost()} placeholder="planner.example.com"
          onInput={(e) => { draftHost.value = (e.currentTarget as HTMLInputElement).value; checked.value = null; }} />
        <small>{t('tun.hostHint')}</small>
      </label>
      <div class="tn-row gap">
        <button class="btn sm" type="button" id="tn-save" disabled={!dirty} onClick={() => { void saveDrafts(); }}>{t('tun.save')}</button>
        <button class="btn sm" type="button" id="tn-check" onClick={() => { void checkGate(); }}>{t('tun.check')}</button>
      </div>
      {cc && !dirty && (checked.value || !cc.ok) && v.config && (
        <>
          <CheckList checks={cc.checks} />
          {checked.value && <div class="note faint">{t('tun.checked')}</div>}
        </>
      )}
    </div>
  );
}

/** The mode, and for a named tunnel its fields: only while no tunnel runs. */
function Setup(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  return (
    <div class="tn-setup">
      <ModeSwitch v={v} />
      <div class="note">{t(`tun.mode.${v.mode}Note`)}</div>
      {v.mode === 'named' && <NamedFields v={v} />}
    </div>
  );
}

function OpenButton(props: { v: TunnelView; label?: string }): JSX.Element {
  const v = props.v;
  const needConfig = v.mode === 'named' && shownConfig().trim() === '';
  return (
    <button class="btn go" type="button" id="tn-open" disabled={needConfig} title={needConfig ? t('tun.needConfig') : undefined}
      onClick={async () => { if (await saveDrafts()) await openGate(); }}>{props.label ?? t('tun.open')}</button>
  );
}

function Closed(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  return (
    <>
      <Setup v={v} />
      <div class="note">{t(v.mode === 'named' ? 'tun.closed.named' : 'tun.closed.quick')}</div>
      {!v.password_set && <div class="tn-box"><span class="note">{t('tun.noPassword')}</span></div>}
      <Options v={v} />
      <div class="tn-row"><OpenButton v={v} /><span class="grow" />{v.version && <span class="note faint">{v.version.replace(/^cloudflared version\s+/i, '')}</span>}</div>
    </>
  );
}

function Starting(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  const tick = Math.max(0, Math.floor((now.value - v.since_ms) / 1000));
  const addressKnown = !!v.host || v.mode === 'named';
  const steps: [boolean, boolean, string][] = [
    [tick >= 1, true, t('tun.start.1')],
    [addressKnown, tick >= 1, t(v.mode === 'named' ? 'tun.start.2.named' : 'tun.start.2.quick')],
    [false, addressKnown, t('tun.start.3')],
  ];
  return (
    <>
      <ol class="tn-steps">
        {steps.map(([done, active, text], i) => (
          <li key={i} class={done ? 'done' : active ? 'now' : ''}>
            <span class="m" aria-hidden="true">{done ? '✓' : active ? '●' : '○'}</span>{text}
          </li>
        ))}
      </ol>
      <div class="tn-row"><span class="note faint">{t('tun.start.limit')}</span><span class="grow" /><button class="btn sm" type="button" id="tn-cancel" onClick={() => { void closeGate(); }}>{t('cancel')}</button></div>
    </>
  );
}

function Open(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  const waiting = waitingIn(v);
  return (
    <>
      <Waiting list={waiting} />
      <Devices list={devicesIn(v)} />
      <Link v={v} />
      {v.link_state !== 'fresh' && <div><button class="btn sm" type="button" id="tn-rotate" onClick={() => { void rotateLink(); }}>{t('tun.rotate')}</button></div>}
      <Options v={v} />
      <Foot v={v} />
    </>
  );
}

function Reconnecting(props: { v: TunnelView }): JSX.Element {
  return (
    <>
      <div class="tn-box warn"><b class="ttl">{t('tun.recon.title')}</b><div class="note">{t('tun.recon.text')}</div></div>
      <Devices list={devicesIn(props.v)} />
      <Foot v={props.v} />
    </>
  );
}

function logText(v: TunnelView): string {
  const e = v.error;
  return e ? [e.message, ...e.log_tail].join('\n') : '';
}

function Failed(props: { v: TunnelView }): JSX.Element {
  const v = props.v;
  const e = v.error;
  const kind = e?.kind ?? 'spawn';
  const known = ['config-missing', 'no-hostname', 'hostname-mismatch', 'spawn', 'timeout', 'exited', 'lost'].includes(kind) ? kind : 'spawn';
  const hint = known === 'exited' ? t(`tun.err.exited.${v.mode}`) : t(`tun.err.${known}.hint`);
  const tunnelHint = known === 'timeout' || known === 'exited' || known === 'lost';
  const text = logText(v);
  return (
    <>
      <div class="tn-box bad"><b class="ttl">{t(`tun.err.${known}`)}</b><div class="note">{hint}</div></div>
      {text && <pre class="tn-log" tabindex={0}>{text}</pre>}
      {tunnelHint && <div class="note faint">{t('tun.err.tun')}</div>}
      <Setup v={v} />
      <div class="tn-row gap"><OpenButton v={v} label={t('tun.retry')} />{text && <button class="btn" type="button" id="tn-copylog" onClick={() => copyText(text)}><Gl n="copy" />{t('tun.copyLog')}</button>}</div>
    </>
  );
}

function Missing(): JSX.Element {
  return (
    <>
      <div class="tn-box"><b class="ttl">{t('tun.miss.title')}</b><div class="note">{t('tun.miss.where')}</div></div>
      <div class="note">{t('tun.miss.arch')}</div><CopyCmd text="sudo pacman -S cloudflared" />
      <div class="note">{t('tun.miss.mac')}</div><CopyCmd text="brew install cloudflared" />
      <div class="note">{t('tun.miss.deb')}</div><CopyCmd text="sudo apt install cloudflared" />
      <div class="note">{t('tun.miss.other')} <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer">{t('tun.miss.releases')}</a> · <a href={PACKAGES_URL} target="_blank" rel="noopener noreferrer">{t('tg.install.packages')}</a>. {t('tun.miss.path')}</div>
      <div class="tn-row"><button class="btn go" type="button" id="tn-recheck" onClick={() => { void loadTunnel(); }}>{t('tun.miss.recheck')}</button></div>
    </>
  );
}

function External(props: { v: TunnelView }): JSX.Element {
  return (
    <>
      <div class="tn-box warn"><b class="ttl">{t('tun.ext.title')}</b><div class="note">{t('tun.ext.text', { pid: props.v.external_pid ?? '' })}</div></div>
      <Setup v={props.v} />
      <div class="tn-row"><button class="btn go" type="button" id="tn-recheck" onClick={() => { void loadTunnel(); }}>{t('tun.miss.recheck')}</button></div>
    </>
  );
}

function Body(props: { v: TunnelView; k: Kind }): JSX.Element {
  const { v, k } = props;
  switch (k) {
    case 'starting': return <Starting v={v} />;
    case 'closing': return <div class="note">{t('tun.sub.closing')}</div>;
    case 'open': case 'pending': return <Open v={v} />;
    case 'reconnecting': return <Reconnecting v={v} />;
    case 'failed': return <Failed v={v} />;
    case 'missing': return <Missing />;
    case 'external': return <External v={v} />;
    default: return <Closed v={v} />;
  }
}

/** The card sits under the chip; when it would stick out at the right it is moved back inside the window. */
function usePlace(open: boolean, pop: { current: HTMLElement | null }): void {
  useLayoutEffect(() => {
    const el = pop.current;
    if (!open || !el) return;
    const place = (): void => {
      el.style.setProperty('--tn-dx', '0px');
      const r = el.getBoundingClientRect();
      let dx = Math.min(0, window.innerWidth - 8 - r.right);
      if (r.left + dx < 8) dx = 8 - r.left;
      el.style.setProperty('--tn-dx', `${Math.round(dx)}px`);
    };
    place();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(place);
    ro.observe(el);
    window.addEventListener('resize', place);
    return () => { ro.disconnect(); window.removeEventListener('resize', place); };
  }, [open]);
}

export function Tunnel(): JSX.Element | null {
  const v = tunnelView.value;
  const open = tunnelCard.value;
  const wrap = useRef<HTMLDivElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  useEffect(() => {
    void loadTunnel();
    const seen = (): void => { if (!document.hidden) void loadTunnel(); };
    document.addEventListener('visibilitychange', seen);
    return () => document.removeEventListener('visibilitychange', seen);
  }, []);
  useEffect(() => {
    if (!open) return;
    now.value = Date.now();
    const id = window.setInterval(() => { now.value = Date.now(); }, 1000);
    const away = (e: MouseEvent): void => { if (!wrap.current?.contains(e.target as Node)) closeTunnelCard(); };
    document.addEventListener('mousedown', away);
    pop.current?.focus({ preventScroll: true });
    return () => { window.clearInterval(id); document.removeEventListener('mousedown', away); };
  }, [open]);
  usePlace(open, pop);
  if (!v) return null;
  const k = kindOf(v);
  const waiting = waitingIn(v).length;
  const active = devicesIn(v).length;
  const count = k === 'pending' ? waiting : k === 'open' || k === 'reconnecting' ? active : 0;
  const word = t(`tun.st.${k}`);
  const state = `${word}${count ? ` · ${count}` : ''}`;
  return (
    <div class="tn-wrap" id="gate" ref={wrap}>
      <button
        type="button" class="btn gatechip" id="gate-chip" data-s={k} data-form="full" aria-haspopup="dialog" aria-expanded={open}
        aria-label={t('tun.aria', { state })} title={t('tun.aria', { state })}
        onClick={() => { if (open) closeTunnelCard(); else { tunnelCard.value = true; void loadTunnel(); } }}
      >
        <Gl n="gate" /><span class="nm">{t('tun.name')}</span><span class="dot" aria-hidden="true" /><span class="st">{word}</span>
        {count > 0 && <span class="n">{count}</span>}
      </button>
      {open && (
        <div class="panel tn-pop" id="tn-pop" role="dialog" aria-label={t('tun.name')} data-s={k} tabindex={-1} ref={pop}>
          <div class="tn-head">
            <b>{t('tun.name')}</b><span>{subOf(v, k)}</span><span class="grow" />
            <button class="btn sm" type="button" id="tn-help" onClick={() => openTunnelGuide()}><Gl n="help" />{t('tun.help')}</button>
            <button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => closeTunnelCard()}><Gl n="close" /></button>
          </div>
          <Body v={v} k={k} />
        </div>
      )}
    </div>
  );
}
