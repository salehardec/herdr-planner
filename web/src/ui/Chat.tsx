import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import {
  chat, panels, snap, screens,
} from '../store';
import {
  addFiles, draftRequest, draftTodos, hasOlder, loadLatest, loadOlder, notePending, openChatAt, optimistic, pendingFiles, pendingMessage, reconcile, removeFile, scrollTo,
  searchChat, send, sending,
} from '../chat';
import { hasFiles, pastedFiles } from '../attach';
import { parseOptions } from '../requests';
import { post, refreshNow, onChatEvent } from '../api';
import { fetchScreen, todoToDraft } from '../actions';
import { t, tn } from '../i18n';
import { sigOf } from '../chat-read';
import { markRead, SEE_MS, unreadCount, unreadTurns, windowActive } from '../chat-unread';
import { dayTimeOf, timeOf } from '../util/dates';
import { Gl } from './Gl';
import { Grip } from './Grip';
import { RichText } from './RichText';
import { Memory } from './FocusOther';
import { openRemember } from './memory-api';
import { dialogOpen } from '../dialogs';
import { PendingFiles, SavedFiles } from './Attached';
import { AutoTextarea, fitText } from './AutoText';
import { KeyButtons } from './Keys';
import { MenuSlot } from './MenuCard';
import { Completion, useComplete } from './Complete';
import { chatBack, chatWide, setChatWide } from '../chat-wide';
import { dragTodo } from '../todo-drag';
import type { MenuData, TurnData } from '../types';
import './chat.css';

const wide = signal<Set<number>>(new Set());
const stuck = signal(true);

function Shots(props: { list: { id: string; name: string; mime?: string }[] }): JSX.Element | null {
  return <SavedFiles list={props.list} class="shots" />;
}

const todoOver = signal(false);

/**
 * Where the caret is in the wrapped text: on the first or on the last visual line. A paragraph without
 * line breaks may still take several lines, so the line breaks alone do not answer this.
 */
function caretLines(ta: HTMLTextAreaElement): { first: boolean; last: boolean } {
  const cs = getComputedStyle(ta);
  const mirror = document.createElement('div');
  mirror.setAttribute('aria-hidden', 'true');
  mirror.style.cssText = 'position:absolute;visibility:hidden;left:-100000px;top:0;height:auto;margin:0;border:0;box-sizing:border-box;overflow:visible;';
  mirror.style.width = `${ta.clientWidth}px`;
  for (const p of [
    'font', 'font-family', 'font-size', 'font-weight', 'font-style', 'letter-spacing', 'line-height',
    'text-transform', 'word-spacing', 'tab-size', 'padding-top', 'padding-right', 'padding-bottom',
    'padding-left', 'white-space', 'overflow-wrap', 'word-break', 'direction',
  ]) mirror.style.setProperty(p, cs.getPropertyValue(p));
  // The whole text, with the caret marked inside it: the text after the caret is what makes a line
  // that is not the last one, so a mirror that stopped at the caret would always say «last».
  mirror.textContent = ta.value.slice(0, ta.selectionStart);
  const marker = document.createElement('span');
  marker.textContent = '\u200b';
  mirror.appendChild(marker);
  mirror.appendChild(document.createTextNode(ta.value.slice(ta.selectionStart)));
  // A textarea shows an empty line after a trailing break; a div with `pre-wrap` gives it no box,
  // so a zero-width character keeps that line measurable and the caret above it is not «last».
  if (ta.value.endsWith('\n')) mirror.appendChild(document.createTextNode('\u200b'));
  document.body.appendChild(mirror);
  const box = mirror.getBoundingClientRect();
  const at = marker.getBoundingClientRect();
  const padTop = parseFloat(cs.paddingTop) || 0;
  const padBottom = parseFloat(cs.paddingBottom) || 0;
  const line = parseFloat(cs.lineHeight) || at.height || 16;
  // The marker's rect is the glyph, not the line box: take half of the line's leading off.
  const lineTop = at.top - Math.max(0, (line - at.height) / 2);
  const first = lineTop <= box.top + padTop + 1;
  const last = lineTop >= box.top + mirror.scrollHeight - padBottom - line - 1;
  mirror.remove();
  return { first, last };
}

