import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { currentView, galaxyId, panels, snap } from '../store';
import { todoAdd, todoDropOn, todoRemove, todoReorder, todoAssign, todoSend, todoToTask, todoUpdate } from '../actions';
import { api } from '../api';
import { t } from '../i18n';
import { Gl } from './Gl';
import { todoHighlight } from './Search';
import { addDialogs } from '../dialogs';
import { getWorld } from '../nav';
import type { TodoItem } from '../types';
import { drawerItems } from '../anim';
import './todo.css';

/** The id of the item being dragged (from the list into the world, or inside the list). */
export const dragTodo = signal<string | null>(null);
const overId = signal<string | null>(null);
const editing = signal<string | null>(null);

export function Todo(): JSX.Element {
  const items = snap.value?.todo ?? [];
  const open = panels.todo.value;
  const listRef = useRef<HTMLUListElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const tasks = currentView.value.tasks.filter((x) => x.status !== 'done');

  useEffect(() => { if (open) drawerItems(listRef.current); }, [open]);
  useEffect(() => {
    const hl = todoHighlight.value;
    if (hl) listRef.current?.querySelector(`[data-todo="${CSS.escape(hl)}"]`)?.scrollIntoView({ block: 'center' });
  }, [todoHighlight.value]);

  const reorder = (from: string, before: string | null): void => {
    const ids = items.map((i) => i.id).filter((id) => id !== from);
    const at = before ? ids.indexOf(before) : ids.length;
    ids.splice(at < 0 ? ids.length : at, 0, from);
    void todoReorder(ids);
  };

  const move = (id: string, delta: number): void => {
    const ids = items.map((i) => i.id);
    const i = ids.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    void todoReorder(ids);
    setTimeout(() => (document.querySelector(`[data-todo="${CSS.escape(id)}"]`) as HTMLElement | null)?.focus(), 60);
  };

  const submit = async (e: Event): Promise<void> => {
    e.preventDefault();
    const el = inputRef.current;
    if (!el || !el.value.trim()) return;
    const r = await todoAdd(el.value.trim());
    if (r) el.value = '';
  };

  return (
    <aside class={`panel todo ${open ? 'open' : ''}`} id="todo" aria-label={t('todo.aria')} aria-hidden={!open}>
      <header><span>{t('todo.title')} <kbd>T</kbd></span><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.todo.value = false; }}><Gl n="close" /></button></header>
      <ul class="tlist" id="todo-list" ref={listRef}>
        {items.length === 0 && <li class="none">{t('todo.empty')}</li>}
        {items.map((it) => (
          <TodoRow
            key={it.id} it={it} tasks={tasks} hl={todoHighlight.value === it.id}
            onMove={move} onDropBefore={(from) => reorder(from, it.id)}
          />
        ))}
      </ul>
      <div class="tnote">{t('todo.note')}</div>
      <form class="tadd" id="todo-new" autocomplete="off" onSubmit={(e) => void submit(e)}>
        <input id="todo-in" ref={inputRef} class="field" maxLength={400} placeholder={t('todo.new')} aria-label={t('todo.new')} />
        <button class="btn" type="submit">+</button>
      </form>
    </aside>
  );
}

