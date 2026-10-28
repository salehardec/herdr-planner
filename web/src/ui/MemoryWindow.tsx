/* The memory in a centred window over the Galaxy: the list of one level (the Galaxy's, a universe's, or the memory another
   program keeps — read only) with a search and filters by kind and state, entries that open to their full text, and the
   changes (edit, delete, «В память», send to the repository by a task) that show «было → стало» first. Esc (see keys.ts
   through dialogs.ts), «Свернуть» and a click on the background close it. */
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { currentView, snap } from '../store';
import { navigateTo } from '../nav';
import { t } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';
import {
  deleteEntry, discardDraft, KINDS, loadMemory, memoryOf, memoryWin, previewDelete, previewDiscard, stateOf,
  type EntryState, type MemEntry, type MemoryWin, type MemPreview,
} from './memory-api';
import { ChangeCard } from './MemoryChange';
import { failWords, Form, Git, matches, Row, type Draft } from './MemoryPanel';
import { marked } from './memory-lint';
import './memory.css';

/** The entries of the level the window shows (a program's own memory is read only). */
const entriesOf = (w: MemoryWin): MemEntry[] => {
  const d = memoryOf(w.program ? '' : w.repo).data;
  return w.program ? d?.foreign?.find((f) => f.program === w.program)?.entries ?? [] : d?.entries ?? [];
};

function Body(props: { w: MemoryWin; shown: MemEntry[]; total: number }): JSX.Element {
  const { w, shown, total } = props;
  const repo = w.program ? '' : w.repo;
  const st = memoryOf(repo);
  const data = st.data;
  const [openIds, setOpen] = useState<Record<string, boolean>>(w.open ? { [w.open.replace(/^@[^|]*\|/, '')]: true } : {});
  const [hotId, setHot] = useState(w.open ? w.open.replace(/^@[^|]*\|/, '') : '');
  const [editing, setEditing] = useState<Draft | null>(null);
  const [delCard, setDelCard] = useState<{ p: MemPreview; id: string; discard?: boolean } | null>(null);
  const [delBusy, setDelBusy] = useState(false);
  const [delErr, setDelErr] = useState('');
  const delRef = useRef<HTMLDivElement>(null);
  const readOnly = !!w.program;
  const editable = !readOnly && !!data?.writable;
  const defaultKind = repo ? 'decision' : 'rule';

  // The entry the search found is marked for a moment.
  useEffect(() => {
    if (!hotId) return undefined;
    const id = window.setTimeout(() => setHot(''), 2500);
    return () => window.clearTimeout(id);
  }, [hotId]);
  useEffect(() => { if (delCard) delRef.current?.scrollIntoView({ block: 'nearest' }); }, [delCard]);

  const goTask = (id: string): void => {
    memoryWin.value = null;
    void navigateTo({ kind: 'planet', id });
  };

  return (
    <div class="mmw-body" id="mem-window-body">
      {!readOnly && <div class="hint">{repo ? t('mem.hintUniverse') : t('mem.hintGalaxy')}</div>}
      {readOnly && <div class="hint">{t('mem.foreignNote')}</div>}
      {data && !readOnly && <div class="hint mm-dir"><code title={data.dir}>{data.dir}</code></div>}
      {data?.note && !readOnly && <div class="hint">{data.note}</div>}
      {st.error && <div class="hint bad" id="mem-load-error">{st.error}</div>}
      {repo && data && <Git repo={repo} data={data} />}

      {delCard && (
        <div ref={delRef}>
          <ChangeCard
            p={delCard.p} busy={delBusy} error={delErr} confirmLabel={delCard.discard ? t('mem.discard') : t('mem.delete')} onCancel={() => setDelCard(null)}
            onConfirm={(a) => {
              setDelBusy(true);
              const done = delCard.discard
                ? discardDraft(repo, delCard.id, delCard.p.token).then(() => toast(t('mem.discarded')))
                : deleteEntry(repo, delCard.id, delCard.p.token, a).then(() => toast(t(delCard.p.draft ? 'mem.deletedDraft' : 'mem.deleted')));
              done
                .then(() => setDelCard(null))
                .catch((er) => setDelErr(failWords(er)))
                .finally(() => setDelBusy(false));
            }}
          />
        </div>
      )}
      {editing && editing.id === '' && <Form repo={repo} draft={editing} onDone={() => setEditing(null)} />}
      {editable && !editing && (
        <div class="row">
          <button class="btn sm" type="button" id="mem-add" onClick={() => setEditing({ id: '', title: '', kind: defaultKind, description: '', body: '' })}><Gl n="plus" />{t('mem.add')}</button>
        </div>
      )}

      <div class="mm-list" id="mem-list" data-mem-level={readOnly ? 'foreign' : repo ? 'universe' : 'galaxy'}>
        {data && total === 0 && !editing && <div class="hint mm-empty">{repo ? t('mem.emptyUniverse') : t('mem.emptyGalaxy')}</div>}
        {shown.map((e) => (
          editing && editing.id === e.id
            ? <Form key={e.id} repo={repo} draft={editing} onDone={() => setEditing(null)} />
            : (
              <Row
                key={e.id} e={e} open={!!openIds[e.id]} editable={editable} hot={hotId === e.id} onTask={goTask}
                onToggle={() => setOpen({ ...openIds, [e.id]: !openIds[e.id] })}
                onEdit={() => setEditing({ id: e.id, title: e.title, kind: e.kind || defaultKind, description: e.description === e.title ? '' : e.description, body: e.body, review: e.review ?? '' })}
                // deleting shows «было → стало» first, like any change
                onDelete={() => {
                  setDelErr('');
                  previewDelete(repo, e.id).then((p) => setDelCard({ p, id: e.id })).catch((er) => toast(failWords(er), { icon: 'failed' }));
                }}
                // throwing a draft away shows «было → стало» too (the draft → what git has)
                onDiscard={() => {
                  setDelErr('');
                  previewDiscard(repo, e.id).then((p) => setDelCard({ p, id: e.id, discard: true })).catch((er) => toast(failWords(er), { icon: 'failed' }));
                }}
              />
            )
        ))}
        {total > 0 && shown.length === 0 && <div class="hint mm-empty" id="mem-nomatch">{t('mem.noMatch')}</div>}
      </div>
    </div>
  );
}