function Turn(props: { turn: TurnData; hit: boolean; unread: boolean; onReuse: (q: string) => void }): JSX.Element {
  const tn = props.turn;
  const long = tn.a.length > 520 || tn.a.split('\n').length > 11;
  const open = wide.value.has(tn.n);
  return (
    <div class={`turn ${tn.from} ${props.hit ? 'hit' : ''}`} data-n={tn.n}>
      <div class="q" data-reuse={tn.n} title={t('chat.reuse')} onClick={() => props.onReuse(tn.q)} data-unread={props.unread && !tn.a ? tn.n : undefined} data-sig={0}>
        <span class="who">
          {t('chat.you')}
          {tn.from === 'board' && !tn.a && <i class={`udot ${props.unread ? 'on' : ''}`} role={props.unread ? 'img' : undefined} aria-label={props.unread ? t('chat.unreadDot') : undefined} title={props.unread ? t('chat.unreadDot') : undefined} />}
        </span>
        <span class="txt">{tn.q}</span>
        <button class="btn sm mm-tomem" type="button" data-act="to-memory" data-who="q" title={t('mem.toMemoryTip')}
          onClick={(e) => { e.stopPropagation(); openRemember({ text: tn.q, source: 'chat' }); }}><Gl n="memory" />{t('mem.toMemory')}</button>
      </div>
      <div class="meta"><span>{dayTimeOf(tn.at)}</span>{tn.from === 'board' && <span>· {t('chat.boardNotice')}</span>}</div>
      {tn.a && (
        <>
          <div class="a" data-unread={props.unread ? tn.n : undefined} data-sig={sigOf(tn.a)}>
            <span class="who">
              {t('quasar.short')}
              <i class={`udot ${props.unread ? 'on' : ''}`} role={props.unread ? 'img' : undefined} aria-label={props.unread ? t('chat.unreadDot') : undefined} title={props.unread ? t('chat.unreadDot') : undefined} />
            </span>
            <RichText class={`txt ${long && !open ? 'clamp' : ''}`} text={tn.a} />
            <button class="btn sm mm-tomem" type="button" data-act="to-memory" data-who="a" title={t('mem.toMemoryTip')}
              onClick={() => openRemember({ text: tn.a, source: 'chat' })}><Gl n="memory" />{t('mem.toMemory')}</button>
          </div>
          {long && (
            <button class="more" type="button" data-chat="turn" onClick={() => {
              const s = new Set(wide.value);
              if (s.has(tn.n)) s.delete(tn.n); else s.add(tn.n);
              wide.value = s;
            }}>{open ? t('chat.less') : t('chat.more')}</button>
          )}
        </>
      )}
    </div>
  );
}

/** A visible share of the element that counts as «seen»: most of it, or, for a tall one, most of what the log can show. */
function seen(e: IntersectionObserverEntry): boolean {
  const room = e.rootBounds?.height ?? 0;
  return e.isIntersecting && e.intersectionRect.height >= Math.min(e.boundingClientRect.height * 0.6, room * 0.5) - 1;
}

