import type { JSX } from 'preact';
import { fetchScreen } from '../actions';
import { screens } from '../store';
import { t } from '../i18n';
import { Gl } from './Gl';

/** A late dialog is read on demand, never by polling a waiting card. fetchScreen also guards concurrent reads. */
export function ReadScreenButton(props: { target: string }): JSX.Element {
  const busy = !!screens.value[props.target]?.busy;
  return (
    <button class="btn" type="button" data-act="read-screen" data-target={props.target}
      disabled={busy} aria-busy={busy} onClick={() => void fetchScreen(props.target)}>
      <Gl n="refresh" />{busy ? t('req.reading') : t('req.rereadScreen')}
    </button>
  );
}
