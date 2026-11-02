/* A words-only permission may have become a terminal dialog since the card was drawn. */
import type { JSX } from 'preact';
import { useRef, useState } from 'preact/hooks';
import { answerPermission, boardOk } from '../actions';
import { PERMISSION_ALLOW, PERMISSION_DENY } from '../requests';
import { t } from '../i18n';
import type { Task } from '../types';

export function PermissionWords(props: { task: Task; draft: (text: string) => void }): JSX.Element {
  const lock = useRef(false);
  const [busy, setBusy] = useState(false);
  const answer = async (note: 'allow' | 'deny'): Promise<void> => {
    if (lock.current || !boardOk()) return;
    lock.current = true;
    setBusy(true);
    try {
      if (await answerPermission(props.task, note) === 'words') {
        props.draft(note === 'allow' ? PERMISSION_ALLOW() : PERMISSION_DENY());
      }
    } finally { lock.current = false; setBusy(false); }
  };
  return (
    <>
      <button class="btn go" type="button" data-act="perm-allow" data-id={props.task.id}
        disabled={busy || !boardOk()} aria-busy={busy} onClick={() => void answer('allow')}>{t('req.allow')}</button>
      <button class="btn danger" type="button" data-act="perm-deny" data-id={props.task.id}
        disabled={busy || !boardOk()} aria-busy={busy} onClick={() => void answer('deny')}>{t('req.deny')}</button>
    </>
  );
}
