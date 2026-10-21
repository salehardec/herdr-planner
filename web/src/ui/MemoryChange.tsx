/* The card that stands between a memory change and the file: «было → стало» with the level and the file, what the check for
   personal data and secrets found, and the human's «да». Nothing is written before the confirmation. */
import type { JSX } from 'preact';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { t } from '../i18n';
import type { Accept, MemPreview } from './memory-api';

export function ChangeCard(props: {
  p: MemPreview;
  busy: boolean;
  error: string;
  onConfirm: (a: Accept) => void;
  onCancel: () => void;
  /** Puts the entry's text again without what was found (the caller asks for a new preview). */
  onRedact?: () => void;
  /** Offers the same text for the Galaxy's memory (local, not in git). */
  onToGalaxy?: () => void;
  /** The label of the confirm button when nothing was found. */
  confirmLabel?: string;
}): JSX.Element {
  const { p } = props;
  const where = p.level === 'galaxy' ? t('mem.chg.galaxy') : t('mem.chg.universe', { repo: p.repo });
  const publicKey = 'mem-chg-public';
  const armedPublic = armed.value?.key === publicKey;
  const hasRedact = props.onRedact && p.redacted.some((r) => r);
  const confirm = (): void => {
    if (p.risk === 'block') confirmStep(publicKey, () => props.onConfirm({ accept_public: true }));
    else if (p.risk === 'confirm') props.onConfirm({ accept_risk: true });
    else props.onConfirm({});
  };
  const label = p.risk === 'block'
    ? (armedPublic ? t('mem.chg.publicGo2') : t('mem.chg.publicGo'))
    : p.risk === 'confirm' ? t('mem.chg.riskGo') : props.confirmLabel ?? t('mem.chg.go');
  return (
    <div class={`chg mm-chg risk-${p.risk} ${props.error ? 'bad' : ''}`} id="mem-chg" data-risk={p.risk}>
      <div class="chg-h">{t('mem.chg.title')} · <span id="mem-chg-where">{where}</span></div>
      <div class="hint" id="mem-chg-dest">
        {p.commit ? t('mem.chg.destCommit') : p.level === 'galaxy' ? t('mem.chg.destGalaxy') : p.draft ? t('mem.chg.destDraft') : t('mem.chg.destFile')}
      </div>
      {p.changes.map((c) => (
        <div class="mm-chg-one" key={c.file} data-op={c.op}>
          <div class="ba">
            <span class={`mm-op op-${c.op}`}>{t(`mem.chg.op.${c.op}`)}</span>
            <code class="mm-chg-file" title={c.file}>{c.file}</code>
          </div>
          <div class="ba"><span class="kl">{t('mem.chg.was')}</span></div>
          <pre class="mm-block mm-was">{c.before ?? t('mem.chg.nothing')}</pre>
          <div class="ba"><span class="kl">{t('mem.chg.now')}</span></div>
          <pre class="mm-block mm-now">{c.after ?? t('mem.chg.removed')}</pre>
        </div>
      ))}
      {p.findings.length > 0 && (
        <div class={`mm-found ${p.risk}`} id="mem-chg-found" role="alert">
          <b>{p.risk === 'block' ? t('mem.chg.foundPublic') : t('mem.chg.found')}</b>
          <ul>
            {p.findings.map((f, i) => (
              <li key={i} data-kind={f.kind}>
                <span class="mm-fk">{t(`mem.chg.kind.${f.kind}`)}</span> <code>{f.text}</code>
                <span class="hint"> {p.changes.length > 1 ? `${f.file}, ` : ''}{t(`mem.chg.field.${f.field}`)}{f.line > 1 ? `:${f.line}` : ''}</span>
              </li>
            ))}
          </ul>
          <div class="hint">{p.risk === 'block' ? t('mem.chg.blockWhy', { v: t(`mem.chg.vis.${p.visibility ?? 'unknown'}`) }) : t('mem.chg.confirmWhy')}</div>
        </div>
      )}
      {props.error && <div class="hint bad" id="mem-chg-error">{props.error}</div>}
      <div class="row act">
        <button class={`btn ${p.risk === 'block' ? 'danger' : 'primary'} ${armedPublic ? 'arm' : ''}`} type="button" id="mem-chg-confirm" disabled={props.busy} onClick={confirm}>{label}</button>
        {hasRedact && <button class="btn" type="button" id="mem-chg-redact" disabled={props.busy} onClick={props.onRedact}>{t('mem.chg.redact')}</button>}
        {props.onToGalaxy && p.findings.length > 0 && <button class="btn" type="button" id="mem-chg-galaxy" disabled={props.busy} onClick={props.onToGalaxy}>{t('mem.chg.toGalaxy')}</button>}
        <button class="btn" type="button" id="mem-chg-cancel" disabled={props.busy} onClick={props.onCancel}>{t('cancel')}</button>
      </div>
    </div>
  );
}
