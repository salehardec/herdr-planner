import { signal } from '@preact/signals';
import { Fragment, type JSX } from 'preact';
import { useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import {
  chat, panels, persist, snap, screens,
} from '../store';
import {
  addFiles, draftRequest, draftTodos, hasOlder, loadLatest, loadOlder, loadOlderWait, notePending, openChatAt, optimistic, pendingFiles, pendingMessage, reconcile, removeFile, scrollTo,
  searchChat, send, sending,
} from '../chat';
import { hasFiles, pastedFiles } from '../attach';
import { parseOptions } from '../requests';
import { post, refreshNow, onChatEvent } from '../api';
import { fetchScreen, todoToDraft } from '../actions';
import { t, tn } from '../i18n';
import { sigOf } from '../chat-read';
import { markRead, markReadToEnd, SEE_MS, unreadTurns, windowActive } from '../chat-unread';
import { dayKey, dayTimeOf, isToday, timeOf } from '../util/dates';
import { Gl } from './Gl';
import { Grip } from './Grip';
import { RichText } from './RichText';
import { Memory } from './FocusOther';
import { dialogOpen } from '../dialogs';
import { PendingFiles, SavedFiles } from './Attached';
import { ChatTurn, PendingTurn } from './ChatTurn';
import { RefText } from './TaskChip';
import { AutoTextarea, fitText } from './AutoText';
import { KeyButtons } from './Keys';
import { MenuSlot } from './MenuCard';
import { Completion, useComplete } from './Complete';
import { chatBack, chatWide, setChatWide } from '../chat-wide';
import { dragTodo } from '../todo-drag';
import type { MenuData, TurnData } from '../types';
import './chat.css';

const stuck = signal(true);
/** The words typed and not sent yet survive a reload. */
export const DRAFT_KEY = 'planner.chat.draft';
const readDraft = (): string => { try { return localStorage.getItem(DRAFT_KEY) ?? ''; } catch { return ''; } };
const clearDraft = (): void => { try { localStorage.removeItem(DRAFT_KEY); } catch { /* private mode */ } };
/** A clock for the «working» line: ticks once a second while the chat shows it. */
const now = signal(Date.now());

function Shots(props: { list: { id: string; name: string; mime?: string }[] }): JSX.Element | null {
  return <SavedFiles list={props.list} class="shots" zoom />;
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

/** «1 мин 20 с» / «1 min 20 s» from milliseconds. */
function elapsedText(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor(total / 60) % 60;
  const sec = total % 60;
  return h > 0 ? t('chat.elapsedHm', { h, m }) : m > 0 ? t('chat.elapsedMs', { m, s: sec }) : t('chat.elapsedS', { s: sec });
}

/** The Coordinator is working: for how long, what it does now (its last tool call), how many sub-agents run. */
function Working(props: { since: number; doing: string; subagents: number }): JSX.Element {
  useEffect(() => {
    const id = window.setInterval(() => { now.value = Date.now(); }, 1000);
    return () => window.clearInterval(id);
  }, []);
  // no turn to count from (the Coordinator is busy with something that is not a message of this chat): no time
  const since = props.since;
  return (
    <div class="working" id="cworking" role="status" aria-live="polite">
      <span class="pulse" aria-hidden="true" />
      <span class="who">{t('quasar.short')}</span>
      <span>{since > 0 ? t('chat.working', { time: elapsedText(now.value - since) }) : t('chat.workingNow')}</span>
      {props.doing && <><span class="sep">·</span><span class="doing" title={props.doing}>{props.doing}</span></>}
      {props.subagents > 0 && <><span class="sep">·</span><span>{tn('chat.subagentsN', props.subagents)}</span></>}
      <span class="dots"><i /><i /><i /></span>
    </div>
  );
}

/** A day rule between the turns of different days. */
function DayRule(props: { at: number }): JSX.Element {
  const key = dayKey(props.at);
  const yesterday = dayKey(Date.now() - 86_400_000);
  const label = isToday(props.at) ? t('chat.today') : key === yesterday ? `${t('chat.yesterday')} · ${key}` : key;
  return <div class="day" role="separator">{label}</div>;
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
  // A line the planner sent is written into the transcript by the board too: it is shown once, as that board turn (or as
  // the notice while the transcript does not have it yet), never twice. A notice older than the newest turn loaded here
  // is history — its turn may simply not be in this page (the chat holds the last few), and a line the Coordinator read
  // during a run is not a turn at all; either way it does not belong under newer turns with its old time.
  const newestTurn = c.turns.reduce((m, x) => Math.max(m, x.at_a || x.at || 0), 0);
  const notices = (coord?.notices ?? []).filter((n) => !c.turns.some((x) => x.q.trim() === n.text.trim())
    && (newestTurn === 0 || n.at > newestTurn));
  const reply = coord?.reply ?? '';
  const err = c.error || (coord?.error ?? '');
  const busy = !!coord?.busy;
  const dialog = !!coord?.dialog;
  // The Coordinator's last words: its transcript's, or (while that answer is not written yet) the tail of its window.
  const lastReply = (showTurns ? c.turns[c.turns.length - 1].a : '') || reply;
  // The turn it is answering now: no answer yet, or an interim line and a tool call still going (`doing`). The
  // working line counts from it and shows what it does.
  const lastTurn = showTurns ? c.turns[c.turns.length - 1] : null;
  const lastOpen = lastTurn && (!lastTurn.a || lastTurn.doing) ? lastTurn : null;
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

  // «At the end» reads everything only when the reader is there on purpose: not before the jump to the first unread on opening
  // has been decided, and not while the log is still scrolling to it.
  const jumped = useRef(false);
  const opened = useRef(false);
  const navUntil = useRef(0);
  const readsEnd = (el: HTMLElement): boolean => opened.current && Date.now() > navUntil.current && el.scrollHeight - el.scrollTop - el.clientHeight < 24;

  // Keep the end in view unless the reader scrolled up; keep the place after older turns are added.
  useLayoutEffect(() => {
    const el = logRef.current;
    if (!el) return;
    if (anchor.current) { el.scrollTop = anchor.current.top + (el.scrollHeight - anchor.current.h); anchor.current = null; }
    else if (stuck.value) el.scrollTop = el.scrollHeight;
    if (open && active && readsEnd(el)) markReadToEnd();
  }, [c.turns, pending?.at, opt?.at, reply, open, busy, c.loading, active]);

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
  const toUnread = async (onlyIfHidden: boolean, first = unreadTurns.peek()[0]): Promise<void> => {
    if (first === undefined) return;
    for (let i = 0; i < 40 && !chat.peek().turns.some((x) => x.n === first) && hasOlder(); i++) {
      if (!(await loadOlderWait())) break;
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
    navUntil.current = Date.now() + 1500;
    scrollTo.value = { n: first, at: Date.now(), block: 'start' };
    // When the scroll has come to rest, the log follows new answers again if it ended at the bottom (the unread were the last ones).
    window.setTimeout(() => {
      const el = logRef.current;
      if (!el) return;
      const atEnd = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      stuck.value = atEnd;
      if (atEnd && open && active) markReadToEnd();
    }, 1600);
  };

  // Opening the chat shows what is new in it. A chat that is open from the start (the wide one is restored open) waits for the history.
  const ready = c.available !== null && !c.loadingLatest;
  useEffect(() => {
    if (!open) { jumped.current = false; opened.current = false; return; }
    if (!ready || jumped.current) return;
    jumped.current = true;
    let ran = false;
    const id = requestAnimationFrame(() => { ran = true; void toUnread(true).finally(() => { opened.current = true; }); });
    // A history that starts loading again before the frame cancels the jump: it is made when the history is there.
    return () => { cancelAnimationFrame(id); if (!ran) jumped.current = false; };
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
    const atEnd = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    // (the first frames of a scroll away from the end still look like the end: they must not pin the log to it again)
    stuck.value = atEnd && Date.now() > navUntil.current;
    // Reaching the end acknowledges every answer, rather than waiting for an observer timer to fire.
    if (open && active && readsEnd(el)) markReadToEnd();
    // (not on the way to the first unread: loading older turns there would pull the log back to where it started)
    if (open && showTurns && el.scrollTop < 40 && Date.now() > navUntil.current) void older();
  };

  const submit = async (): Promise<void> => {
    const ta = taRef.current;
    if (!ta) return;
    comp.close();
    stuck.value = true;
    const text = ta.value;
    const ok = await send(text);
    if (ok) { ta.value = ''; fitText(ta); clearDraft(); }
  };
  // The draft: restored when the chat mounts, kept a moment after each keystroke, dropped once sent or on Esc.
  const draftTimer = useRef(0);
  useEffect(() => {
    const ta = taRef.current;
    const saved = readDraft();
    if (ta && saved && !ta.value) { ta.value = saved; fitText(ta); }
  }, []);
  const keepDraft = (): void => {
    window.clearTimeout(draftTimer.current);
    draftTimer.current = window.setTimeout(() => {
      const v = taRef.current?.value ?? '';
      if (v.trim()) persist(DRAFT_KEY, v); else clearDraft();
    }, 300);
  };

  const onKey = (e: KeyboardEvent): void => {
    const ta = e.target as HTMLTextAreaElement;
    // A dialog over the chat (the «В память» one) takes Esc: the page's own handler closes it.
    if (e.key === 'Escape' && dialogOpen()) return;
    if (e.key === 'Escape') { ta.value = ''; fitText(ta); clearDraft(); draftTodos.value = []; ta.blur(); chatBack(); e.stopPropagation(); return; }
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
          <button class="btn sm ubtn" id="cunread" type="button" title={t('chat.toUnread')} onClick={() => {
            const first = unread[0];
            markReadToEnd();
            void toUnread(false, first);
          }}>
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
            {c.turns.map((x, i) => {
              const prev = i > 0 ? c.turns[i - 1] : null;
              const newDay = !prev || dayKey(prev.at) !== dayKey(x.at);
              // a notice after another unanswered notice within five minutes: the label and the time are not repeated
              const compact = !!prev && !newDay && prev.from === 'board' && !prev.a && x.from === 'board' && x.at - prev.at < 5 * 60_000;
              return (
                <Fragment key={x.n}>
                  {newDay && x.at > 0 && <DayRule at={x.at} />}
                  <ChatTurn turn={x} hit={c.highlight === x.n} unread={unread.includes(x.n)} compact={compact} onReuse={(q) => { const ta = taRef.current; if (ta) { ta.value = q; fitText(ta); ta.focus(); ta.setSelectionRange(q.length, q.length); keepDraft(); } }} />
                </Fragment>
              );
            })}
          </>
        ) : (
          <Fallback
            history={history} reply={reply} status={coord?.status ?? ''}
            sentAt={coord?.sent_at ?? 0} sent={coord?.last_sent ?? ''}
          />
        )}
        {notices.map((n) => (
          <div class="turn board" data-notice={n.at} key={`notice-${n.at}-${n.text}`}>
            <div class="msg notice" tabIndex={0}>
              <span class="who"><Gl n="board" />{t('chat.board')}</span>
              <span class="time">{timeOf(n.at)}</span>
              <div class="txt"><RefText text={n.text} /></div>
            </div>
          </div>
        ))}
        {pending && <PendingTurn text={pending.text} attachments={pending.attachments} />}
        {err && (
          <div class="msg a err"><span class="who"><Gl n="quasar" />{t('quasar.short')}</span><span class="txt">{err}</span></div>
        )}
        {busy && !err && <Working since={lastOpen?.at ?? 0} doing={lastOpen?.doing ?? ''} subagents={(coord?.subagents ?? []).filter((x) => x.state === 'running').length} />}
      </div>
      {!stuck.value && showTurns && (
        <button class={`btn sm tolast ${unread.length ? 'news' : ''}`} id="tolast" type="button" aria-live="polite" onClick={() => {
          stuck.value = true;
          const el = logRef.current;
          if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
          void loadLatest();
        }}>{unread.length ? <>↓ <span class="n">{unread.length}</span>{tn('chat.toLastN', unread.length)}</> : t('chat.toLast')}</button>
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
            {scr?.error ? <div class="hint">{scr.error}</div> : <pre>{scr?.text || (scr?.busy ? t('chat.loading') : '')}</pre>}
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
          onInput={() => { const ta = taRef.current; if (ta) comp.update(ta); keepDraft(); }}
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
        <div class="msg q"><span class="who"><Gl n="user" />{t('chat.you')}</span><span class="txt">{props.sent}</span></div>
      )}
      {props.history.map((m) => (
        <div class="turn" key={m.at}>
          <div class="msg q">
            <span class="who"><Gl n="user" />{t('chat.you')}</span>
            <span class="time">{timeOf(m.at)} · {t('chat.sentToQuasar')}</span>
            <span class="txt">{m.text || t('chat.images')}</span>
            <Shots list={m.attachments} />
          </div>
        </div>
      ))}
      {props.reply && (
        <div class="msg a">
          <span class="who"><Gl n="quasar" />{t('quasar.short')}</span>
          <span class="time">{t('chat.lastFromWindow')} · {props.status}</span>
          <RichText class="txt" text={props.reply} />
        </div>
      )}
      {!props.reply && !props.history.length && !props.sent && (
        <div class="msg a empty"><span class="who"><Gl n="quasar" />{t('quasar.short')}</span><span class="txt">{t('chat.empty')}</span></div>
      )}
    </>
  );
}