function TodoRow(props: {
  it: TodoItem; tasks: { id: string; tag: string }[]; hl: boolean; onMove: (id: string, d: number) => void; onDropBefore: (from: string) => void;
}): JSX.Element {
  const { it } = props;
  const isEditing = editing.value === it.id;
  return (
    <li
      class={`titem ${dragTodo.value === it.id ? 'drag' : ''} ${overId.value === it.id ? 'over' : ''} ${props.hl ? 'hl' : ''}`}
      data-todo={it.id} draggable={!isEditing} tabIndex={0}
      onDragStart={(e) => {
        dragTodo.value = it.id;
        if (e.dataTransfer) { e.dataTransfer.effectAllowed = 'copyMove'; e.dataTransfer.setData('text/plain', it.id); }
      }}
      onDragEnd={() => { dragTodo.value = null; overId.value = null; getWorld()?.setDropTarget(null); }}
      onDragOver={(e) => { if (dragTodo.value && dragTodo.value !== it.id) { e.preventDefault(); overId.value = it.id; } }}
      onDragLeave={() => { if (overId.value === it.id) overId.value = null; }}
      onDrop={(e) => {
        const from = dragTodo.value;
        overId.value = null;
        if (from && from !== it.id) { e.preventDefault(); e.stopPropagation(); props.onDropBefore(from); }
      }}
      onKeyDown={(e) => {
        if (e.altKey && e.key === 'ArrowUp') { e.preventDefault(); props.onMove(it.id, -1); }
        else if (e.altKey && e.key === 'ArrowDown') { e.preventDefault(); props.onMove(it.id, 1); }
        else if (e.key === 'Delete') void todoRemove(it.id);
      }}
    >
      <span class="h" aria-hidden="true">⋮⋮</span>
      <div class="body">
        {isEditing ? (
          <input
            class="field edit" value={it.text} autoFocus
            onKeyDown={(e) => {
              if (e.key === 'Enter') { e.preventDefault(); const v = (e.target as HTMLInputElement).value.trim(); editing.value = null; if (v && v !== it.text) void todoUpdate(it.id, v); }
              else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); editing.value = null; }
            }}
            onBlur={(e) => { const v = (e.target as HTMLInputElement).value.trim(); if (editing.value === it.id) { editing.value = null; if (v && v !== it.text) void todoUpdate(it.id, v); } }}
          />
        ) : (
          <span class="txt" data-todo-text={it.id} title={t('todo.edit')} onDblClick={() => { editing.value = it.id; }}>{it.text}</span>
        )}
        <div class="to">
          <select
            class="field sm" aria-label={t('todo.to')} value={it.to}
            onChange={(e) => void todoAssign(it.id, (e.target as HTMLSelectElement).value)}
          >
            <option value="">{t('todo.toNobody')}</option>
            <option value="quasar">{t('todo.toQuasar')}</option>
            {props.tasks.map((x) => <option value={x.id}>{x.tag}</option>)}
          </select>
          {it.to && <button class="btn sm go" type="button" data-act="todo-send" onClick={() => void todoSend(it.id)}>{t('todo.send')}</button>}
          <button class="btn sm icon" type="button" data-act="todo-del" data-todo-id={it.id} aria-label={t('todo.remove')} title={t('todo.remove')} onClick={() => void todoRemove(it.id)}>×</button>
        </div>
      </div>
    </li>
  );
}

/* ───────── drag into the world ───────── */

const models = signal<{ names: string[]; loading: boolean; note: string; program: string }>({ names: [], loading: false, note: '', program: '' });

export async function loadModels(program: string): Promise<void> {
  models.value = { ...models.value, program, loading: true };
  const poll = async (left: number): Promise<void> => {
    try {
      const r = await api<{ names: string[]; note: string; loading: boolean }>('/api/models?program=' + encodeURIComponent(program));
      if (models.value.program !== program) return;
      models.value = { names: r.names, loading: r.loading, note: r.note, program };
      if (r.loading && left > 0) setTimeout(() => { void poll(left - 1); }, 2500);
    } catch {
      models.value = { names: [], loading: false, note: '', program };
    }
  };
  await poll(36);
}

/** Where a dragged item lands: planet → its agent, Quasar → the Coordinator, universe → a new task. */
export function installTodoDnD(canvas: HTMLElement): void {
  const idOf = (e: DragEvent): string => dragTodo.value ?? e.dataTransfer?.getData('text/plain') ?? '';
  canvas.addEventListener('dragover', (e) => {
    if (!idOf(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    const w = getWorld();
    if (!w) return;
    const rect = canvas.getBoundingClientRect();
    w.setDropTarget(w.entityAt(e.clientX - rect.left, e.clientY - rect.top, ['planet', 'quasar', 'universe']));
  });
  canvas.addEventListener('dragleave', () => getWorld()?.setDropTarget(null));
  canvas.addEventListener('drop', (e) => {
    const id = idOf(e);
    const w = getWorld();
    if (!id || !w) return;
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const target = w.entityAt(e.clientX - rect.left, e.clientY - rect.top, ['planet', 'quasar', 'universe']);
    w.setDropTarget(null);
    dragTodo.value = null;
    if (!target) return;
    void todoDropOn(id, target).then((r) => {
      if (r === 'asked') { addDialogs.popover.value = { id, repo: target.id, galaxy: target.galaxy }; void loadModels('claude'); }
    });
  });
}

export { models as modelList };
void galaxyId; void todoToTask;
