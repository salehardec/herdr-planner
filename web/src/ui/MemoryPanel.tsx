import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { armed, currentView, snap } from '../store';
import { confirmStep } from '../actions';
import { send } from '../chat';
import { afterAnswered, navigateTo } from '../nav';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { dayTimeOf } from '../util/dates';
import { Gl } from './Gl';
import { AutoTextarea, fitText } from './AutoText';
import { BatchBar } from './MemoryBatch';
import { cardPick, linesOf, setLines, toggleCard, toggleLine, memPick, pickKey } from './memory-batch';
import {
  deleteEntry, discardDraft, dismissSuggestion, keepSuggestion, KINDS, kindLabel, loadMemory, memoryOf, openMemory, previewSend, sendMemory, stateOf,
  previewDelete, previewDiscard, previewSave, previewSuggestion, rememberDlg, saveEntry, connectMemory, setSuggest,
  type Accept, type MemData, type MemEntry, type MemPreview, type MemSuggestion, type SaveBody, type SentTask, forgetSent,
} from './memory-api';
import { ChangeCard } from './MemoryChange';
import { parseDocId } from './memory-ids';
import './memory.css';

export const failWords = (e: unknown): string => (e as Error)?.message || t('mem.failed');

/** One entry: its line, and (opened) its text with Edit and Delete. The state badge sits beside the title, never inside
 *  the title's column: on a narrow row it drops to its own line instead of squeezing the title. */
