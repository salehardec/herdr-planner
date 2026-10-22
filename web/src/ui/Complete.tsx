/* The list of `/` and `@`: one field of the page at a time owns it, the list sits above that field.
   The keys are read before the field's own: Enter picks a row while the list is open, Esc closes it,
   Shift+Enter is still a new line. */
import { signal, type Signal } from '@preact/signals';
import { Fragment, type JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import {
  applyItem, detect, groupsOf, mentionData, mentionItems, slashItems,
  type FileHit, type Group, type Item, type SlashCmd, type Trigger,
} from '../complete';
import { commandsFor, filesFor, forgetCommands } from '../hints';
import { t } from '../i18n';
import { snap } from '../store';
import { Gl } from './Gl';
import './complete.css';

/** What the list shows now: where the symbol is, the rows and the one under the keyboard. */
export interface View {
  trigger: Trigger;
  items: Item[];
  cur: number;
}

export interface Completer {
  /** Read by the field's own render: `null` when the list is closed. */
  view: Signal<View | null>;
  /** After every change of the field's text or caret. */
  update: (el: HTMLTextAreaElement) => void;
  /** Before the field's own keys; `true` when the list took the key. */
  key: (e: KeyboardEvent, el: HTMLTextAreaElement) => boolean;
  /** A click on a row. */
  pick: (i: number, el: HTMLTextAreaElement) => void;
  /** The row under the mouse. */
  hover: (i: number) => void;
  close: () => void;
}

const GROUP_ICON: Record<Group, string> = {
  command: 'send', task: 'ship', universe: 'universe', galaxy: 'galaxy', runner: 'yard', file: 'file', image: 'attach',
};
const GROUP_LABEL: Record<Group, string> = {
  command: 'hint.group.command', task: 'hint.group.task', universe: 'hint.group.universe', galaxy: 'hint.group.galaxy',
  runner: 'hint.group.runner', file: 'hint.group.file', image: 'hint.group.image',
};

/**
 * The `/` and `@` list of one field. `program` is the program that will read the words (the Coordinator's,
 * or the task's agent), `dir` the folder it works in (the Coordinator's own is empty: the server knows it).
 */
export function useComplete(props: { program: string; dir: string }): Completer {
  // One signal for the life of the field: a new one per render would be written by the handlers and
  // read, empty, by the render that its own write causes.
  const view = useRef(signal<View | null>(null)).current;
  const loaded = useRef<{ key: string; list: SlashCmd[] } | null>(null);
  const loading = useRef(false);
  const failed = useRef<string | null>(null);
  const field = useRef<HTMLTextAreaElement | null>(null);
  const files = useRef<{ q: string; list: FileHit[] }>({ q: '', list: [] });
  const timer = useRef(0);
  const seq = useRef(0);
  const key = `${props.program}|${props.dir}`;

  const close = (): void => { view.value = null; };

  const build = (trigger: Trigger): View => {
    let items: Item[];
    if (trigger.char === '/') {
      const list = loaded.current?.key === key ? loaded.current.list : [];
      items = slashItems(list, trigger.query);
    } else {
      const q = trigger.query.trim().toLowerCase();
      const found = files.current.q === q ? files.current.list : [];
      items = mentionItems(trigger.query, mentionData(snap.value, found));
    }
    const prev = view.peek();
    const cur = prev && prev.trigger.char === trigger.char ? Math.min(prev.cur, Math.max(0, items.length - 1)) : 0;
    return { trigger, items, cur };
  };

  /** Reads the commands of the program; a list that was asked for while they were on the way is shown then. */
  const load = (): void => {
    const program = props.program;
    if (!program || loading.current || loaded.current?.key === key || failed.current === key) return;
    loading.current = true;
    const mine = ++seq.current;
    void commandsFor(program, props.dir)
      .then((list) => {
        if (mine !== seq.current) return;
        loaded.current = { key, list };
      })
      .catch(() => { failed.current = key; })
      .finally(() => {
        loading.current = false;
        // The person may have typed the slash before the answer came: the list appears now.
        const el = field.current;
        const v = view.peek();
        const trigger = el && !v ? detect(el.value, el.selectionStart ?? 0) : null;
        if (trigger?.char === '/') {
          const again = build(trigger);
          view.value = again.items.length ? again : null;
        }
      });
  };

  // A change of the Coordinator (or of the task) is a change of the program: what was read is read again,
  // so the list is the program that is running now, never the one before it.
  useEffect(() => {
    loaded.current = null;
    failed.current = null;
    view.value = null;
    if (!props.program) return;
    forgetCommands(props.program);
    load();
  }, [props.program, props.dir]);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const update = (el: HTMLTextAreaElement): void => {
    field.current = el;
    const trigger = detect(el.value, el.selectionStart ?? 0);
    if (!trigger) { close(); return; }
    if (trigger.char === '/') {
      if (!props.program) { close(); return; }
      if (loaded.current?.key !== key) { load(); close(); return; }
      const next = build(trigger);
      view.value = next.items.length ? next : null;
      return;
    }
    const q = trigger.query.trim().toLowerCase();
    const local = build(trigger);
    view.value = local.items.length ? local : null;
    if (!q || files.current.q === q) return;
    // The files live on the server: they come after the typing has paused, and only the last question counts.
    window.clearTimeout(timer.current);
    const mine = ++seq.current;
    timer.current = window.setTimeout(() => {
      void filesFor(trigger.query)
        .then((list) => {
          if (mine !== seq.current) return;
          files.current = { q, list };
          // The files may be the only rows (a name that is not a task or a universe): build from the field as it is now.
          const el = field.current;
          const now = el ? detect(el.value, el.selectionStart ?? 0) : null;
          if (now?.char !== '@') return;
          const again = build(now);
          view.value = again.items.length ? again : null;
        })
        .catch(() => { /* the list of the page stays */ });
    }, 120);
  };

  const pick = (i: number, el: HTMLTextAreaElement): void => {
    const v = view.peek();
    const item = v?.items[i];
    if (!v || !item) return;
    const next = applyItem(el.value, v.trigger, item);
    close();
    el.value = next.text;
    el.setSelectionRange(next.caret, next.caret);
    // The field's own handler (its draft, its height) sees the text as if it were typed.
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.focus();
  };

  const onKey = (e: KeyboardEvent, el: HTMLTextAreaElement): boolean => {
    const v = view.peek();
    if (!v) return false;
    if (e.key === 'Escape') {
      // The page's own Esc (a step back, closing the chat) is not for a list that is closing.
      e.preventDefault();
      e.stopPropagation();
      close();
      return true;
    }
    if (!v.items.length) return false;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      view.value = { ...v, cur: (v.cur + 1) % v.items.length };
      return true;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      view.value = { ...v, cur: (v.cur - 1 + v.items.length) % v.items.length };
      return true;
    }
    if ((e.key === 'Enter' && !e.shiftKey) || e.key === 'Tab') {
      e.preventDefault();
      pick(v.cur, el);
      return true;
    }
    return false;
  };

  const hover = (i: number): void => {
    const v = view.peek();
    if (v && v.cur !== i) view.value = { ...v, cur: i };
  };

  return { view, update, key: onKey, pick, hover, close };
}

