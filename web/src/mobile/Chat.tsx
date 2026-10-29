/* The Coordinator's chat on the phone: the conversation as bubbles, board notices as lines, a message line and a few quick phrases.
   The transcript, the Markdown and the task chips are the page's own (chat.ts, Markdown, TaskChip); only the drawing is new. */
import { Fragment, type JSX } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { chat, screens, snap } from '../store';
import { hasOlder, loadLatest, loadOlder, pendingMessages, send, sending } from '../chat';
import { onChatEvent } from '../api';
import { fetchScreen } from '../actions';
import { parseOptions } from '../requests';
import { markRead, unreadTurns, windowActive, SEE_MS } from '../chat-unread';
import { sigOf } from '../chat-read';
import { eventText, noticeStatus, parseNotice, splitAttachments, splitEvents } from '../chat-parse';
import { dayKey, isToday, timeOf } from '../util/dates';
import { t, tn } from '../i18n';
import type { MenuData, TurnData } from '../types';
import { Gl } from '../ui/Gl';
import { Markdown } from '../ui/Markdown';
import { MenuSlot } from '../ui/MenuCard';
import { SavedFiles } from '../ui/Attached';
import { RefText, TaskChip, taskOf } from '../ui/TaskChip';
import { chatShown } from './state';

const QUICK = ['m.quick.1', 'm.quick.2', 'm.quick.3'];
const PAGE = 12;

function Day(props: { at: number }): JSX.Element {
  const key = dayKey(props.at);
  const label = isToday(props.at) ? t('chat.today') : key === dayKey(Date.now() - 86_400_000) ? t('chat.yesterday') : key;
  return <div class="day" role="separator">{label}</div>;
}

function Human(props: { text: string; at: number; pending?: boolean; turn?: TurnData }): JSX.Element {
  const { text, files, pasted } = splitAttachments(props.text);
  const images = (props.turn?.images ?? []).map((mime, i) => ({
    id: `turn-${props.turn!.n}-${i}`, name: `${t('chat.image')} ${i + 1}`, mime, url: `/m/api/chat/image/${props.turn!.n}/${i}`,
  }));
  const pics = images.length ? 0 : files.length + pasted;
  return (
    <>
      <div class="who me"><b>{t('chat.you')}</b> {props.pending ? t('chat.sentWaiting') : timeOf(props.at)}</div>
      <div class={`b me ${props.pending ? 'pending' : ''}`} data-bubble="human">
        {text && <p><RefText text={text} /></p>}
        <SavedFiles list={images} />
        {pics > 0 && <p class="pic"><Gl n="image" />{tn('m.pictures', pics)}</p>}
      </div>
    </>
  );
}

function Notice(props: { turn: TurnData }): JSX.Element {
  const tn = props.turn;
  const n = parseNotice(tn.q);
  if (!n) return <div class="ev"><RefText text={tn.q} /></div>;
  const task = taskOf(n.id);
  const status = noticeStatus(n) || task?.status || 'queued';
  const state = n.stateRu || (task ? (task.label || t(`status.${task.status}`)) : '');
  return (
    <div class="ev" data-notice="1">
      <TaskChip tag={n.id} />
      {(n.title || task?.title) && <span class="evt"> «{n.title || task?.title}»</span>}
      {state && <span class={`badge s-${status}`}> {state}</span>}
      {n.note && <div class="evn"><RefText text={n.note} /></div>}
    </div>
  );
}

function Answer(props: { turn: TurnData }): JSX.Element {
  const tn = props.turn;
  const { body, events } = splitEvents(tn.a);
  return (
    <>
      {body && (
        <>
          <div class="who"><b>{t('quasar.short')}</b> {timeOf(tn.at_a || tn.at)}</div>
          <div class="b q" data-bubble="answer"><Markdown text={body} chips /></div>
        </>
      )}
      {events.map((ev, i) => <div class="ev" key={i} data-ev={ev.verb}><RefText text={eventText(ev)} /></div>)}
    </>
  );
}

function Working(props: { doing: string }): JSX.Element {
  return (
    <div class="work" id="cworking" role="status" aria-live="polite">
      <i aria-hidden="true" />{t('m.working')}{props.doing && <span class="doing"> · {props.doing}</span>}
    </div>
  );
}