export function Chat(): JSX.Element {
  const c = chat.value;
  const open = panels.chat.value;
  const s = snap.value;
  const coord = s?.coordinator;
  const logRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const anchor = useRef<{ h: number; top: number } | null>(null);
  const showTurns = c.available === true && c.turns.length > 0;
  const pending = pendingMessage();
  const opt = optimistic.value;
  const history = coord?.history ?? [];
  const reply = coord?.reply ?? '';
  const err = c.error || (coord?.error ?? '');
  const busy = !!coord?.busy;
  const dialog = !!coord?.dialog;
  // The Coordinator's last words: its transcript's, or (while that answer is not written yet) the tail of its window.
  const lastReply = (showTurns ? c.turns[c.turns.length - 1].a : '') || reply;
  const options = dialog || busy ? [] : parseOptions(lastReply);
  const scr = screens.value.coordinator;
  const unread = unreadTurns.value;
  // With the words they were read as: an answer that changes (an interim line, then the real one) is news again.
  const unreadKey = unread.map((n) => `${n}:${sigOf(c.turns.find((x) => x.n === n)?.a ?? '')}`).join(',');
  const active = windowActive.value;
  // The commands of the program that reads these words: the Coordinator's own.
  const comp = useComplete({ program: coord?.program ?? '', dir: '' });
  // The Coordinator's selection menu, read from its screen: shown as a card with buttons while the screen has it.
  const menu: MenuData | null = dialog && !scr?.error ? (scr?.menu ?? null) : null;

  // Load the transcript when the page starts, when the chat opens and when it grows: a hidden chat counts what is new in it.
  useEffect(() => { void loadLatest(); }, [open]);
  useEffect(() => onChatEvent(() => { void loadLatest(); }), []);
  useEffect(() => { if (open && dialog) void fetchScreen('coordinator'); }, [open, dialog, coord?.reply]);
  // While the Coordinator waits for a choice the card follows the screen: an answer given in the terminal closes it.
  useEffect(() => {
    if (!open || !dialog) return;
    const id = window.setInterval(() => { if (!document.hidden) void fetchScreen('coordinator'); }, 1500);
    return () => window.clearInterval(id);
  }, [open, dialog]);
  useEffect(() => { reconcile(); }, [coord?.history.length]);

  // Keep the end in view unless the reader scrolled up; keep the place after older turns are added.
  useLayoutEffect(() => {
    const el = logRef.current;
    if (!el) return;
    if (anchor.current) { el.scrollTop = anchor.current.top + (el.scrollHeight - anchor.current.h); anchor.current = null; }
    else if (stuck.value) el.scrollTop = el.scrollHeight;
  }, [c.turns.length, pending?.at, opt?.at, reply, open, busy, c.loading]);

  useEffect(() => {
    const target = scrollTo.value;
    if (!target) return;
    requestAnimationFrame(() => {
      const el = logRef.current?.querySelector<HTMLElement>(`[data-n="${target.n}"]`);
      if (el) { stuck.value = false; el.scrollIntoView({ block: target.block ?? 'center', behavior: 'smooth' }); }
    });
  }, [scrollTo.value]);

  // A message counts as read once it has been in view for a moment while the chat is open and the window is the one in front.
  useEffect(() => {
    const log = logRef.current;
    if (!log || !open || !active || !unreadKey || typeof IntersectionObserver === 'undefined') return;
    const timers = new Map<number, number>();
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        const n = Number((e.target as HTMLElement).dataset.unread);
        const sig = Number((e.target as HTMLElement).dataset.sig);
        const timer = timers.get(n);
        if (seen(e)) {
          if (timer === undefined) timers.set(n, window.setTimeout(() => { timers.delete(n); markRead([[n, sig]]); }, SEE_MS));
        } else if (timer !== undefined) { window.clearTimeout(timer); timers.delete(n); }
      }
    }, { root: log, threshold: Array.from({ length: 21 }, (_, i) => i / 20) });
    log.querySelectorAll('[data-unread]').forEach((el) => io.observe(el));
    return () => { io.disconnect(); timers.forEach((id) => window.clearTimeout(id)); };
  }, [open, active, unreadKey, c.turns.length]);

  /** Takes the log to the first unread turn: older turns are loaded until it is there. */
  const toUnread = async (onlyIfHidden: boolean): Promise<void> => {
    const first = unreadTurns.peek()[0];
    if (first === undefined) return;
    for (let i = 0; i < 40 && !chat.peek().turns.some((x) => x.n === first) && hasOlder(); i++) {
      if (!(await loadOlder())) break;
    }
    const log = logRef.current;
    const el = log?.querySelector<HTMLElement>(`[data-n="${first}"]`);
    if (!log || !el) return;
    if (onlyIfHidden) {
      const at = el.getBoundingClientRect();
      const room = log.getBoundingClientRect();
      if (at.top >= room.top && at.bottom <= room.bottom) return;
    }
    stuck.value = false;
    scrollTo.value = { n: first, at: Date.now(), block: 'start' };
  };

  // Opening the chat shows what is new in it. A chat that is open from the start (the wide one is restored open) waits for the history.
  const jumped = useRef(false);
  const ready = c.available !== null;
  useEffect(() => {
    if (!open) { jumped.current = false; return; }
    if (!ready || jumped.current) return;
    jumped.current = true;
    const id = requestAnimationFrame(() => { void toUnread(true); });
    return () => cancelAnimationFrame(id);
  }, [open, ready]);

  const older = async (): Promise<void> => {
    stuck.value = false;
    const el = logRef.current;
    if (el) anchor.current = { h: el.scrollHeight, top: el.scrollTop };
    const got = await loadOlder();
    if (!got) anchor.current = null;
  };

  const onScroll = (): void => {
    const el = logRef.current;
    if (!el) return;
    stuck.value = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    if (open && showTurns && el.scrollTop < 40) void older();
  };

  const submit = async (): Promise<void> => {
    const ta = taRef.current;
    if (!ta) return;
    comp.close();
    stuck.value = true;
    const text = ta.value;
    const ok = await send(text);
    if (ok) { ta.value = ''; fitText(ta); }
  };

  const onKey = (e: KeyboardEvent): void => {
    const ta = e.target as HTMLTextAreaElement;
    // A dialog over the chat (the «В память» one) takes Esc: the page's own handler closes it.
    if (e.key === 'Escape' && dialogOpen()) return;
    if (e.key === 'Escape') { ta.value = ''; fitText(ta); draftTodos.value = []; ta.blur(); chatBack(); e.stopPropagation(); return; }
    const log = logRef.current;
    if (!log || !showTurns) return;
    if (!['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(e.key)) return;
    const edge = ta.selectionStart === ta.selectionEnd ? caretLines(ta) : { first: false, last: false };
    const step = e.key === 'PageUp' || e.key === 'PageDown' ? log.clientHeight * 0.85 : 70;
    if ((e.key === 'ArrowUp' && edge.first) || (e.key === 'PageUp' && edge.first)) {
      e.preventDefault();
      stuck.value = false;
      if (log.scrollTop <= 1) void older(); else log.scrollTop -= step;
    } else if ((e.key === 'ArrowDown' && edge.last) || (e.key === 'PageDown' && edge.last)) {
      e.preventDefault();
      log.scrollTop += step;
    }
  };

  const onPaste = (e: ClipboardEvent): void => {
    const files = pastedFiles(e);
    if (!files.length) return;
    e.preventDefault();
    addFiles(files);
  };

  const status = err ? t('chat.state.error') : dialog ? t('chat.state.dialog') : busy ? t('chat.state.busy') : (coord?.status || t('chat.state.ready'));
  const typed = (taRef.current?.value ?? '').trim().length > 0 || pendingFiles.value.length > 0;
  const hits = c.hits;

  const variantSend = (value: string): void => {
    notePending(value);
    void post('/api/chat', { text: value }).catch((er: Error) => {
      chat.value = { ...chat.value, pending: null, error: er.message };
    });
    setTimeout(() => { void refreshNow(); void loadLatest(); }, 900);
  };

  // Files from the desktop are attached; a todo card dropped here becomes the draft of the message.
  const dropProps = {
    onDragOver: (e: DragEvent) => {
      if (hasFiles(e.dataTransfer)) { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'; }
      else if (dragTodo.value) { e.preventDefault(); todoOver.value = true; if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'; }
    },
    onDragLeave: (e: DragEvent) => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) todoOver.value = false; },
    onDrop: (e: DragEvent) => {
      todoOver.value = false;
      const id = dragTodo.value;
      if (id && !e.dataTransfer?.files?.length) { e.preventDefault(); e.stopPropagation(); dragTodo.value = null; void todoToDraft(id); return; }
      if (e.dataTransfer?.files?.length) { e.preventDefault(); addFiles(e.dataTransfer.files); }
    },
  };
  const draft = draftRequest.value;
  useEffect(() => {
    const ta = taRef.current;
    if (!draft || !ta) return;
    draftRequest.value = null;
    ta.value = ta.value.trim() ? `${ta.value.replace(/\s+$/, '')}\n${draft.text}` : draft.text;
    fitText(ta);
    ta.focus();
    ta.setSelectionRange(ta.value.length, ta.value.length);
  }, [draft]);

  return (
    <section class={`panel chat ${chatWide.value ? 'wide' : ''} ${open ? '' : 'off'} ${todoOver.value ? 'drop' : ''}`} id="chat" aria-label={t('chat.aria')} aria-hidden={!open} {...dropProps}>
      <Grip panel="chat" side="left" />
      <header>
        <b>{t('quasar.label')}</b>
        <span class={`st ${err ? 'bad' : busy ? 'busy' : ''}`} id="cstate">● {status}</span>
        {unread.length > 0 && (
          <button class="btn sm ubtn" id="cunread" type="button" title={t('chat.toUnread')} onClick={() => void toUnread(false)}>
            <i class="udot on" />↓ {tn('chat.unreadN', unread.length)}
          </button>
        )}
        <input
          id="chat-q" class="cq" type="search" value={c.q} placeholder={t('chat.search')} aria-label={t('chat.search')}
          onInput={(e) => searchChat((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Escape') { searchChat(''); (e.target as HTMLInputElement).blur(); e.stopPropagation(); } }}
        />
        {chatWide.value && (
          <button class={`btn sm ${panels.todo.value ? 'on' : ''}`} id="cwide-todo" type="button" aria-pressed={panels.todo.value} title="T" onClick={() => { panels.todo.value = !panels.todo.value; }}>
            <Gl n="list" />{t('todo.title')}
          </button>
        )}
        <button class="btn sm" id="cwide" type="button" aria-pressed={chatWide.value} title={chatWide.value ? t('chat.collapse') : t('chat.expand')} onClick={() => setChatWide(!chatWide.value)}>
          <Gl n={chatWide.value ? 'shrink' : 'expand'} />{chatWide.value ? t('chat.collapse') : t('chat.expand')}
        </button>
        <button class="btn sm" id="chide" type="button" title="/" onClick={() => { panels.chat.value = false; }}>{t('chat.hide')}</button>
      </header>
      <div class="cmem"><Memory compact /></div>
      {c.q.trim().length >= 2 && (
        <div id="chits" class="chits">
          {c.hitsBusy && c.hits.length === 0 && <div class="h dim">{t('chat.searching')}</div>}
          {!c.hitsBusy && hits.length === 0 && <div class="h dim">{t('chat.noHits')}</div>}
          {hits.map((h) => (
            <div class="h" key={h.n} data-hit={h.n} onClick={() => { void openChatAt(h.n); }}>
              <span class="when">{dayTimeOf(h.at)}</span>
              <span dangerouslySetInnerHTML={{ __html: h.q_snip || h.a_snip }} />
              {h.a_snip && h.q_snip && <span class="a" dangerouslySetInnerHTML={{ __html: h.a_snip }} />}
            </div>
          ))}
        </div>
      )}
      <div class="log" id="clog" ref={logRef} onScroll={onScroll} aria-live="polite">
        {showTurns ? (
          <>
            {hasOlder() ? (
              <button class="older" type="button" data-chat="older" onClick={() => void older()}>
                {c.loading ? t('chat.loading') : t('chat.older')}
              </button>
            ) : <div class="meta start">{t('chat.start')}</div>}
            {c.turns.map((x) => (
              <Turn key={x.n} turn={x} hit={c.highlight === x.n} unread={unread.includes(x.n)} onReuse={(q) => { const ta = taRef.current; if (ta) { ta.value = q; fitText(ta); ta.focus(); ta.setSelectionRange(q.length, q.length); } }} />
            ))}
          </>
        ) : (
          <Fallback
            history={history} reply={reply} status={coord?.status ?? ''}
            sentAt={coord?.sent_at ?? 0} sent={coord?.last_sent ?? ''}
          />
        )}
        {pending && (
          <div class="turn pending">
            <div class="q"><span class="who">{t('chat.you')}</span><span class="txt">{pending.text}</span></div>
            <Shots list={pending.attachments} />
            <div class="meta"><span>{t('chat.sentWaiting')}</span></div>
          </div>
        )}
        {err && (
          <div class="a err"><span class="who">{t('quasar.short')}</span><span class="txt">{err}</span></div>
        )}
        {busy && !err && (
          <div class="a"><span class="who">{t('quasar.short')}</span><span class="txt">{t('chat.reading')}<span class="dots"><i /><i /><i /></span></span></div>
        )}
      </div>
      {!stuck.value && showTurns && (
        <button class="btn sm tolast" id="tolast" type="button" onClick={() => {
          stuck.value = true;
          const el = logRef.current;
          if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
          void loadLatest();
        }}>{t('chat.toLast')}</button>
      )}
      <MenuSlot
        target="coordinator" menu={menu} place="chat"
        onAnswered={() => { void fetchScreen('coordinator'); setTimeout(() => { void fetchScreen('coordinator'); void refreshNow(); }, 900); }}
        screen={(
          <>
            <pre class="menu-screen">{scr?.text}</pre>
            <KeyButtons target="coordinator" />
          </>
        )}
        fallback={dialog ? (
          <div class="dialog" id="cdialog">
            <div class="h">{t('chat.dialogTitle')}</div>
            {scr?.error ? <div class="hint">{scr.error}</div> : <pre>{scr?.text || (scr?.busy ? t('chat.reading') : '')}</pre>}
            <KeyButtons target="coordinator" />
            {!s?.herdr.running && <div class="hint">{t('chat.dialogNoHerdr')}</div>}
          </div>
        ) : null}
      />
      {options.length > 0 && (
        <div class="variants" id="cvariants">
          {options.map((o, i) => (
            <button class="btn sm" type="button" key={i} data-variant={i} onClick={() => variantSend(o.value)}>{o.label} {o.value}</button>
          ))}
        </div>
      )}
      <PendingFiles files={pendingFiles.value} onRemove={removeFile} id="attach" />
      <form class="compose" id="ask" autocomplete="off" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <Completion
          view={comp.view.value} id="ask-complete" onHover={comp.hover}
          onPick={(i) => { const ta = taRef.current; if (ta) comp.pick(i, ta); }}
        />
        <AutoTextarea
          id="ask-in" elRef={taRef} placeholder={t('chat.placeholder')} aria-label={t('chat.aria')}
          aria-controls="ask-complete" aria-expanded={!!comp.view.value}
          onSend={() => void submit()}
          onInput={() => { const ta = taRef.current; if (ta) comp.update(ta); }}
          onClick={() => { const ta = taRef.current; if (ta) comp.update(ta); }}
          onKeyDown={(e) => { const ta = taRef.current; if (ta && comp.key(e, ta)) return; onKey(e); }}
          onPaste={onPaste} onFocus={() => { panels.chat.value = true; }}
        />
        <label class="attachbtn" title={t('chat.attachTitle')}>
          <Gl n="attach" />
          <input type="file" id="file-in" accept="image/png,image/jpeg,image/webp,image/gif,audio/*,text/*,.md,.log,.json,.yaml,.yml,.toml,.csv" multiple hidden onChange={(e) => { addFiles((e.target as HTMLInputElement).files); (e.target as HTMLInputElement).value = ''; }} />
        </label>
        <button class={`btn primary send`} type="submit" id="send" data-ready={String(typed)} disabled={sending.value} aria-label={t('chat.send')}>{t('chat.send')}</button>
        <div class="keyhint" id="ask-hint">{t('text.keys.send')}</div>
      </form>
    </section>
  );
}

function Fallback(props: {
  history: { text: string; at: number; attachments: { id: string; name: string }[] }[]; reply: string;
  status: string; sentAt: number; sent: string;
}): JSX.Element {
  return (
    <>
      {!props.history.length && props.sent && (
        <div class="q"><span class="who">{t('chat.you')}</span><span class="txt">{props.sent}</span></div>
      )}
      {props.history.map((m) => (
        <div class="turn" key={m.at}>
          <div class="q"><span class="who">{t('chat.you')}</span><span class="txt">{m.text || t('chat.images')}</span></div>
          <Shots list={m.attachments} />
          <div class="meta"><span>{timeOf(m.at)}</span><span>· {t('chat.sentToQuasar')}</span></div>
        </div>
      ))}
      {props.reply && (
        <>
          <div class="a"><span class="who">{t('quasar.short')}</span><RichText class="txt" text={props.reply} /></div>
          <div class="meta"><span>{t('chat.lastFromWindow')}</span><span>· {props.status}</span></div>
        </>
      )}
      {!props.reply && !props.history.length && !props.sent && (
        <div class="a empty"><span class="who">{t('quasar.short')}</span><span class="txt">{t('chat.empty')}</span></div>
      )}
    </>
  );
}
