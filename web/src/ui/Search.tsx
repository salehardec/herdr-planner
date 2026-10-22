import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { chat, currentView, searchState, snap } from '../store';
import { api } from '../api';
import { buildDocs, GROUPS, searchDocs, type Doc } from '../search';
import { t } from '../i18n';
import { highlight } from '../util/format';
import { goHome, navigateTo } from '../nav';
import { panels } from '../store';
import { openChat } from '../keys';
import { openChatAt } from '../chat';
import { Gl } from './Gl';
import { openMemoryEntry } from './MemoryPanel';
import type { Entity, SearchHit } from '../types';
import { rows as animRows } from '../anim';
import { useRef } from 'preact/hooks';
import './search.css';

interface Row {
  kind: SearchHit['kind'];
  id: string;
  path: string;
  title: string;
  status: string;
  snippet?: string;
}

const serverHits = signal<Row[]>([]);
export const todoHighlight = signal<string | null>(null);
let timer = 0;
let seq = 0;

const GROUP_ICON: Record<string, string> = {
  galaxy: 'galaxy', quasar: 'quasar', universe: 'universe', planet: 'ship', moon: 'moon', runner: 'yard', todo: 'list', chat: 'chat', memory: 'memory', closing: 'done',
};
const GROUP_LABEL: Record<string, string> = {
  galaxy: 'search.galaxies', quasar: 'search.quasar', universe: 'search.universes', planet: 'search.planets',
  moon: 'search.moons', runner: 'search.runners', todo: 'search.todo', chat: 'search.chat', memory: 'search.memory', closing: 'search.closing',
};

function onType(q: string): void {
  searchState.value = { ...searchState.value, q, open: true, cur: 0 };
  window.clearTimeout(timer);
  const mine = ++seq;
  if (q.trim().length < 2) { serverHits.value = []; return; }
  // The server knows what the page does not: the whole old chat, memory and closing notes.
  timer = window.setTimeout(async () => {
    try {
      const r = await api<{ hits: SearchHit[] }>('/api/search?q=' + encodeURIComponent(q) + '&limit=20');
      if (mine !== seq) return;
      serverHits.value = r.hits.map((h) => ({ kind: h.kind, id: h.id, path: h.path, title: h.title, status: h.status, snippet: h.snippet }));
    } catch { /* the local part stays */ }
  }, 150);
}

function rows(q: string): Row[] {
  const docs: Doc[] = buildDocs(currentView.value, snap.value, chat.value.turns, {
    quasar: t('quasar.label'), todo: t('search.todoPath'), chat: t('search.chatPath'),
  });
  const local = searchDocs(docs, q).map<Row>((f) => ({ ...f.doc }));
  const seen = new Set(local.map((r) => `${r.kind}|${r.id}`));
  const extra = serverHits.value.filter((h) => !seen.has(`${h.kind}|${h.id}`));
  const all = [...local, ...extra];
  const per = new Map<string, number>();
  const out: Row[] = [];
  for (const r of all.sort((a, b) => GROUPS.indexOf(a.kind) - GROUPS.indexOf(b.kind))) {
    const n = per.get(r.kind) ?? 0;
    if (n >= 6) continue;
    per.set(r.kind, n + 1);
    out.push(r);
  }
  return out;
}

export function choose(r: Row): void {
  searchState.value = { open: false, q: '', hits: [], cur: 0 };
  (document.getElementById('q') as HTMLInputElement | null)?.blur();
  switch (r.kind) {
    case 'galaxy': void goHome(); return;
    case 'todo': panels.todo.value = true; todoHighlight.value = r.id; setTimeout(() => { todoHighlight.value = null; }, 2000); return;
    case 'chat': openChat(true); void openChatAt(Number(r.id)); return;
    case 'memory': openMemoryEntry(r.id); return;
    case 'closing': void navigateTo({ kind: 'planet', id: r.id }); return;
    default: {
      const e: Entity = { kind: r.kind as Entity['kind'], id: r.id };
      void navigateTo(e);
    }
  }
}

function pathText(r: Row): string {
  return r.path;
}

export function Search(): JSX.Element {
  const st = searchState.value;
  const list = st.open && st.q.trim() ? rows(st.q) : [];
  const resRef = useRef<HTMLDivElement>(null);
  useEffect(() => { if (st.open && st.q.trim()) animRows(resRef.current, '.srow'); }, [st.open, list.length > 0]);
  const groups = GROUPS.filter((g) => list.some((r) => r.kind === g));
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'ArrowDown') { e.preventDefault(); searchState.value = { ...st, cur: Math.min(list.length - 1, st.cur + 1) }; }
    else if (e.key === 'ArrowUp') { e.preventDefault(); searchState.value = { ...st, cur: Math.max(0, st.cur - 1) }; }
    else if (e.key === 'Enter') { e.preventDefault(); const r = list[st.cur]; if (r) choose(r); }
    else if (e.key === 'Escape') { searchState.value = { ...st, open: false }; (e.target as HTMLElement).blur(); e.stopPropagation(); }
  };
  let flat = -1;
  return (
    <div class="top-search" role="search">
      <Gl n="search" cls="ic" />
      <input
        id="q" class="q" type="search" autocomplete="off" spellcheck={false} value={st.q} aria-label={t('search.aria')}
        aria-controls="sres" aria-expanded={st.open && list.length > 0} placeholder={t('search.placeholder')}
        onInput={(e) => onType((e.target as HTMLInputElement).value)}
        onFocus={() => { searchState.value = { ...searchState.value, open: true }; }}
        onKeyDown={onKey}
      />
      <span class="kk"><kbd>Ctrl</kbd> <kbd>K</kbd></span>
      {st.open && st.q.trim() !== '' && (
        <div id="sres" ref={resRef} role="listbox" class="sres" onMouseDown={(e) => e.preventDefault()}>
          {list.length === 0 && <div class="empty">{t('search.none')}</div>}
          {groups.map((g) => (
            <>
              <h4><Gl n={GROUP_ICON[g]} />{t(GROUP_LABEL[g])}</h4>
              {list.filter((r) => r.kind === g).map((r) => {
                flat = list.indexOf(r);
                return (
                  <div
                    key={`${r.kind}|${r.id}`} role="option" aria-selected={flat === st.cur}
                    class={`srow s-${r.status || 'none'} ${flat === st.cur ? 'cur' : ''}`}
                    data-kind={r.kind} data-id={r.id}
                    onMouseEnter={() => { searchState.value = { ...searchState.value, cur: list.indexOf(r) }; }}
                    onClick={() => choose(r)}
                  >
                    <span class="ic"><Gl n={r.kind === 'planet' && r.status ? r.status : GROUP_ICON[r.kind]} /></span>
                    <span class="body">
                      <span class="ttl" dangerouslySetInnerHTML={{ __html: highlight(r.title, st.q) }} />
                      {r.snippet ? <span class="snip" dangerouslySetInnerHTML={{ __html: r.snippet }} /> : null}
                      <span class="path">{pathText(r)}</span>
                    </span>
                    {r.status && r.kind !== 'chat' ? <span class="st">{statusWord(r)}</span> : null}
                  </div>
                );
              })}
            </>
          ))}
        </div>
      )}
    </div>
  );
}

function statusWord(r: Row): string {
  if (r.kind === 'planet') return t(`status.${r.status}`);
  if (r.kind === 'moon') return t(`moon.${r.status}.short`);
  return '';
}
