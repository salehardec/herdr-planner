import type { JSX } from 'preact';
import { answered, panels, requests, snap } from '../store';
import { fromQueue, openRequest, queueCursor } from '../nav';
import { t } from '../i18n';
import { Gl } from './Gl';
import { Grip } from './Grip';
import { BatchBar } from './MemoryBatch';
import { cardPick, memPick, toggleCard } from './memory-batch';
import './wait.css';

/** «Ждёт тебя» as a queue to work through: open one, answer, and come back to the list with the cursor on the next. */
export function Wait(): JSX.Element {
  const open = panels.wait.value;
  const list = requests.value.filter((r) => r.type !== 'coordinator-options');
  const suggestions = snap.value?.memory_suggestions ?? [];
  void memPick.value;
  const cur = Math.min(queueCursor.value, Math.max(0, list.length - 1));
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'ArrowDown') { e.preventDefault(); queueCursor.value = Math.min(list.length - 1, cur + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); queueCursor.value = Math.max(0, cur - 1); }
    else if (e.key === 'Enter') { e.preventDefault(); void openRequest(cur); }
  };
  return (
    <section class={`panel waitlist ${open ? 'open' : ''}`} id="drawer-wait" aria-label={t('wait.title')} aria-hidden={!open} onKeyDown={onKey}>
      <header><span>{t('wait.title')} <kbd>B</kbd></span><span>{list.length}</span><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.wait.value = false; }}><Gl n="close" /></button></header>
      <div class="wlist" tabIndex={0}>
        {list.length === 0 && <div class="none">{t('wait.none')}</div>}
        {list.map((r, i) => {
          const alarm = r.type === 'runner-alert';
          const mem = r.type === 'memory';
          const done = !alarm && answered.value.has(r.id);
          const sug = mem ? suggestions.find((m) => m.task === r.id) : undefined;
          const state = sug ? cardPick(sug) : 'none';
          const row = (
            <button
              key={r.key} type="button" data-request={r.key} data-alert={r.alert?.kind} data-sel={r.id === 'coordinator' ? '' : r.id}
              class={`q s-${alarm ? 'failed' : mem ? 'done' : r.task?.status ?? 'waiting'} ${mem ? 'mem' : ''} ${i === cur ? 'cur' : ''} ${done ? 'resolved' : ''}`}
              title={mem ? t('req.memoryTip', { tag: r.tag, repo: r.repo }) : undefined}
              aria-selected={i === cur} onClick={() => void openRequest(i)}
            >
              <Gl n={alarm ? 'yard' : mem ? 'memory' : r.task?.status ?? 'waiting'} /><b>{r.tag}</b>
              <span class="tx">{mem && <span class="qk">{r.title}</span>}{r.text || r.title}</span><span class="pj">{r.repo}</span>
            </button>
          );
          if (!sug) return row;
          return (
            <div class="qw" key={r.key}>
              <input
                class="mm-pick" type="checkbox" data-pick-card={sug.task} checked={state === 'all'} ref={(el) => { if (el) el.indeterminate = state === 'some'; }}
                aria-label={t('mem.batchPickCard', { tag: sug.tag })} onChange={() => toggleCard(sug)}
              />
              {row}
            </div>
          );
        })}
      </div>
      {suggestions.length > 0 && <div class="wbatch"><BatchBar scope="wait" list={suggestions} /></div>}
      {fromQueue.value && <div class="hint wnote">{t('wait.back')}</div>}
      <Grip panel="wait" side="right" />
    </section>
  );
}
