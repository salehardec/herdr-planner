/* The keys of an agent's dialog: what the board sends through `herdr agent send-keys`.
   The labels are the keys themselves (arrows for the arrows, capital letters for the letters), so a
   button label always starts with a capital or a sign. */
import type { JSX } from 'preact';
import { sendKey } from '../actions';
import { t } from '../i18n';

const KEYS = ['up', 'down', 'left', 'right', 'enter', 'esc', '1', '2', '3', 'y', 'n'] as const;
const LABEL: Record<string, string> = {
  up: '↑', down: '↓', left: '←', right: '→', enter: '↵ Enter', esc: 'Esc', y: 'Y', n: 'N',
};

export function KeyButtons(props: { target: string; role?: string; onSent?: () => void }): JSX.Element {
  return (
    <div class="keys" data-keys-of={props.target} aria-label={t('screen.keys')}>
      {KEYS.map((k) => (
        <button
          class="btn sm" type="button" key={k} data-act="keys" data-target={props.target} data-keys={k}
          onClick={() => void sendKey(props.target, k, props.role).then(() => props.onSent?.())}
        >
          {LABEL[k] ?? k}
        </button>
      ))}
    </div>
  );
}
