import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import {
  aim, edge, flight, linkStatus, locked, mode, panels, snap, speed, webgl, selected, galaxyId, level, linkMode,
} from '../store';
import { t } from '../i18n';
import { refreshNow } from '../api';
import { enterGalaxy, goHome } from '../nav';
import { Gl } from './Gl';
import { FlightStationLine } from './Arrival';
import './../styles/overlay.css';

export function Banner(): JSX.Element | null {
  const s = snap.value;
  if (!s || s.board.ok) return null;
  return (
    <div class="banner" id="banner">
      <Gl n="failed" />
      <span class="grow">{t('banner.down', { at: new Date(s.board.stale_at || s.at).toLocaleTimeString() })}</span>
      <button class="btn sm" type="button" data-act="refresh" onClick={() => { void refreshNow(); }}><Gl n="refresh" />{t('banner.retry')}</button>
    </div>
  );
}

export function NoGl(): JSX.Element | null {
  if (webgl.value) return null;
  return <div class="nogl" id="nogl"><b>{t('nogl.title')}</b><span>{t('nogl.text')}</span></div>;
}

/** The ship's state: the edge of the Galaxy, the mouse lock, the crosshair, the speed. */
export function FlightHud(): JSX.Element | null {
  const manual = mode.value === 'manual';
  const a = aim.value;
  const ring = Math.round(a.hold * 100);
  return (
    <>
      {edge.value && manual && <div class="notice edge" id="edge">{t('flight.edge')}</div>}
      {manual && locked.value && <div class="notice lock" id="lockhint">{t('flight.locked')}</div>}
      {manual && (
        <div class={`cross ${a.entity ? 'aimed' : ''}`} id="cross" aria-hidden="true">
          <div class="ring">
            <svg viewBox="0 0 36 36"><circle cx="18" cy="18" r="15" fill="none" stroke="var(--pl-accent)" stroke-width="2.5" stroke-dasharray={`${(ring / 100) * 94} 94`} /></svg>
          </div>
          {a.entity && <div class="name">{aimLabel()}</div>}
        </div>
      )}
      {manual && (
        <div class="speedo" id="speedo" aria-hidden="true">
          <span>{t('flight.speed')}</span>
          <b>{Math.round(speed.value.now)}</b>
          <div class="bar"><i style={`--w:${Math.min(100, (speed.value.now / Math.max(1, speed.value.cruise * 2.4)) * 100)}%`} /></div>
          <span>{t('flight.cruise', { n: Math.round(speed.value.cruise) })}</span>
        </div>
      )}
    </>
  );
}

import { taskOf } from '../store';
function aimLabel(): string {
  const e = aim.value.entity;
  if (!e) return '';
  if (e.kind === 'planet') return taskOf(e.id, e.galaxy)?.tag ?? e.id;
  if (e.kind === 'quasar') return t('quasar.label');
  if (e.kind === 'galaxy') return e.id || (snap.value?.host ?? '');
  if (e.kind === 'moon') return e.id.split('#')[1];
  return e.id;
}

const phaseWord = (p: string): string => t(`flight.${p}`);

