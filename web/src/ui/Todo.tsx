import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { currentView, panels, snap } from '../store';
import { todoAdd, todoDropOn, todoRemove, todoReorder, todoAssign, todoSend, todoToTask, todoUpdate } from '../actions';
import { api } from '../api';
import { t } from '../i18n';
import { Gl } from './Gl';
import { todoHighlight } from './Search';
import { openNewTask, openTell } from '../dialogs';
import { getWorld } from '../nav';
import type { TodoItem } from '../types';
import { drawerItems } from '../anim';
import { backspaceTakesFile, hasFiles, makeShelf, pastedFiles } from '../attach';
import { dragTodo, endDrag, overId, startDrag } from '../todo-drag';
import { draftTodos } from '../chat';
import { PendingFiles, SavedFiles } from './Attached';
import { AutoTextarea, fitText } from './AutoText';
import './todo.css';

export { dragTodo };
const editing = signal<string | null>(null);
const fileOver = signal(false);
/** The files of the item being written. */
const shelf = makeShelf();

export function Todo(): JSX.Element {
  const items = snap.value?.todo ?? [];
  const open = panels.todo.value;
  const asideRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const tasks = currentView.value.tasks.filter((x) => x.status !== 'done');

  useEffect(() => { if (open) drawerItems(listRef.current); }, [open]);
  useEffect(() => {
    const hl = todoHighlight.value;
    if (hl) listRef.current?.querySelector(`[data-todo="${CSS.escape(hl)}"]`)?.scrollIntoView({ block: 'center' });
  }, [todoHighlight.value]);

  // The room the drawer takes at the top: on narrow windows the navigator and the focus card move below it.
  useEffect(() => {
    const el = asideRef.current;
    const win = document.getElementById('win');
    if (!el || !win || typeof ResizeObserver === 'undefined') return;
    const set = (): void => { win.style.setProperty('--todo-h', open ? `${Math.round(el.getBoundingClientRect().height)}px` : '0px'); };
    set();
    const ro = new ResizeObserver(set);
    ro.observe(el);
    return () => { ro.disconnect(); win.style.setProperty('--todo-h', '0px'); };
  }, [open]);

  // A screenshot pasted while the drawer is open lands in the item being written (not when a field elsewhere took it).
  useEffect(() => {
    if (!open) return;
    const onPaste = (e: ClipboardEvent): void => {
      if (e.defaultPrevented) return;
      const files = pastedFiles(e);
      if (!files.length) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest?.('#chat, .dlg') || (target?.matches?.('textarea,input') && !target.closest('#todo'))) return;
      e.preventDefault();
      shelf.add(files);
    };
    document.addEventListener('paste', onPaste);
    return () => document.removeEventListener('paste', onPaste);
  }, [open]);

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

  const submit = async (): Promise<void> => {
    const el = inputRef.current;
    if (!el) return;
    // The server trims blank edges; leading indentation of the lines is what a person typed.
    let text = el.value;
    if (!text.trim() && !shelf.files.value.length) return;
    const got = await shelf.settled();
    if (!got.ok) { return; }
    // Only a file: the item is named after it.
    if (!text) text = t('todo.fileOnly', { name: got.saved[0].name });
    const r = await todoAdd(text, got.saved);
    if (r) { el.value = ''; fitText(el); shelf.clear(); }
  };

  return (
    <aside
      ref={asideRef}
      class={`panel todo ${open ? 'open' : ''} ${fileOver.value ? 'filedrop' : ''}`} id="todo" aria-label={t('todo.aria')} aria-hidden={!open}
      onDragOver={(e) => { if (hasFiles(e.dataTransfer)) { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'; fileOver.value = true; } }}
      onDragLeave={(e) => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) fileOver.value = false; }}
      onDrop={(e) => { fileOver.value = false; if (e.dataTransfer?.files?.length) { e.preventDefault(); shelf.add(e.dataTransfer.files); } }}
    >
      <header><span>{t('todo.title')} <kbd>T</kbd></span><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.todo.value = false; }}><Gl n="close" /></button></header>
      <form class="tadd" id="todo-new" autocomplete="off" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <AutoTextarea
          id="todo-in" elRef={inputRef} class="field"
          placeholder={t('todo.new')} aria-label={t('todo.new')} onSend={() => void submit()}
          onKeyDown={(e) => { backspaceTakesFile(e, e.currentTarget, shelf); }}
        />
        <label class="btn browse" id="todo-browse" title={t('todo.browseTip')}>
          <Gl n="attach" />{t('todo.browse')}
          <input
            type="file" id="todo-file" multiple hidden
            accept="image/png,image/jpeg,image/webp,image/gif,audio/*,text/*,.md,.log,.json,.yaml,.yml,.toml,.csv"
            onChange={(e) => { shelf.add((e.target as HTMLInputElement).files); (e.target as HTMLInputElement).value = ''; }}
          />
        </label>
        <button class="btn" type="submit" aria-label={t('todo.add')} title={t('todo.add')}>+</button>
        <div class="keyhint" id="todo-hint">{t('text.keys.add')}</div>
      </form>
      <PendingFiles files={shelf.files.value} onRemove={shelf.remove} id="todo-attach" />
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
    </aside>
  );
}