export function Row(props: {
  e: MemEntry; open: boolean; editable: boolean; hot: boolean; onToggle: () => void; onEdit: () => void; onDelete: () => void; onDiscard?: () => void;
  /** Goes to the board task a «waits for the merge» entry is in (the badge is a link then). */
  onTask?: (id: string) => void;
}): JSX.Element {
  const { e } = props;
  const gone = e.draft === 'deleted';
  const frozen = gone || !!e.draft?.startsWith('sent');   // a board task has it: change it after the merge
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { if (props.hot) ref.current?.scrollIntoView({ block: 'center' }); }, [props.hot]);
  const badge = e.draft ? t(`mem.draft.${e.draft}`, { task: e.task ?? '' }) : '';
  const task = e.draft?.startsWith('sent') && e.task && props.onTask ? currentView.value.tasks.find((x) => x.tag === e.task) : undefined;
  return (
    <div ref={ref} class={`mm-row ${props.open ? 'open' : ''} ${props.hot ? 'hot' : ''} ${e.draft ? `draft d-${e.draft}` : ''}`} data-mem={e.id} data-draft={e.draft || undefined}>
      <div class="mm-head">
        <button class="mm-line" type="button" aria-expanded={props.open} onClick={props.onToggle}>
          <Gl n={props.open ? 'chev' : 'chev-r'} cls="mm-chev" />
          <span class={`mm-kind k-${e.kind || 'note'}`}>{kindLabel(e.kind)}</span>
          <span class="mm-text">
            <b class="mm-title" title={e.title}>{e.title}</b>
            {!props.open && e.description && e.description !== e.title && <span class="mm-desc">{e.description}</span>}
          </span>
        </button>
        {e.draft && (task
          ? <button type="button" class={`mm-draft mm-task d-${e.draft}`} data-act="mem-task" data-task={task.id} title={t('mem.taskTip', { task: task.tag })} onClick={() => props.onTask?.(task.id)}>{badge}</button>
          : <span class={`mm-draft d-${e.draft}`} title={t('mem.draftTip')}>{badge}</span>)}
      </div>
      {props.open && (
        <div class="mm-more">
          {e.description && e.description !== e.title && <div class="mm-lead">{e.description}</div>}
          <div class="mm-body">{e.body}</div>
          <div class="mm-meta">
            <code>{e.id}.md</code>
            {e.added && <span>{t('mem.added', { d: e.added })}</span>}
            {e.source && <span>{t('mem.from', { s: e.source })}</span>}
            {e.at > 0 && <span>{t('mem.changed', { d: dayTimeOf(e.at) })}</span>}
          </div>
          {props.editable && (
            <div class="row">
              {!frozen && <button class="btn sm" type="button" data-act="mem-edit" onClick={props.onEdit}>{t('mem.edit')}</button>}
              {!frozen && <button class="btn sm danger" type="button" data-act="mem-del" onClick={props.onDelete}>{t('mem.delete')}</button>}
              {e.draft && !e.draft.startsWith('sent') && props.onDiscard && <button class="btn sm" type="button" data-act="mem-discard" onClick={props.onDiscard}>{t('mem.discard')}</button>}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export interface Draft { id: string; title: string; kind: string; description: string; body: string }

export function Form(props: { repo: string; draft: Draft; onDone: () => void }): JSX.Element {
  const [d, setD] = useState(props.draft);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState('');
  const [card, setCard] = useState<{ p: MemPreview; body: SaveBody } | null>(null);
  const set = (patch: Partial<Draft>): void => setD({ ...d, ...patch });
  const ok = d.body.trim().length > 0 || d.title.trim().length > 0;
  const bodyOf = (x: Draft, repo: string, id: string): SaveBody => ({ repo, id, title: x.title, kind: x.kind, description: x.description, body: x.body });
  // Saving shows «было → стало» first; the entry is written only when the card is confirmed.
  const ask = async (x: Draft, repo: string = props.repo, id: string = x.id): Promise<void> => {
    setBusy(true);
    setErr('');
    try {
      const body = bodyOf(x, repo, id);
      setCard({ p: await previewSave(body), body });
    } catch (e) { setErr(failWords(e)); }
    setBusy(false);
  };
  const confirm = async (a: Accept): Promise<void> => {
    if (!card) return;
    setBusy(true);
    setErr('');
    try {
      await saveEntry(card.body, card.p.token, a);
      toast(t(card.p.draft ? 'mem.savedDraft' : 'mem.saved'));
      props.onDone();
    } catch (e) { setErr(failWords(e)); setBusy(false); }
  };
  // The same text without what was found: the form takes it and asks for a new preview.
  const redact = (): void => {
    const r = card?.p.redacted[0];
    if (!r) return;
    const x = { ...d, title: r.title, description: r.description, body: r.body };
    setD(x);
    void ask(x);
  };
  return (
    <div class="mm-form" id="mem-form">
      <label class="fl2">{t('mem.fTitle')}
        <input class="field" id="mem-title" value={d.title} maxLength={80} placeholder={t('mem.fTitlePh')} disabled={!!card} onInput={(e) => set({ title: (e.target as HTMLInputElement).value })} />
      </label>
      <label class="fl2">{t('mem.fKind')}
        <select class="field" id="mem-kind" value={d.kind} disabled={!!card} onChange={(e) => set({ kind: (e.target as HTMLSelectElement).value })}>
          {KINDS.map((k) => <option value={k}>{t(`mem.kind.${k}`)}</option>)}
        </select>
      </label>
      <label class="fl2">{t('mem.fDesc')}
        <input class="field" id="mem-desc" value={d.description} maxLength={200} placeholder={t('mem.fDescPh')} disabled={!!card} onInput={(e) => set({ description: (e.target as HTMLInputElement).value })} />
      </label>
      <label class="fl2">{t('mem.fBody')}
        <textarea class="field" id="mem-body" rows={5} value={d.body} disabled={!!card} onInput={(e) => set({ body: (e.target as HTMLTextAreaElement).value })} />
      </label>
      {err && !card && <div class="hint bad" id="mem-err">{err}</div>}
      {card ? (
        <ChangeCard
          p={card.p} busy={busy} error={err} onConfirm={(a) => void confirm(a)} onCancel={() => { setCard(null); setErr(''); }}
          onRedact={redact} onToGalaxy={props.repo ? () => void ask(d, '', '') : undefined}
        />
      ) : (
        <div class="row">
          <button class="btn primary" type="button" id="mem-save" disabled={!ok || busy} onClick={() => void ask(d)}>{t('mem.save')}</button>
          <button class="btn" type="button" id="mem-cancel" onClick={props.onDone}>{t('cancel')}</button>
        </div>
      )}
    </div>
  );
}

/** One proposed line: the field is as tall as its text (and follows the width of its panel), so nothing hides under a scroll. */
function LineField(props: { value: string; disabled: boolean; onInput: (v: string) => void }): JSX.Element {
  const el = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => {
    const node = el.current;
    if (!node || typeof ResizeObserver === 'undefined') return undefined;
    let w = node.clientWidth;
    const ro = new ResizeObserver(() => { if (node.clientWidth !== w) { w = node.clientWidth; fitText(node); } });
    ro.observe(node);
    return () => ro.disconnect();
  }, []);
  return (
    <AutoTextarea
      elRef={el} class="field" value={props.value} aria-label={t('mem.suggestLine')} disabled={props.disabled}
      onInput={(e) => props.onInput((e.target as HTMLTextAreaElement).value)}
    />
  );
}

/** Lines the model proposed for a universe after a task was done: the human edits, keeps or drops them. */
export function SuggestionCard(props: { s: MemSuggestion; compact?: boolean }): JSX.Element {
  const { s } = props;
  const lines = linesOf(s);
  const picks = !props.compact;
  void memPick.value;
  const pickState = cardPick(s);
  const [busy, setBusy] = useState(false);
  const [card, setCard] = useState<MemPreview | null>(null);
  const [err, setErr] = useState('');
  const kept = lines.filter((l) => l.text.trim());
  // Keeping is a memory change like any other: «было → стало» first, written after the human's «да».
  const ask = async (use: typeof lines = kept): Promise<void> => {
    setBusy(true);
    setErr('');
    try { setCard(await previewSuggestion(s.task, use.filter((l) => l.text.trim()))); } catch (e) { toast(failWords(e), { icon: 'failed' }); }
    setBusy(false);
  };
  const confirm = async (a: Accept): Promise<void> => {
    if (!card) return;
    setBusy(true);
    setErr('');
    try {
      await keepSuggestion(s.task, kept, card.token, a);
      toast(t(card.draft ? 'mem.suggestKeptDraft' : 'mem.suggestKept', { repo: s.repo }));
      void loadMemory(s.repo);
      void afterAnswered(s.task);
    } catch (e) { setErr(failWords(e)); setBusy(false); }
  };
  const dismiss = async (): Promise<void> => {
    setBusy(true);
    try { await dismissSuggestion(s.task); void loadMemory(s.repo); void afterAnswered(s.task); } catch (e) { toast(failWords(e), { icon: 'failed' }); setBusy(false); }
  };
  // The lines without what was found; a new preview follows.
  const redact = (): void => {
    if (!card) return;
    const next = kept.map((l, i) => ({ ...l, text: card.redacted[i]?.body ?? l.text }));
    setLines(s.task, next);
    void ask(next);
  };
  return (
    <div class="mm-sug" data-sug={s.task}>
      <div class="mm-sug-h">
        {picks && (
          <input
            class="mm-pick" type="checkbox" data-pick-card={s.task} checked={pickState === 'all'} disabled={!!card}
            ref={(el) => { if (el) el.indeterminate = pickState === 'some'; }}
            aria-label={t('mem.batchPickCard', { tag: s.tag })} onChange={() => toggleCard(s)}
          />
        )}
        <Gl n="memory" />{t('mem.suggestFor', { tag: s.tag, repo: s.repo })}
      </div>
      {s.title && <div class="hint">{s.title}</div>}
      {lines.map((l, i) => (
        <div class="mm-sug-line" key={i}>
          {picks && (
            <input
              class="mm-pick" type="checkbox" data-pick={pickKey(s.task, i)} checked={memPick.value.has(pickKey(s.task, i))} disabled={!!card || !l.text.trim()}
              aria-label={t('mem.batchPickLine')} onChange={() => toggleLine(s, i)}
            />
          )}
          <span class={`mm-kind k-${l.kind}`}>{kindLabel(l.kind)}</span>
          <LineField value={l.text} disabled={!!card} onInput={(text) => setLines(s.task, lines.map((x, j) => (j === i ? { ...x, text } : x)))} />
        </div>
      ))}
      {card ? (
        <ChangeCard p={card} busy={busy} error={err} onConfirm={(a) => void confirm(a)} onCancel={() => { setCard(null); setErr(''); }} onRedact={redact} confirmLabel={t('mem.suggestKeep')} />
      ) : (
        <div class="row">
          <button class="btn sm primary" type="button" data-act="mem-sug-accept" disabled={busy || kept.length === 0} onClick={() => void ask()}>{t('mem.suggestKeep')}</button>
          <button class="btn sm" type="button" data-act="mem-sug-dismiss" disabled={busy} onClick={() => void dismiss()}>{t('mem.suggestNo')}</button>
        </div>
      )}
      {!props.compact && <div class="hint">{t('mem.suggestNote')}</div>}
    </div>
  );
}

/** What the Coordinator is told about the memory: a block in the board's setting `coordinator.instructions`. A change
 *  shows «было → стало» (the human's own text stays, the block is added at the end) and waits for two presses; the
 *  board's own `set` makes it. */
function Connect(props: { data: MemData }): JSX.Element | null {
  const c = props.data.connect;
  const [card, setCard] = useState<{ on: boolean; current: string; value: string | null; error: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [showBlock, setShowBlock] = useState(false);
  if (!c) return null;
  // The card is made by the server from the instructions as they are at that moment.
  const ask = async (on: boolean): Promise<void> => {
    setBusy(true);
    try {
      const p = await connectMemory(on);
      setCard({ on, current: p.current, value: p.now, error: '' });
    } catch (e) { toast(failWords(e), { icon: 'failed' }); }
    setBusy(false);
  };
  const go = (): void => {
    if (!card) return;
    confirmStep('mem-connect', async () => {
      setBusy(true);
      try {
        // what the card showed goes with the confirmation: the server refuses if the instructions changed meanwhile
        await connectMemory(card.on, card.current);
        toast(card.on ? t('mem.connectedNow') : t('mem.disconnected'));
        setCard(null);
        void loadMemory('');
      } catch (e) {
        if ((e as { status?: number }).status === 409) {
          // changed under us: nothing was sent; the card is made again from the new text
          toast(failWords(e), { icon: 'failed' });
          setBusy(false);
          await ask(card.on);
          return;
        }
        setCard({ ...card, error: failWords(e) });
      }
      setBusy(false);
    });
  };
  const tell = async (): Promise<void> => {
    const text = t('mem.tellText', { path: `${props.data.dir}/MEMORY.md` });
    if (await send(text)) toast(t('mem.told'));
  };
  const armedNow = armed.value?.key === 'mem-connect';
  const on = c.connected;
  return (
    <div class={`mm-conn ${on ? 'on' : 'off'}`} id="mem-connect">
      <div class="mm-conn-h">
        <Gl n="quasar" />
        <b>{t('mem.coordTitle')}</b>
        <span class={`badge s-${on ? 'working' : 'queued'}`}>{on ? t('mem.connOn') : c.stale ? t('mem.connStale') : t('mem.connPending')}</span>
      </div>
      <div class="hint">{c.error ? c.error : on ? t('mem.connWhy') : t('mem.connNo')}</div>
      {!card && (
        <div class="row">
          {(!on || c.stale) && <button class="btn sm" type="button" id="mem-connect-toggle" disabled={busy || !!c.error} onClick={() => void ask(true)}>{c.stale ? t('mem.connUpdate') : t('mem.connect')}</button>}
          {on && <button class="btn sm" type="button" id="mem-disconnect" disabled={busy} onClick={() => void ask(false)}>{t('mem.disconnect')}</button>}
          {on && <button class="btn sm" type="button" id="mem-tell" onClick={() => void tell()}>{t('mem.tell')}</button>}
          <button class="btn sm" type="button" id="mem-showblock" aria-pressed={showBlock} onClick={() => setShowBlock(!showBlock)}>{t('mem.showBlock')}</button>
        </div>
      )}
      {card && (
        <div class={`chg ${card.error ? 'bad' : ''}`} id="mem-change">
          <div class="chg-h">{t('mem.cardTitle', { name: "coordinator.instructions" })}</div>
          <div class="ba"><span class="kl">{t('mem.cardWas')}</span></div>
          <pre class="mm-block" id="mem-was">{card.current || t('mem.cardNone')}</pre>
          <div class="ba"><span class="kl">{t('mem.cardNow')}</span></div>
          <pre class="mm-block" id="mem-now">{card.value === null ? t('mem.cardUnset') : card.value}</pre>
          <div class="hint">{card.on ? t('mem.cardNoteOn') : t('mem.cardNoteOff')}</div>
          {card.error && <div class="hint bad" id="mem-change-error">{card.error}</div>}
          <div class="row act">
            <button class={`btn primary ${armedNow ? 'arm' : ''}`} type="button" id="mem-change-confirm" disabled={busy || !!card.error} onClick={go}>
              {armedNow ? t('mem.cardConfirm2') : t('mem.cardConfirm')}
            </button>
            <button class="btn" type="button" id="mem-change-cancel" onClick={() => setCard(null)}>{t('cancel')}</button>
          </div>
        </div>
      )}
      <div class="hint">{t('mem.connScope')}</div>
      {showBlock && (
        <>
          <pre class="mm-block" id="mem-block">{c.block}</pre>
          <div class="row">
            <button class="btn sm" type="button" onClick={() => { void navigator.clipboard?.writeText(c.block); toast(t('toast.copied')); }}>{t('mem.copy')}</button>
          </div>
          <div class="hint">{t('mem.blockNote')}</div>
        </>
      )}
    </div>
  );
}

/** The universe's memory that is not in git yet: drafts (kept outside the checkout, so the board can still merge into it).
 *  Sending shows «было → стало», checks the text for personal data once more and hands it to a board task: the planner
 *  never writes or commits into the checkout. The task's id stays on the panel. */
export function Git(props: { repo: string; data: MemData }): JSX.Element | null {
  const g = props.data.git;
  const [busy, setBusy] = useState(false);
  const [card, setCard] = useState<MemPreview | null>(null);
  const [err, setErr] = useState('');
  if (!g) return null;
  if (!g.checkout) return <div class="hint" id="mem-git">{t('mem.notGit')}</div>;
  const ask = async (): Promise<void> => {
    setBusy(true);
    setErr('');
    try { setCard(await previewSend(props.repo)); } catch (e) { toast(failWords(e), { icon: 'failed' }); }
    setBusy(false);
  };
  const confirm = async (a: Accept): Promise<void> => {
    if (!card) return;
    setBusy(true);
    setErr('');
    try {
      const r: SentTask = await sendMemory(props.repo, card.token, a);
      toast(t('mem.sent', { tag: r.task }));
      setCard(null);
    } catch (e) { setErr(failWords(e)); }
    setBusy(false);
  };
  // What a board task has and the checkout has not yet: kept by the planner, so it survives a reload.
  const sentNote = g.sent.map((b) => (
    <div class="hint ok mm-sent" id="mem-sent" key={b.task} data-task={b.id ?? ''}>
      {t('mem.sentNote', { tag: b.task, n: b.files })}{' '}
      <button class="btn sm" type="button" data-act="mem-sent-forget" onClick={() => void forgetSent(props.repo, b.task)}>{t('mem.sentForget')}</button>
    </div>
  ));
  if (g.dirty.length === 0 && g.drafts === 0) return <div class="hint" id="mem-git">{g.sent.length === 0 && t('mem.gitClean')}{sentNote}</div>;
  return (
    <div class="mm-git" id="mem-git">
      {g.drafts > 0 && <div class="hint warn" id="mem-drafts">{tn('mem.gitDrafts', g.drafts)}</div>}
      {g.dirty.length > 0 && <div class="hint warn">{tn('mem.gitDirty', g.dirty.length)}</div>}
      {g.dirty.length > 0 && <ul class="mm-files">{g.dirty.map((d) => <li key={d.file}><code>{d.state || 'M'}</code> <code>{d.file}</code></li>)}</ul>}
      {g.dirty.length > 0 && <div class="hint" id="mem-dirty-own">{t('mem.gitDirtyOwn')}</div>}
      {card ? (
        <ChangeCard p={card} busy={busy} error={err} onConfirm={(a) => void confirm(a)} onCancel={() => { setCard(null); setErr(''); }} confirmLabel={t('mem.send')} />
      ) : g.drafts > 0 && (
        <div class="row">
          <button class="btn sm" type="button" id="mem-send" disabled={busy} onClick={() => void ask()}>{t('mem.send')}</button>
        </div>
      )}
      {g.drafts > 0 && <div class="hint">{t('mem.sendNote')}</div>}
      {sentNote}
    </div>
  );
}

export const matches = (e: MemEntry, q: string): boolean => {
  const n = q.trim().toLowerCase();
  return !n || `${e.title} ${e.description} ${e.body} ${e.id}`.toLowerCase().includes(n);
};

/** The memory of one level in the narrow column of the Focus: a summary (how many entries, how many wait for a merge or are
 *  drafts), a search and the button that opens the list in a window (`MemoryWindow`). The Galaxy's level (`repo` empty)
 *  also holds the Coordinator's wiring; a universe's, the lines the model proposed after a task. */
export function MemoryPanel(props: { repo: string }): JSX.Element {
  const { repo } = props;
  const st = memoryOf(repo);
  const data = st.data;
  const [q, setQ] = useState('');
  const seq = snap.value?.seq ?? 0;
  const lastLoad = useRef(0);

  useEffect(() => { void loadMemory(repo); lastLoad.current = Date.now(); }, [repo]);
  // Files change under us (an agent wrote one): a fresh look when the board's state moves, at most every 6 s.
  useEffect(() => {
    if (Date.now() - lastLoad.current > 6000) { lastLoad.current = Date.now(); void loadMemory(repo); }
  }, [seq]);

  const title = repo ? t('mem.titleUniverse', { repo }) : t('mem.titleGalaxy', { g: snap.value?.host || '' });
  const entries = data?.entries ?? [];
  const waiting = entries.filter((e) => stateOf(e) === 'waiting').length;
  const drafts = entries.filter((e) => stateOf(e) === 'draft').length;
  const foreign = repo ? [] : (data?.foreign ?? []).filter((f) => f.entries.length > 0);
  const universeSug = repo ? (data?.suggestions ?? []) : [];
  const open = (w: { q?: string; program?: string } = {}): void => openMemory({ repo, ...w });

  return (
    <section class="mm" id={repo ? 'mem-universe' : 'mem-galaxy'} data-mem-level={repo ? 'universe' : 'galaxy'} aria-label={title}>
      <h3 class="mm-h">
        <Gl n="memory" />{title}
        <span class="cnt" id={repo ? 'mem-universe-count' : 'mem-galaxy-count'}>{entries.length}</span>
        {st.loading && <span class="spinner" />}
      </h3>
      <div class="hint">{repo ? t('mem.hintUniverse') : t('mem.hintGalaxy')}</div>
      {data && <div class="hint mm-dir"><code title={data.dir}>{data.dir}</code></div>}
      {data?.note && <div class="hint">{data.note}</div>}
      {st.error && <div class="hint bad" id="mem-load-error">{st.error}</div>}

      <BatchBar scope={repo} list={universeSug} />
      {universeSug.map((s) => <SuggestionCard key={s.task} s={s} />)}

      {!repo && data && <Connect data={data} />}
      {!repo && data?.settings && (
        <div class="mm-set">
          <button class={`tog ${data.settings.suggest ? 'on' : ''}`} type="button" id="mem-suggest-toggle" aria-pressed={data.settings.suggest}
            onClick={() => { void setSuggest(!data.settings!.suggest).catch((e) => toast(failWords(e), { icon: 'failed' })); }}>{data.settings.suggest ? t('mem.suggestOn') : t('mem.suggestOff')}</button>
          <span class="hint">{t('mem.suggestSetting')}</span>
        </div>
      )}

      <div class="mm-sum" id="mem-summary">
        <div class="mm-stats">
          <span class="mm-stat" id="mem-stat-all">{tn('mem.statEntries', entries.length)}</span>
          {waiting > 0 && <span class="mm-stat warn" id="mem-stat-waiting">{t('mem.statWaiting', { n: waiting })}</span>}
          {drafts > 0 && <span class="mm-stat warn" id="mem-stat-drafts">{t('mem.statDrafts', { n: drafts })}</span>}
          {foreign.map((f) => <span class="mm-stat" key={f.program} data-foreign-stat={f.program}>{t('mem.statForeign', { p: f.label, n: f.entries.length })}</span>)}
        </div>
        {data && entries.length === 0 && foreign.length === 0 && <div class="hint mm-empty">{repo ? t('mem.emptyUniverse') : t('mem.emptyGalaxy')}</div>}
        <input
          class="field" id="mem-filter" type="search" value={q} placeholder={t('mem.filterOpen')} aria-label={t('mem.filter')}
          onInput={(e) => setQ((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); open({ q }); } }}
        />
        <div class="row">
          <button class="btn primary" type="button" id="mem-open" onClick={() => open({ q })}><Gl n="expand" />{t('mem.open')}</button>
        </div>
      </div>
    </section>
  );
}

/* ───────── «В память» ───────── */

const dText = signal('');
const dTitle = signal('');
const dKind = signal('rule');
const dLevel = signal<'galaxy' | 'universe'>('galaxy');
const dRepo = signal('');
const dBusy = signal(false);
const dErr = signal('');
const dCard = signal<{ p: MemPreview; body: SaveBody } | null>(null);
let shown: unknown = null;

/** The dialog that puts a message, a closing note or a sentence into the Galaxy's or a universe's memory. */
export function RememberDialog(): JSX.Element | null {
  const init = rememberDlg.value;
  const view = currentView.value;
  // A fresh opening resets the form.
  if (init !== shown) {
    shown = init;
    if (init) {
      dText.value = init.text;
      dTitle.value = init.title ?? '';
      dRepo.value = init.repo && view.projects.some((p) => p.name === init.repo) ? init.repo : view.projects[0]?.name ?? '';
      dLevel.value = init.repo && dRepo.value ? 'universe' : 'galaxy';
      dKind.value = init.kind ?? (dLevel.value === 'universe' ? 'decision' : 'rule');
      dBusy.value = false;
      dErr.value = '';
      dCard.value = null;
    }
  }
  // The text field takes the focus, so Esc reaches the page (and not the chat line the click came from).
  useEffect(() => { if (init) (document.getElementById('rem-text') as HTMLTextAreaElement | null)?.focus(); }, [init]);
  if (!init) return null;
  const close = (): void => { rememberDlg.value = null; };
  const universe = dLevel.value === 'universe';
  const where = universe ? t('mem.whereUniverse', { repo: dRepo.value }) : t('mem.whereGalaxy', { g: snap.value?.host || '' });
  // «В память» shows «было → стало» first; the entry is written when the card is confirmed.
  const ask = async (toGalaxy = false): Promise<void> => {
    dBusy.value = true;
    dErr.value = '';
    try {
      const repo = universe && !toGalaxy ? dRepo.value : '';
      const body: SaveBody = { repo, title: dTitle.value, kind: dKind.value, body: dText.value, source: init.source };
      dCard.value = { p: await previewSave(body), body };
    } catch (e) { dErr.value = failWords(e); }
    dBusy.value = false;
  };
  const confirm = async (a: Accept): Promise<void> => {
    const c = dCard.value;
    if (!c) return;
    dBusy.value = true;
    dErr.value = '';
    try {
      await saveEntry(c.body, c.p.token, a);
      toast(t('mem.rememberedIn', { where: c.body.repo ? t('mem.whereUniverse', { repo: c.body.repo }) : t('mem.whereGalaxy', { g: snap.value?.host || '' }) }));
      close();
    } catch (e) { dErr.value = failWords(e); dBusy.value = false; }
  };
  const toGalaxy = (): void => { dLevel.value = 'galaxy'; if (dKind.value === 'decision') dKind.value = 'rule'; void ask(true); };
  const redact = (): void => {
    const r = dCard.value?.p.redacted[0];
    if (!r) return;
    dTitle.value = r.title;
    dText.value = r.body;
    void ask();
  };
  const card = dCard.value;
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}>
      <section class="panel dlg" id="remember" role="dialog" aria-label={t('mem.rememberTitle')}>
        <header><b>{t('mem.rememberTitle')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={close}><Gl n="close" /></button></header>
        <div class="bd">
          <div class="mm-level" role="group" aria-label={t('mem.level')}>
            <button type="button" class={`btn sm ${!universe ? 'on' : ''}`} id="rem-galaxy" onClick={() => { dCard.value = null; dLevel.value = 'galaxy'; if (dKind.value === 'decision') dKind.value = 'rule'; }}>{t('mem.levelGalaxy')}</button>
            <button type="button" class={`btn sm ${universe ? 'on' : ''}`} id="rem-universe" disabled={view.projects.length === 0} onClick={() => { dCard.value = null; dLevel.value = 'universe'; if (dKind.value === 'rule') dKind.value = 'decision'; }}>{t('mem.levelUniverse')}</button>
            {universe && (
              <select class="field" id="rem-repo" value={dRepo.value} aria-label={t('mem.fRepo')} onChange={(e) => { dCard.value = null; dRepo.value = (e.target as HTMLSelectElement).value; }}>
                {view.projects.map((p) => <option value={p.name}>{p.name}</option>)}
              </select>
            )}
          </div>
          <div class="hint">{universe ? t('mem.levelUniverseHint') : t('mem.levelGalaxyHint')}</div>
          <label class="fl2">{t('mem.fBody')}
            <textarea class="field" id="rem-text" rows={6} value={dText.value} disabled={!!card} onInput={(e) => { dText.value = (e.target as HTMLTextAreaElement).value; }} />
          </label>
          <div class="mm-two">
            <label class="fl2">{t('mem.fTitle')}
              <input class="field" id="rem-title" value={dTitle.value} maxLength={80} disabled={!!card} placeholder={t('mem.fTitlePh')} onInput={(e) => { dTitle.value = (e.target as HTMLInputElement).value; }} />
            </label>
            <label class="fl2">{t('mem.fKind')}
              <select class="field" id="rem-kind" value={dKind.value} onChange={(e) => { dKind.value = (e.target as HTMLSelectElement).value; }}>
                {KINDS.map((k) => <option value={k}>{t(`mem.kind.${k}`)}</option>)}
              </select>
            </label>
          </div>
          {init.source && <div class="hint">{t('mem.from', { s: init.source })}</div>}
          {dErr.value && !card && <div class="hint bad" id="rem-err">{dErr.value}</div>}
          {card ? (
            <ChangeCard
              p={card.p} busy={dBusy.value} error={dErr.value} onConfirm={(a) => void confirm(a)} onCancel={() => { dCard.value = null; dErr.value = ''; }}
              onRedact={redact} onToGalaxy={card.body.repo ? toGalaxy : undefined} confirmLabel={t('mem.rememberGo', { where: card.body.repo ? t('mem.whereUniverse', { repo: card.body.repo }) : t('mem.whereGalaxy', { g: snap.value?.host || '' }) })}
            />
          ) : (
            <div class="act">
              <button class="btn primary" type="button" id="rem-go" disabled={dBusy.value || !dText.value.trim()} onClick={() => void ask()}>{t('mem.rememberGo', { where })}</button>
              <button class="btn" type="button" onClick={close}>{t('cancel')}</button>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

/** Opens a found memory entry: flies to the Quasar (Galaxy level) or the universe and opens the memory window on it. */
export function openMemoryEntry(docId: string): void {
  const { repo, foreign, focus } = parseDocId(docId);
  if (repo) void navigateTo({ kind: 'universe', id: repo });
  else void navigateTo({ kind: 'quasar', id: 'quasar' });
  openMemory({ repo, program: foreign || undefined, open: focus });
}

/** The suggestions waiting for a task's universe, for the task's own panel. */
export function suggestionsOfTask(taskId: string): MemSuggestion[] {
  return (snap.value?.memory_suggestions ?? []).filter((s) => s.task === taskId);
}

