import type { JSX } from 'preact';
import { currentView } from '../store';
import { navigateTo } from '../nav';

const KEY = /\b([A-Z][A-Z0-9]{1,6}-\d+)\b/g;

/** Text where a task key (`PLN-9`) is a button that flies to the task. */
export function RichText(props: { text: string; class?: string }): JSX.Element {
  const parts: (string | { key: string })[] = [];
  let last = 0;
  const text = props.text ?? '';
  for (const m of text.matchAll(KEY)) {
    const at = m.index ?? 0;
    if (at > last) parts.push(text.slice(last, at));
    parts.push({ key: m[1] });
    last = at + m[1].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return (
    <span class={props.class}>
      {parts.map((p, i) => typeof p === 'string' ? p : (
        <button key={i} class="k" data-tag={p.key} type="button" onClick={(e) => { e.stopPropagation(); goToTag(p.key); }}>{p.key}</button>
      ))}
    </span>
  );
}

export function goToTag(tag: string): void {
  const t = currentView.value.tasks.find((x) => x.tag === tag);
  if (t) void navigateTo({ kind: 'planet', id: t.id });
}