/** The words of an item: three lines, then «ещё»; a double click opens them for correction. */
function TodoText(props: { id: string; text: string }): JSX.Element {
  const [open, setOpen] = useState(false);
  const [long, setLong] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    if (open) return;
    const el = ref.current;
    if (!el) return;
    const measure = (): void => { setLong(el.scrollHeight > el.clientHeight + 1); };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [props.text, open]);
  return (
    <>
      <span
        ref={ref} class={`txt ${open ? 'open' : ''}`} data-todo-text={props.id} title={t('todo.edit')}
        onDblClick={() => { editing.value = props.id; }}
      >{props.text}</span>
      {long && (
        <button class="btn sm more" type="button" aria-expanded={open} onClick={(e) => { e.stopPropagation(); setOpen(!open); }}>
          {open ? t('todo.less') : t('todo.more')}
        </button>
      )}
    </>
  );
}

/** The words being corrected: its own state, so a board refresh cannot replace what is being typed. */
function TodoEdit(props: { text: string; onCommit: (text: string) => Promise<void>; onCancel: () => void }): JSX.Element {
  const [draft, setDraft] = useState(props.text);
  const ref = useRef<HTMLTextAreaElement>(null);
  const tried = useRef<string | null>(null);
  // Enter and then leaving the field are one attempt: a refused text is not sent twice.
  const commit = (): void => {
    const text = ref.current?.value ?? draft;
    if (tried.current === text) return;
    tried.current = text;
    void props.onCommit(text);
  };
  return (
    <>
      <AutoTextarea
        class="field edit" elRef={ref} value={draft} autoFocus
        onInput={(e) => { tried.current = null; setDraft((e.target as HTMLTextAreaElement).value); }}
        onSend={commit}
        onKeyDown={(e) => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); props.onCancel(); } }}
        onBlur={commit}
      />
      <span class="hint edit-hint">{t('text.keys.save')}</span>
    </>
  );
}

function TodoRow(props: {
  it: TodoItem; tasks: { id: string; tag: string }[]; hl: boolean; onMove: (id: string, d: number) => void; onDropBefore: (from: string) => void;
}): JSX.Element {
  const { it } = props;
  const isEditing = editing.value === it.id;
  const files = it.attachments ?? [];
  /** Enter (or leaving the field) keeps the words; Shift+Enter is a new line. A refused save stays open. */
  const commitEdit = async (text: string): Promise<void> => {
    if (editing.value !== it.id) return;
    const v = text;
    if (!v.trim() || v === it.text || (await todoUpdate(it.id, { text: v }))) editing.value = null;
  };
  return (
    <li
      class={`titem ${draftTodos.value.includes(it.id) ? 'drafted' : ''} ${dragTodo.value === it.id ? 'drag' : ''} ${overId.value === it.id ? 'over' : ''} ${props.hl ? 'hl' : ''}`}
      data-todo={it.id} draggable={!isEditing} tabIndex={0}
      onDragStart={(e) => {
        startDrag(it.id);
        if (e.dataTransfer) { e.dataTransfer.effectAllowed = 'copyMove'; e.dataTransfer.setData('text/plain', it.id); }
      }}
      onDragEnd={() => { endDrag(); getWorld()?.setDropTarget(null); }}
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
          <TodoEdit key={it.id} text={it.text} onCommit={commitEdit} onCancel={() => { editing.value = null; }} />
        ) : (
          <TodoText id={it.id} text={it.text} />
        )}
        {draftTodos.value.includes(it.id) && <span class="dnote">{t('todo.inDraft')}</span>}
        <SavedFiles list={files} class="tfiles" onRemove={(id) => void todoUpdate(it.id, { attachments: files.filter((a) => a.id !== id) })} />
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

/** Where a dragged item lands: planet → its agent (after a window to correct it), Quasar → the Coordinator, universe → a new task. */
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
    endDrag();
    if (!target) return;
    void todoDropOn(id, target).then((r) => {
      if (r === 'asked') { openNewTask(id, target.id); void loadModels('claude'); }
      else if (r === 'tell') openTell(id, target.id);
    });
  });
}

export { models as modelList };
void todoToTask;
