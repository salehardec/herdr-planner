import type { JSX } from 'preact';
import { currentView, filter, stageCount } from '../store';
import { t } from '../i18n';
import { getWorld } from '../nav';
import { count } from '../anim';
import type { Status } from '../types';
import './kanban.css';

export const STAGES: { k: string; label: string; st: Status[] }[] = [
  { k: 'queue', label: 'stage.queue', st: ['queued'] },
  { k: 'work', label: 'stage.work', st: ['plan', 'working', 'waiting', 'failed'] },
  { k: 'review', label: 'stage.review', st: ['review'] },
  { k: 'appr', label: 'stage.appr', st: ['ready'] },
  { k: 'done', label: 'stage.done', st: ['done'] },
];

const same = (a: Status[] | null, b: Status[]): boolean => !!a && a.length === b.length && b.every((x) => a.includes(x));

export function toggleStage(st: Status[]): void {
  filter.value = same(filter.value, st) ? null : st.slice();
  getWorld()?.setFilter(filter.value);
}

/** The line «Очередь · План и работа · Ревью · К одобрению · Сделано» with counts; a click filters the world. */
export function Kanban(): JSX.Element {
  const total = currentView.value.tasks.length;
  const waiting = currentView.value.tasks.filter((x) => x.status === 'waiting' || x.status === 'failed').length;
  return (
    <div class="panel kanban" id="kanban" role="group" aria-label={t('kanban.aria')}>
      {STAGES.map((g, i) => {
        const n = stageCount(g.st);
        const w = total ? Math.round((n / total) * 100) : 0;
        const on = same(filter.value, g.st);
        return (
          <>
            {i > 0 && <span class="arr" aria-hidden="true">→</span>}
            <button type="button" class={on ? 'on' : ''} data-stage={g.k} aria-pressed={on} onClick={() => toggleStage(g.st)}>
              <span>{t(g.label)}</span>
              <b ref={(el) => { if (el) count(el, n); }} />
              {g.k === 'work' && waiting > 0 && <span class="w">{t('kanban.waits', { n: waiting })}</span>}
              <i style={`--w:${w}%`} />
            </button>
          </>
        );
      })}
    </div>
  );
}
