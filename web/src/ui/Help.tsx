import type { JSX } from 'preact';
import { panels } from '../store';
import { t } from '../i18n';
import { Gl } from './Gl';
import { STATUS_ORDER } from '../types';

const ROWS: [string, string][] = [
  ['Ctrl+K', 'help.search'], ['P', 'help.mode'], ['/', 'help.chat'], ['N', 'help.nav'], ['F', 'help.focus'], ['T', 'help.todo'],
  ['B', 'help.wait'], ['J', 'help.log'], ['R', 'help.archive'], ['Q', 'help.limits'], ['L', 'help.link'], ['H', 'help.home'],
  ['0 / Home', 'help.overview'], ['Enter', 'help.fly'], ['Tab / Shift+Tab', 'help.tab'], ['[ / ]', 'help.universe'], ['1–9', 'help.nth'],
  ['+ / −', 'help.zoom'], ['W A S D / ← ↑ → ↓', 'help.wasd'], ['Space / C', 'help.updown'], ['Shift', 'help.boost'], ['X', 'help.stop'],
  ['Ctrl+= / Ctrl+− / Ctrl+0', 'help.scale'], ['Esc', 'help.esc'], ['?', 'help.help'],
];

export function Help(): JSX.Element | null {
  if (!panels.help.value) return null;
  return (
    <div class="panel help" id="help" role="dialog" aria-label={t('help.title')}>
      <h2>{t('help.title')}</h2>
      <p class="hint">{t('help.intro')}</p>
      <h3 class="lg-h">{t('legend.title')}</h3>
      <ul class="legend" id="legend">
        {STATUS_ORDER.map((s) => (
          <li key={s} class={`lg-row s-${s}`} data-status={s}>
            <span class="lg-k"><Gl n={s} cls={s === 'working' ? 'spin' : ''} />{t(`status.${s}`)}</span>
            <span class="lg-d">{t(`legend.${s}`)}</span>
          </li>
        ))}
      </ul>
      <p class="hint">{t('legend.motion')}</p>
      <h3 class="lg-h">{t('help.keys')}</h3>
      <table>
        <tbody>
          {ROWS.map(([k, v]) => <tr key={k}><td>{k}</td><td>{t(v)}</td></tr>)}
        </tbody>
      </table>
      <button class="btn" type="button" data-act="help" onClick={() => { panels.help.value = false; }}><Gl n="close" />{t('close')}</button>
    </div>
  );
}