export function ChatPage(): JSX.Element {
  const c = chat.value;
  const coord = snap.value?.coordinator;
  const on = chatShown.value;
  const logRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const stuck = useRef(true);
  const anchor = useRef<{ h: number; top: number } | null>(null);
  const [text, setText] = useState('');
  const pending = pendingMessages();
  const busy = !!coord?.busy;
  const dialog = !!coord?.dialog;
  const err = c.error || (coord?.error ?? '');
  const lastTurn = c.turns.length ? c.turns[c.turns.length - 1] : null;
  const lastReply = lastTurn?.a || coord?.reply || '';
  const options = dialog || busy ? [] : parseOptions(lastReply);
  const open = lastTurn && (!lastTurn.a || lastTurn.doing) ? lastTurn : null;
  const scr = screens.value.coordinator;
  const menu: MenuData | null = dialog && !scr?.error ? (scr?.menu ?? null) : null;

  useEffect(() => { void loadLatest(PAGE); }, []);
  useEffect(() => onChatEvent(() => { void loadLatest(PAGE); }), []);
  useEffect(() => { if (on && dialog) void fetchScreen('coordinator'); }, [on, dialog, coord?.reply]);

  // the end stays in view unless the reader has scrolled up; older turns added above do not move what is read
  useLayoutEffect(() => {
    const el = logRef.current;
    if (!el) return;
    if (anchor.current) { el.scrollTop = anchor.current.top + (el.scrollHeight - anchor.current.h); anchor.current = null; }
    else if (stuck.current) el.scrollTop = el.scrollHeight;
  }, [c.turns, pending.map((p) => p.id).join(','), coord?.reply, busy, on, c.loading]);

  // what is on the screen of an open chat is read once it has been there for a moment
  const unread = unreadTurns.value;
  const unreadKey = unread.join(',');
  useEffect(() => {
    if (!on || !windowActive.value || !unread.length) return;
    const id = window.setTimeout(() => {
      markRead(unread.map((n) => [n, sigOf(chat.peek().turns.find((x) => x.n === n)?.a ?? '')] as [number, number]));
    }, SEE_MS);
    return () => window.clearTimeout(id);
  }, [on, windowActive.value, unreadKey]);

  const older = async (): Promise<void> => {
    const el = logRef.current;
    stuck.current = false;
    if (el) anchor.current = { h: el.scrollHeight, top: el.scrollTop };
    if (!(await loadOlder())) anchor.current = null;
  };
  const submit = async (value: string): Promise<void> => {
    const body = value.trim();
    if (!body || sending.value) return;
    stuck.current = true;
    if (await send(body)) {
      setText('');
      if (taRef.current) taRef.current.style.height = 'auto';
    }
  };
  const fit = (el: HTMLTextAreaElement): void => {
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 112)}px`;
  };

  return (
    <section class="page chatp" id="p-chat" aria-label={t('chat.aria')}>
      <div
        class="log" id="clog" ref={logRef} aria-live="polite"
        onScroll={(e) => { const el = e.currentTarget; stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40; }}
      >
        {c.available === false && <div class="ev">{t('chat.empty')}</div>}
        {c.turns.length > 0 && (hasOlder()
          ? <button class="linkbtn older" type="button" data-chat="older" onClick={() => void older()}>{c.loading ? t('chat.loading') : t('m.older')}</button>
          : <div class="day">{t('chat.start')}</div>)}
        {c.turns.map((x, i) => {
          const prev = i > 0 ? c.turns[i - 1] : null;
          return (
            <Fragment key={x.n}>
              {(!prev || dayKey(prev.at) !== dayKey(x.at)) && x.at > 0 && <Day at={x.at} />}
              {x.from === 'board' ? <Notice turn={x} /> : <Human text={x.q} at={x.at} turn={x} />}
              {x.a && <Answer turn={x} />}
            </Fragment>
          );
        })}
        {pending.map((p) => <Human key={p.id} text={p.text} at={p.at} pending />)}
        {err && <div class="b q err">{err}</div>}
        {busy && !err && <Working doing={open?.doing ?? ''} />}
      </div>
      {dialog && (
        <div class="dock">
          <MenuSlot
            target="coordinator" menu={menu} place="chat"
            onAnswered={() => { void fetchScreen('coordinator'); setTimeout(() => { void fetchScreen('coordinator'); }, 900); }}
            fallback={<pre class="perm">{scr?.error ? scr.error : scr?.text || t('chat.loading')}</pre>}
          />
        </div>
      )}
      {options.length > 0 && (
        <div class="quick" id="cvariants">
          {options.map((o, i) => <button type="button" key={i} data-variant={i} onClick={() => void submit(o.value)}>{o.label} {o.value}</button>)}
        </div>
      )}
      <div class="quick" id="cquick">
        {QUICK.map((k) => <button type="button" key={k} onClick={() => void submit(t(k))}>{t(k)}</button>)}
      </div>
      <form class="composer" id="ask" autocomplete="off" onSubmit={(e) => { e.preventDefault(); void submit(text); }}>
        <textarea
          class="in" id="ask-in" ref={taRef} rows={1} value={text} placeholder={t('m.placeholder')} aria-label={t('chat.aria')}
          onInput={(e) => { const el = e.currentTarget; setText(el.value); fit(el); }}
          onFocus={() => { stuck.current = true; }}
        />
        <button class="send" type="submit" id="send" aria-label={t('chat.send')} disabled={!text.trim() || sending.value}><Gl n="send" /></button>
      </form>
    </section>
  );
}
