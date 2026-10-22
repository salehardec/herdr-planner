/* The top bar's station: whose station it is (this machine's) and the ⚙ that opens it. */
import type { JSX } from 'preact';
import { snap } from '../store';
import { t } from '../i18n';
import { openStation } from './station-api';
import './stchip.css';

export function StationChip(): JSX.Element {
  const host = snap.value?.host ?? '';
  return (
    <div class="stchip" id="station-chip">
      <span class="long">{t('chip.own', { host })}</span>
      <span class="short">{host}</span>
      <button class="btn icon sm stgear" type="button" id="station-open" aria-label={t('chip.openHome')} title={t('chip.openHome')} onClick={() => openStation()}>⚙</button>
    </div>
  );
}