/** The connection to a far galaxy: «устанавливаю связь…», «на связи», «нет связи: причина». */
export function LinkNotice(): JSX.Element | null {
  const ls = linkStatus.value;
  const f = flight.value;
  // Flying to a far galaxy: its target is the galaxy itself (`kind: 'galaxy'`, id = label).
  const flyingTo = f.target && typeof f.target === 'object' && f.target.kind === 'galaxy' && f.target.id ? f.target.id : '';
  if (ls.state === 'idle') {
    return f.phase !== 'idle' && f.phase !== 'accel' ? <div class="notice link" id="link"><span class="spinner" /><b>{phaseWord(f.phase)}</b>{flyingTo && <FlightStationLine label={flyingTo} />}</div> : null;
  }
  if (ls.state === 'connecting') {
    return <div class="notice link" id="link"><span class="spinner" /><b>{t('link.connecting', { g: ls.label })}</b><FlightStationLine label={ls.label} /></div>;
  }
  if (ls.state === 'online') {
    return <div class="notice link" id="link"><Gl n="galaxy" /><b>{t('link.online', { g: ls.label })}</b><span class="hint">{ls.version ? `herdr-projects ${ls.version} · ` : ''}{t('link.allHere')}</span>
      <button class="btn sm" type="button" onClick={() => void goHome()}>{t('link.home')}</button></div>;
  }
  return (
    <div class="notice link bad" id="link">
      <Gl n="failed" /><b>{t('link.offline', { g: ls.label, why: ls.why ?? '' })}</b>
      <ul class="why">{(ls.detail ?? []).map((d, i) => <li key={i}>{d}</li>)}</ul>
      <div class="row"><button class="btn sm primary" type="button" id="link-retry" onClick={() => void enterGalaxy(ls.label)}>{t('galaxy.retry')}</button>
        <button class="btn sm" type="button" onClick={() => { linkStatus.value = { state: 'idle', label: '' }; void goHome(); }}>{t('link.home')}</button></div>
    </div>
  );
}

const hintClosed = signal(((): boolean => { try { return localStorage.getItem('herdr-planner.hint') === 'closed'; } catch { return false; } })());

/** The little card with the keys: shown until the operator closes it. */
export function HintCard(): JSX.Element | null {
  const manual = mode.value === 'manual';
  // In the cabin (something selected) the keys card gives its place to the navigator and the chat.
  if (hintClosed.value || (!manual && selected.value)) return null;
  return (
    <div class="panel hintcard" id="hint" role="note">
      <button class="x" type="button" aria-label={t('close')} onClick={() => { hintClosed.value = true; try { localStorage.setItem('herdr-planner.hint', 'closed'); } catch { /* private */ } }}>✕</button>
      <h5>{manual ? t('hint.manual') : t('hint.auto')}</h5>
      <div class="keys">
        {manual ? (
          <>
            <span><kbd>W</kbd><kbd>A</kbd><kbd>S</kbd><kbd>D</kbd></span><span>{t('hint.wasd')}</span>
            <span><kbd>Space</kbd> <kbd>C</kbd></span><span>{t('hint.updown')}</span>
            <span><kbd>Shift</kbd></span><span>{t('hint.boost')}</span>
            <span>{t('hint.mouse')}</span><span>{t('hint.mouseText')}</span>
            <span>{t('hint.click')}</span><span>{t('hint.clickText')}</span>
            <span><kbd>P</kbd></span><span>{t('hint.autopilot')}</span>
          </>
        ) : (
          <>
            <span><kbd>N</kbd></span><span>{t('hint.navigator')}</span>
            <span>{t('hint.drag')}</span><span>{t('hint.dragText')}</span>
            <span>{t('hint.wheel')}</span><span>{t('hint.wheelText')}</span>
            <span><kbd>Ctrl</kbd><kbd>K</kbd></span><span>{t('hint.search')}</span>
            <span><kbd>P</kbd></span><span>{t('hint.manual2')}</span>
            <span><kbd>Esc</kbd></span><span>{t('hint.esc')}</span>
          </>
        )}
      </div>
    </div>
  );
}

export function HerdrPop(): JSX.Element | null {
  if (!panels.herdr.value) return null;
  const s = snap.value;
  return (
    <div class="panel pop" style="top:56px;right:14px;width:360px;padding:12px;z-index:60" role="dialog">
      <b>{t('herdr.title')}</b>
      <p class="hint">{t('herdr.text', { state: s?.herdr.running ? t('herdr.running') : t('herdr.notRunning') })}</p>
      <button class="btn sm" type="button" onClick={() => { panels.herdr.value = false; }}>{t('close')}</button>
    </div>
  );
}

void selected; void galaxyId; void level; void linkMode;
