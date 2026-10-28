import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { t } from '../i18n';
import { Gl } from './Gl';
import { CheckList, CopyCmd } from './Tunnel';
import { checkGate, checked, draftUuid, shownConfig, shownHost, tunnelGuide, tunnelView, type TunnelMode } from './tunnel-api';
import { ACCESS_URL, DASH_URL, PACKAGES_URL, RELEASES_URL, guideFor, isUuid, type Guide } from './tunnel-guide';
import './tunnel.css';

const link = (href: string, label: string): JSX.Element => <a href={href} target="_blank" rel="noopener noreferrer">{label}</a>;

function Install(props: { g: Guide }): JSX.Element {
  const g = props.g;
  return (
    <div class="tg-install">
      <div class="note">{t('tg.install.arch')}</div><CopyCmd text={g.install.pacman} />
      <div class="note">{t('tg.install.mac')}</div><CopyCmd text={g.install.brew} />
      <div class="note">{t('tg.install.deb')}</div><CopyCmd text={g.install.apt} />
      <div class="note faint">{t('tg.install.debNote')} {link(PACKAGES_URL, t('tg.install.packages'))} · {link(RELEASES_URL, t('tg.install.releases'))}</div>
    </div>
  );
}

function Step(props: { n: number; title: string; children: JSX.Element | JSX.Element[] }): JSX.Element {
  return (
    <li class="tg-step">
      <span class="num" aria-hidden="true">{props.n}</span>
      <div class="tg-body"><b class="tg-h">{props.title}</b>{props.children}</div>
    </li>
  );
}

function Quick(props: { g: Guide }): JSX.Element {
  return (
    <>
      <p class="note">{t('tg.q.intro')}</p>
      <ol class="tg-steps">
        <Step n={1} title={t('tg.q.s1')}>
          <p class="note">{t('tg.q.s1.text')}</p>
          <Install g={props.g} />
        </Step>
        <Step n={2} title={t('tg.q.s2')}><p class="note">{t('tg.q.s2.text')}</p></Step>
      </ol>
      <div class="tn-box warn">
        <b class="ttl">{t('tg.q.warn.title')}</b>
        <ul class="tg-list">
          <li>{t('tg.q.warn.addr')}</li>
          <li>{t('tg.q.warn.pw')}</li>
          <li>{t('tg.q.warn.cf')}</li>
        </ul>
      </div>
    </>
  );
}

function Named(props: { g: Guide }): JSX.Element {
  const g = props.g;
  const v = tunnelView.value;
  const cc = v?.config_check;
  return (
    <>
      <p class="note">{t('tg.n.intro')}</p>
      <div class={`tn-box${g.hostGiven && g.configGiven ? ' ok' : ''}`} id="tg-values">
        <span class="note">{g.hostGiven || g.configGiven ? t('tg.n.values', { host: g.host, config: g.config }) : t('tg.n.valuesNone')}</span>
      </div>
      <label class="tg-uuid">
        <span>{t('tg.n.uuid')}</span>
        <input class="field" id="tg-uuid" type="text" spellcheck={false} autocomplete="off" placeholder="00000000-0000-0000-0000-000000000000"
          value={draftUuid.value ?? (g.uuidGiven ? g.uuid : '')} onInput={(e) => { draftUuid.value = (e.currentTarget as HTMLInputElement).value.trim(); }} />
        <small>{draftUuid.value && !isUuid(draftUuid.value) ? t('tg.n.uuidBad') : t('tg.n.uuidHint')}</small>
      </label>
      <ol class="tg-steps">
        <Step n={1} title={t('tg.n.s1')}>
          <p class="note">{t('tg.n.s1.text', { host: g.host })} {link(DASH_URL, t('tg.n.s1.link'))}</p>
        </Step>
        <Step n={2} title={t('tg.n.s2')}>
          <p class="note">{t('tg.n.s2.text')}</p>
          <CopyCmd text={g.login} />
          <CopyCmd text={g.certCheck} />
        </Step>
        <Step n={3} title={t('tg.n.s3')}>
          <p class="note">{t('tg.n.s3.text')}</p>
          <CopyCmd text={g.create} />
        </Step>
        <Step n={4} title={t('tg.n.s4')}>
          <p class="note">{t('tg.n.s4.text', { config: g.config })}</p>
          <CopyCmd text={g.yaml} block />
        </Step>
        <Step n={5} title={t('tg.n.s5')}>
          <p class="note">{t('tg.n.s5.text', { host: g.host })}</p>
          <CopyCmd text={g.route} />
        </Step>
        <Step n={6} title={t('tg.n.s6')}>
          <p class="note">{t('tg.n.s6.text')}</p>
          <div class="tn-row gap"><button class="btn sm" type="button" id="tg-check" onClick={() => { void checkGate(); }}>{t('tg.n.check')}</button></div>
          {cc && checked.value && v?.config ? <CheckList checks={cc.checks} /> : <></>}
        </Step>
      </ol>
    </>
  );
}

