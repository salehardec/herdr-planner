import type { JSX } from 'preact';
import { currentView, tasksIndex } from '../store';
import { taskOpener } from '../task-open';
import { t } from '../i18n';
import { splitRefs } from '../chat-parse';
import { Gl } from './Gl';
import type { Task } from '../types';
import './taskchip.css';

/** The task a key or a `repo/n` names, on the board or in its archive. */
export function taskOf(ref: string): Task | undefined {
  const idx = tasksIndex.value;
  const byId = idx.get(ref);
  if (byId) return byId;
  for (const x of idx.values()) if (x.tag === ref) return x;
  return undefined;
}

export function goToTag(tag: string): void {
  const task = taskOf(tag);
  if (task) taskOpener.open(task);
}

/**
 * A task key as a chip: a dot of the state's colour, a card with the name and the state on hover and focus,
 * a click or Enter opens the task in the Focus. A key the board does not know is a chip without a dot.
 */
/** The card opens above the chip, or below it when the chip is near the top of its scrolling box (the card would be cut). */
const placeCard = (e: Event): void => {
  const el = e.currentTarget as HTMLElement;
  const box = el.closest('.log, .panel, .summary-scroll') as HTMLElement | null;
  const top = box ? box.getBoundingClientRect().top : 0;
  el.dataset.side = el.getBoundingClientRect().top - top < 176 ? 'down' : 'up';
};

export function TaskChip(props: { tag: string }): JSX.Element {
  const task = taskOf(props.tag);
  const shown = task?.tag ?? props.tag;
  const state = task ? (task.label || t(`status.${task.status}`)) : '';
  const tip = task ? `${shown} · ${task.title} · ${state}` : `${shown} · ${t('chat.noTask')}`;
  return (
    <button
      class={`k tk ${task ? `s-${task.status}` : 'none'}`} data-tag={shown} type="button" title={tip} aria-label={tip}
      onClick={(e) => { e.stopPropagation(); goToTag(props.tag); }}
      onMouseEnter={placeCard} onFocus={placeCard}
      onKeyDown={(e) => { if (e.key === 'Escape') { (e.currentTarget as HTMLElement).blur(); e.stopPropagation(); } }}
    >
      {task && <i class="dotc" />}
      {shown}
      <span class="kcard" aria-hidden="true">
        {task ? (
          <>
            <b>{shown} · {task.title}</b>
            <span class={`badge s-${task.status}`}><Gl n={task.status} />{state}</span>
            <span class="hint">{t('chat.openTask')} · Enter</span>
          </>
        ) : (
          <>
            <b>{shown}</b>
            <span class="hint">{t('chat.noTask')}</span>
          </>
        )}
      </span>
    </button>
  );
}

const known = (id: string): boolean => currentView.value.tasks.some((x) => x.id === id) || tasksIndex.value.has(id);

/** Text where every task key (and every `repo/n` of a known task) is a chip. */
export function RefText(props: { text: string }): JSX.Element {
  return <>{splitRefs(props.text, known).map((p, i) => (typeof p === 'string' ? p : <TaskChip key={i} tag={p.ref} />))}</>;
}
