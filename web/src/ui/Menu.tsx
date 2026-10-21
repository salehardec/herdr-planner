import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import {
  linkMode, NOTIFY_KEY, INVERT_KEY, SENS_KEY, SIMPLE_KEY, invertY, notifyOn, panels, persist, sensitivity, showQuiet, simple, snap, QUERY, requests, galaxyId,
} from '../store';
import { lang, setLang, t } from '../i18n';
import { post, refreshNow } from '../api';
import { toast } from '../toast';
import { Gl } from './Gl';
import { addDialogs } from '../dialogs';
import { openAddUniverse } from './AddUniverse';
import { setLinkMode } from '../keys';
import { getWorld, goHome } from '../nav';
import { openShip, openStation } from './station-api';

const open = signal(false);

export function Menu(): JSX.Element {
  const s = snap.value;
  const close = (): void => { open.value = false; };
  const item = (icon: string, label: string, on: () => void, pressed?: boolean, key?: string): JSX.Element => (
    <button type="button" role="menuitem" aria-pressed={pressed} onClick={() => { on(); close(); }}>
      <Gl n={icon} />{label}{key ? <kbd>{key}</kbd> : null}
    </button>
  );
  return (
    <div class="menu">
      <button class="btn icon" id="menu-btn" type="button" aria-haspopup="menu" aria-expanded={open.value} aria-label={t('menu.title')} title={t('menu.title')} onClick={() => { open.value = !open.value; }}>
        <Gl n="planner" />
      </button>
      {open.value && (
        <div class="panel menupop" role="menu" onMouseLeave={() => undefined}>
          {item('refresh', t('menu.refresh'), () => { void post('/api/refresh').catch(() => {}); void refreshNow(); toast(t('toast.refreshed')); })}
          {item('home', t('menu.home'), () => { void goHome(); }, undefined, 'H')}
          {item('list', t('menu.log'), () => { panels.log.value = !panels.log.value; }, panels.log.value, 'J')}
          {item('archive', t('menu.archive'), () => { panels.archive.value = !panels.archive.value; panels.focus.value = true; }, panels.archive.value, 'R')}
          {item('list', t('menu.todo'), () => { panels.todo.value = !panels.todo.value; }, panels.todo.value, 'T')}
          {item('limit', t('menu.limits'), () => { panels.limits.value = !panels.limits.value; }, panels.limits.value, 'Q')}
          {item('link', t('menu.linkMode'), () => { setLinkMode(!linkMode.value); }, linkMode.value, 'L')}
          {item('universe', t('menu.quiet'), () => { showQuiet.value = !showQuiet.value; }, showQuiet.value)}
          <hr />
          {item('globe', t('menu.addUniverse'), () => { openAddUniverse(''); })}
          {item('galaxy', t('menu.addGalaxy'), () => { addDialogs.galaxy.value = true; })}
          {item('universe', galaxyId.value === '' ? t('menu.station') : t('menu.stationOf', { g: galaxyId.value }), () => { openStation(galaxyId.value); })}
          {item('plan', t('menu.crewWizard'), () => { openStation('', true); })}
          {item('ship', t('menu.ship'), () => { openShip(); })}
          <hr />
          <div class="setrow">
            <span>{t('menu.language')}</span>
            <div class="row">
              <button type="button" class={`btn sm ${lang.value === 'ru' ? 'on' : ''}`} data-lang="ru" onClick={() => setLang('ru')}>{t('lang.ru')}</button>
              <button type="button" class={`btn sm ${lang.value === 'en' ? 'on' : ''}`} data-lang="en" onClick={() => setLang('en')}>{t('lang.en')}</button>
            </div>
          </div>
          {item('planner', t('menu.simple'), () => {
            if (QUERY.has('calm')) { toast(t('toast.calmFixed')); return; }
            simple.value = !simple.value; persist(SIMPLE_KEY, simple.value ? 'on' : 'off');
          }, simple.value)}
          {item('bell', t('menu.notify'), () => {
            notifyOn.value = !notifyOn.value; persist(NOTIFY_KEY, notifyOn.value ? 'on' : 'off');
            if (notifyOn.value && typeof Notification !== 'undefined' && Notification.permission === 'default') void Notification.requestPermission().catch(() => {});
          }, notifyOn.value)}
          <div class="setrow">
            <label for="sens">{t('menu.sensitivity')} <b>{sensitivity.value.toFixed(1)}</b></label>
            <input
              id="sens" type="range" min="0.3" max="3" step="0.1" value={sensitivity.value}
              onInput={(e) => { sensitivity.value = Number((e.target as HTMLInputElement).value); persist(SENS_KEY, String(sensitivity.value)); getWorld()?.setSensitivity(sensitivity.value, invertY.value); }}
            />
          </div>
          {item('up', t('menu.invertY'), () => { invertY.value = !invertY.value; persist(INVERT_KEY, invertY.value ? '1' : '0'); getWorld()?.setSensitivity(sensitivity.value, invertY.value); }, invertY.value)}
          <hr />
          {item('help', t('menu.help'), () => { panels.help.value = !panels.help.value; }, undefined, '?')}
          {item('screen', s?.herdr.running ? t('menu.herdrOn') : t('menu.herdrOff'), () => { panels.herdr.value = !panels.herdr.value; })}
          {s?.auth.required && (
            <>
              <span class="faint" style="padding:0.25rem 0.5rem">{s.auth.user}</span>
              {item('logout', t('menu.logout'), () => { void post('/api/logout').catch(() => {}).then(() => { location.href = '/login'; }); })}
            </>
          )}
        </div>
      )}
    </div>
  );
}

void requests;