/** The rows of the open list, grouped under their headings. */
export function Completion(props: {
  view: View | null; id: string; onPick: (i: number) => void; onHover: (i: number) => void;
}): JSX.Element | null {
  const v = props.view;
  const ref = useRef<HTMLDivElement>(null);
  const cur = v?.cur ?? 0;
  useEffect(() => {
    const el = ref.current?.querySelector<HTMLElement>('.ci.cur');
    // The keyboard may walk past the bottom of a long list: the row under it stays in view.
    el?.scrollIntoView({ block: 'nearest' });
  }, [cur, v?.items.length]);
  if (!v || !v.items.length) return null;
  const groups = groupsOf(v.items);
  return (
    <div class="cmpl" id={props.id} ref={ref} role="listbox" aria-label={t('hint.aria')} onMouseDown={(e) => e.preventDefault()}>
      {groups.map((g) => (
        <Fragment key={g}>
          <div class="cg" role="presentation"><Gl n={GROUP_ICON[g]} />{t(GROUP_LABEL[g])}</div>
          {v.items.map((item, i) => (item.group === g ? (
            <div
              key={`${item.group}|${item.insert}`} class={`ci ${i === v.cur ? 'cur' : ''}`} role="option"
              aria-selected={i === v.cur} data-i={i}
              onMouseEnter={() => props.onHover(i)} onClick={() => props.onPick(i)}
            >
              <span class="ic"><Gl n={item.icon} /></span>
              <span class="lb">{item.label}</span>
              {item.hint && <span class="hn">{item.hint.startsWith('cmd.') ? t(item.hint) : item.hint}</span>}
            </div>
          ) : null))}
        </Fragment>
      ))}
      <div class="cf" role="presentation">{t('hint.keys')}</div>
    </div>
  );
}