function Trouble(props: { mode: TunnelMode; g: Guide }): JSX.Element {
  const { mode, g } = props;
  return (
    <section class="tg-trouble" aria-label={t('tg.t.title')}>
      <h3>{t('tg.t.title')}</h3>
      {mode === 'named' && (
        <>
          <div class="tg-t"><b>{t('tg.t.wrongTunnel')}</b><p class="note">{t('tg.t.wrongTunnel.text')}</p></div>
          <div class="tg-t"><b>{t('tg.t.wrongCreds')}</b><p class="note">{t('tg.t.wrongCreds.text')}</p></div>
        </>
      )}
      <div class="tg-t">
        <b>{t('tg.t.quic')}</b>
        <p class="note">{t('tg.t.quic.text')}</p>
        <CopyCmd text={g.http2} />
        {mode === 'quick' && <p class="note">{t('tg.t.quic.quick')} <code>{g.quickConfig}</code></p>}
      </div>
      <div class="tg-t"><b>{t('tg.t.stream')}</b><p class="note">{t('tg.t.stream.text')}</p></div>
      {mode === 'named' && (
        <div class="tg-t"><b>{t('tg.t.access')}</b><p class="note">{t('tg.t.access.text', { host: g.host })} {link(ACCESS_URL, t('tg.t.access.link'))}</p></div>
      )}
    </section>
  );
}

/** «How to set up»: the steps of both ways, with the owner's own address, config path and port put into the commands. */
export function TunnelGuide(): JSX.Element | null {
  const mode = tunnelGuide.value;
  const dlg = useRef<HTMLElement>(null);
  useEffect(() => { if (mode) dlg.current?.focus({ preventScroll: true }); }, [mode === null]);
  if (!mode) return null;
  const v = tunnelView.value;
  const fromConfig = v?.config_check?.checks.find((c) => c.id === 'tunnel')?.have ?? '';
  const g = guideFor({ config: shownConfig(), host: shownHost(), port: v?.port ?? 7117, home: v?.home ?? '', uuid: draftUuid.value !== null ? draftUuid.value : fromConfig });
  const close = (): void => { tunnelGuide.value = null; (document.getElementById('gate-chip') as HTMLElement | null)?.focus(); };
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel dlg wide tg" id="tunnel-guide" role="dialog" aria-label={t('tg.title')} tabindex={-1} ref={dlg}>
        <header><b>{t('tg.title')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={close}><Gl n="close" /></button></header>
        <div class="bd">
          <div class="tn-mode" role="group" aria-label={t('tun.mode')}>
            {(['quick', 'named'] as const).map((m) => (
              <button key={m} type="button" data-mode={m} class={mode === m ? 'on' : ''} aria-pressed={mode === m} onClick={() => { tunnelGuide.value = m; }}>{t(`tg.${m}`)}</button>
            ))}
          </div>
          {mode === 'quick' ? <Quick g={g} /> : <Named g={g} />}
          <Trouble mode={mode} g={g} />
        </div>
      </section>
    </div>
  );
}
