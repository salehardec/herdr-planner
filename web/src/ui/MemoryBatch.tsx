/* The bar over a list of «В память» proposals (the Waiting drawer, a universe's panel): select all / clear, and one button
   to keep or drop everything ticked. Keeping shows «было → стало» of every ticked card first and waits for one «да». */
import type { JSX } from 'preact';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { t } from '../i18n';
import type { MemorySuggestion } from '../types';
import { ChangeCard } from './MemoryChange';
import './memory.css';
import {
  batchAsk, batchCancel, batchDismiss, batchKeep, clearPick, memBatch, memPick, pickAll, picked,
} from './memory-batch';

export function BatchBar(props: { scope: string; list: MemorySuggestion[] }): JSX.Element | null {
  const { scope, list } = props;
  if (list.length === 0) return null;
  void memPick.value;
  const chosen = picked(list);
  const lines = chosen.reduce((n, c) => n + c.lines.length, 0);
  const b = memBatch.value?.scope === scope ? memBatch.value : null;
  const busy = !!memBatch.value?.busy;
  const worst = b ? (b.items.some((i) => i.p.risk === 'block') ? 'block' : b.items.some((i) => i.p.risk === 'confirm') ? 'confirm' : 'none') : 'none';
  const key = 'mem-batch-public';
  const armedPublic = armed.value?.key === key;
  const go = (): void => {
    if (worst === 'block') confirmStep(key, () => batchKeep());
    else void batchKeep();
  };
  const goLabel = worst === 'block' ? (armedPublic ? t('mem.chg.publicGo2') : t('mem.chg.publicGo')) : worst === 'confirm' ? t('mem.chg.riskGo') : t('mem.suggestKeep');
  return (
    <div class="mm-batch" data-batch={scope}>
      <div class="mm-batch-bar">
        <span class="mm-batch-n" data-batch-count={lines}>{lines ? t('mem.batchPicked', { n: lines, c: chosen.length }) : t('mem.batchNone')}</span>
        <button class="btn sm" type="button" data-act="mem-batch-all" disabled={busy} onClick={() => pickAll(list)}>{t('mem.batchAll')}</button>
        <button class="btn sm" type="button" data-act="mem-batch-clear" disabled={busy || lines === 0} onClick={() => clearPick(list)}>{t('mem.batchClear')}</button>
        <button class="btn sm primary" type="button" data-act="mem-batch-accept" disabled={busy || lines === 0 || !!b} onClick={() => void batchAsk(scope, list)}>{t('mem.suggestKeep')}</button>
        <button class="btn sm" type="button" data-act="mem-batch-dismiss" disabled={busy || lines === 0 || !!b} onClick={() => void batchDismiss(list)}>{t('mem.suggestNo')}</button>
      </div>
      {b && b.items.length > 0 && (
        <div class="mm-batch-card" data-batch-card>
          <div class="chg-h">{t('mem.batchChanges', { n: b.items.length })}</div>
          {b.items.map((i) => (
            <div class="mm-batch-one" key={i.s.task} data-sug={i.s.task}>
              <div class="hint"><b>{i.s.tag}</b> · {i.s.repo}</div>
              <ChangeCard p={i.p} busy={b.busy} error="" onConfirm={() => undefined} onCancel={batchCancel} bare />
            </div>
          ))}
          {b.err && <div class="hint bad" data-batch-error>{b.err}</div>}
          <div class="row act">
            <button class={`btn sm ${worst === 'block' ? 'danger' : 'primary'} ${armedPublic ? 'arm' : ''}`} type="button" data-act="mem-batch-confirm" disabled={b.busy} onClick={go}>{goLabel}</button>
            <button class="btn sm" type="button" data-act="mem-batch-cancel" disabled={b.busy} onClick={batchCancel}>{t('cancel')}</button>
          </div>
        </div>
      )}
      <div class="hint">{t('mem.batchNote')}</div>
    </div>
  );
}