function Win(props: { w: MemoryWin }): JSX.Element {
  const { w } = props;
  const view = currentView.value;
  const [q, setQ] = useState(w.q ?? '');
  const [kind, setKind] = useState('');
  const [state, setState] = useState<EntryState | ''>('');
  const [review, setReview] = useState(false);
  const [lastRepo, setLastRepo] = useState(w.repo || view.projects[0]?.name || '');
  const searchRef = useRef<HTMLInputElement>(null);
  const level = w.program ? 'foreign' : w.repo ? 'universe' : 'galaxy';
  const lv = w.program ? '' : w.repo;

  // Both the Galaxy's data (its tabs) and the level's are fresh when the window opens; the files may change under it.
  useEffect(() => { void loadMemory(''); searchRef.current?.focus(); }, []);
  useEffect(() => { void loadMemory(lv); }, [lv]);
  useEffect(() => {
    const id = window.setInterval(() => { if (!document.hidden) void loadMemory(lv); }, 8000);
    return () => window.clearInterval(id);
  }, [lv]);
  useEffect(() => { if (w.repo) setLastRepo(w.repo); }, [w.repo]);

  const close = (): void => { memoryWin.value = null; };
  const go = (next: Partial<MemoryWin>): void => { setState(''); setReview(false); memoryWin.value = { repo: '', ...next }; };
  const foreign = (memoryOf('').data?.foreign ?? []).filter((f) => f.entries.length > 0 || f.program === w.program);
  const all = entriesOf(w);
  const shown = all.filter((e) => matches(e, q) && (!kind || e.kind === kind) && (!state || stateOf(e) === state) && (!review || (e.lint?.length ?? 0) > 0));
  const filtered = !!q.trim() || !!kind || !!state || review;
  const toReview = marked(all).length;
  const title = level === 'foreign' ? t('mem.foreign', { p: foreign.find((f) => f.program === w.program)?.label ?? w.program ?? '' })
    : level === 'universe' ? t('mem.titleUniverse', { repo: w.repo }) : t('mem.titleGalaxy', { g: snap.value?.host || '' });

  return (
    <div class="scrim memory-scrim" id="mem-window-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel memory-win" id="mem-window" role="dialog" aria-modal="true" aria-label={t('mem.win')} data-level={level}>
        <header>
          <span class="sh-title">
            <Gl n="memory" />
            <b>{title}</b>
            <span class="cnt" id="mem-window-count">{all.length}</span>
          </span>
          <span class="sh-buttons">
            <button class="btn" type="button" data-act="memory-close" onClick={close}><Gl n="shrink" />{t('focus.collapse')}</button>
            <button class="btn icon sm" type="button" data-act="memory-close" aria-label={t('focus.collapse')} onClick={close}><Gl n="close" /></button>
          </span>
        </header>
        <div class="mmw-bar">
          <div class="mm-level" role="group" aria-label={t('mem.winLevel')}>
            <button type="button" class={`btn sm ${level === 'galaxy' ? 'on' : ''}`} id="mem-lv-galaxy" onClick={() => go({})}>{t('mem.winGalaxy')}</button>
            <button type="button" class={`btn sm ${level === 'universe' ? 'on' : ''}`} id="mem-lv-universe" disabled={view.projects.length === 0 && !w.repo} onClick={() => go({ repo: lastRepo })}>{t('mem.winUniverse')}</button>
            {level === 'universe' && (
              <select class="field" id="mem-lv-repo" value={w.repo} aria-label={t('mem.fRepo')} onChange={(e) => go({ repo: (e.target as HTMLSelectElement).value })}>
                {!view.projects.some((p) => p.name === w.repo) && <option value={w.repo}>{w.repo}</option>}
                {view.projects.map((p) => <option value={p.name}>{p.name}</option>)}
              </select>
            )}
            {foreign.map((f) => (
              <button type="button" class={`btn sm ${w.program === f.program ? 'on' : ''}`} key={f.program} id={`mem-lv-${f.program}`} data-program={f.program} onClick={() => go({ program: f.program })}>{f.label}</button>
            ))}
          </div>
          <div class="mmw-filters">
            <input ref={searchRef} class="field" id="mem-search" type="search" value={q} placeholder={t('mem.filter')} aria-label={t('mem.filter')} onInput={(e) => setQ((e.target as HTMLInputElement).value)} />
            <select class="field" id="mem-f-kind" value={kind} aria-label={t('mem.fKindLabel')} onChange={(e) => setKind((e.target as HTMLSelectElement).value)}>
              <option value="">{t('mem.fKindAny')}</option>
              {KINDS.map((k) => <option value={k}>{t(`mem.kind.${k}`)}</option>)}
            </select>
            {level === 'universe' && (
              <select class="field" id="mem-f-state" value={state} aria-label={t('mem.fStateLabel')} onChange={(e) => setState((e.target as HTMLSelectElement).value as EntryState | '')}>
                <option value="">{t('mem.fStateAny')}</option>
                {(['saved', 'draft', 'waiting'] as const).map((s) => <option value={s}>{t(`mem.state.${s}`)}</option>)}
              </select>
            )}
            {level !== 'foreign' && (toReview > 0 || review) && (
              <select class="field" id="mem-f-lint" value={review ? 'lint' : ''} aria-label={t('mem.fLintLabel')} onChange={(e) => setReview((e.target as HTMLSelectElement).value === 'lint')}>
                <option value="">{t('mem.fLintAny')}</option>
                <option value="lint">{t('mem.fLintOnly', { n: toReview })}</option>
              </select>
            )}
            {filtered && <button class="btn sm" type="button" id="mem-f-clear" onClick={() => { setQ(''); setKind(''); setState(''); setReview(false); }}>{t('mem.filterClear')}</button>}
            <span class="hint mmw-count" id="mem-shown">{t('mem.shown', { a: shown.length, b: all.length })}</span>
          </div>
        </div>
        <div class="mmw-scroll">
          <Body key={`${w.program ?? ''}|${w.repo}`} w={w} shown={shown} total={all.length} />
        </div>
      </section>
    </div>
  );
}

export function MemoryWindow(): JSX.Element | null {
  const w = memoryWin.value;
  return w ? <Win w={w} /> : null;
}
