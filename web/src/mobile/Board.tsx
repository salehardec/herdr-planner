/* The board on the phone: the tasks in heaps, a tap opens the sheet of a task. */
import type { JSX } from 'preact';
import { currentView } from '../store';
import { t } from '../i18n';
import { heaps, shareOf } from './board';
import { sheet } from './state';
import type { Task } from '../types';

function Row(props: { task: Task }): JSX.Element {
  const task = props.task;
  const share = shareOf(task);
  const sub = task.msg.trim() || task.board_label || task.label;
  return (
    <button class={`row s-${task.status}`} type="button" data-row={task.id} onClick={() => { sheet.value = { kind: 'task', id: task.id }; }}>
      <span class="dot" />
      <span class="rmid">
        <span class="t"><b>{task.tag}</b> {task.title}</span>
        {sub && <span class="s">{sub}</span>}
        {share !== null && task.status === 'working' && <span class="bar"><i style={`width:${share}%`} /></span>}
      </span>
      <span class="rt">{task.label || t(`status.${task.status}.short`)}</span>
    </button>
  );
}

export function BoardPage(): JSX.Element {
  const list = heaps(currentView.value.tasks);
  return (
    <section class="page" id="p-board" aria-label={t('m.tab.board')}>
      {list.map((h) => (
        <div key={h.key} data-heap={h.key}>
          <div class="sect">{t(`m.heap.${h.key}`)} · {h.tasks.length}<i /></div>
          {h.tasks.map((x) => <Row key={x.id} task={x} />)}
        </div>
      ))}
      {list.length === 0 && <div class="empty"><b>{t('m.boardEmpty')}</b></div>}
    </section>
  );
}
