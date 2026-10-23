/* One turn of the Coordinator's chat: the human's message (a bubble on the right, with its files), or a board
   notice (its own kind: the task, its name and its state), then the Coordinator's answer (Markdown, the whole of
   it) with the board events it ended with as event lines. Actions show on hover and on focus. */
import type { JSX } from 'preact';
import { t } from '../i18n';
import { toast } from '../toast';
import { draftRequest } from '../chat';
import { sigOf } from '../chat-read';
import { eventText, noticeStatus, parseNotice, splitAttachments, splitEvents, type BoardEvent } from '../chat-parse';
import { timeOf } from '../util/dates';
import type { TurnData } from '../types';
import type { SavedFile } from '../attach';
import { Gl } from './Gl';
import { Markdown } from './Markdown';
import { RefText, TaskChip, taskOf } from './TaskChip';
import { SavedFiles } from './Attached';
import { openRemember } from './memory-api';

const copyText = (text: string): void => { navigator.clipboard?.writeText(text).catch(() => {}); toast(t('toast.copied')); };
const quote = (text: string): void => {
  const lines = text.trim().split('\n').map((l) => `> ${l}`).join('\n');
  draftRequest.value = { text: `${lines}\n\n`, at: Date.now() };
};

function Act(props: { n: string; label: string; onClick: () => void; attrs?: Record<string, string | number> }): JSX.Element {
  return (
    <button class="act" type="button" title={props.label} aria-label={props.label} {...props.attrs} onClick={(e) => { e.stopPropagation(); props.onClick(); }}>
      <Gl n={props.n} />{props.label}
    </button>
  );
}

/** The hover bar of a message: copy, quote into the line, to memory; the human's own message can be repeated. */
function Acts(props: { text: string; who: 'q' | 'a'; onReuse?: () => void; n: number }): JSX.Element {
  return (
    <div class="acts">
      <Act n="copy" label={t('chat.act.copy')} onClick={() => copyText(props.text)} attrs={{ 'data-act': 'copy' }} />
      <Act n="quote" label={t('chat.act.quote')} onClick={() => quote(props.text)} attrs={{ 'data-act': 'quote' }} />
      <Act n="memory" label={t('mem.toMemory')} onClick={() => openRemember({ text: props.text, source: 'chat' })} attrs={{ 'data-act': 'to-memory', 'data-who': props.who }} />
      {props.onReuse && <Act n="redo" label={t('chat.act.repeat')} onClick={props.onReuse} attrs={{ 'data-reuse': props.n }} />}
    </div>
  );
}

function Unread(props: { on: boolean }): JSX.Element {
  return <i class={`udot ${props.on ? 'on' : ''}`} role={props.on ? 'img' : undefined} aria-label={props.on ? t('chat.unreadDot') : undefined} title={props.on ? t('chat.unreadDot') : undefined} />;
}

export function EventLine(props: { ev: BoardEvent }): JSX.Element {
  return <div class="cev" data-ev={props.ev.verb}><Gl n="event" /><RefText text={eventText(props.ev)} /></div>;
}

/** A message of the human (not a notice): the words, the files, the hover actions. */
/** The images Claude Code kept inside the transcript record, served by the planner from it. */
const turnImages = (tn: TurnData): SavedFile[] =>
  (tn.images ?? []).map((mime, i) => ({ id: `turn-${tn.n}-${i}`, name: `${t('chat.image')} ${i + 1}`, mime, url: `/api/chat/image/${tn.n}/${i}` }));

function Human(props: { turn: TurnData; unread: boolean; onReuse: (q: string) => void; pending?: boolean; files?: SavedFile[] }): JSX.Element {
  const tn = props.turn;
  const parsed = splitAttachments(tn.q);
  const { text } = parsed;
  const files = [...parsed.files, ...turnImages(tn), ...(props.files ?? [])];
  // a picture pasted into the Coordinator's own window has a mark and no block: only then the plate
  const pasted = files.length ? 0 : parsed.pasted;
  return (
    <div class={`msg q ${props.pending ? 'pending' : ''}`} tabIndex={0} data-unread={props.unread && !tn.a ? tn.n : undefined} data-sig={0}>
      <span class="who"><Gl n="user" />{t('chat.you')}</span>
      <span class="time">{props.pending ? t('chat.sentWaiting') : timeOf(tn.at)}</span>
      <div class="txt"><RefText text={text} /></div>
      {(files.length > 0 || pasted > 0) && (
        <div class="shots">
          <SavedFiles list={files} zoom />
          {pasted > 0 && <span class="pasted" title={t('chat.pastedImageTip')}><Gl n="image" />{t('chat.pastedImage')}{pasted > 1 ? ` · ${pasted}` : ''}</span>}
        </div>
      )}
      {!props.pending && <Acts text={text} who="q" n={tn.n} onReuse={() => props.onReuse(text)} />}
    </div>
  );
}

/** A notice the board sent the Coordinator: the task as a chip, its name, its state as a badge. */
function Notice(props: { turn: TurnData; unread: boolean; compact: boolean; parsed: ReturnType<typeof parseNotice> }): JSX.Element {
  const tn = props.turn;
  const n = props.parsed;
  const task = n ? taskOf(n.id) : undefined;
  const status = n ? (noticeStatus(n) || task?.status || '') : '';
  const stateText = n ? (n.stateRu || (task ? (task.label || t(`status.${task.status}`)) : '')) : '';
  return (
    <div class={`msg notice ${props.compact ? 'compact' : ''}`} tabIndex={0} data-unread={props.unread && !tn.a ? tn.n : undefined} data-sig={0}>
      <span class="who"><Gl n="board" />{t('chat.board')}<Unread on={props.unread && !tn.a} /></span>
      <span class="time">{timeOf(tn.at)}</span>
      {n ? (
        <div class="ntl">
          <TaskChip tag={n.id} />
          {(n.title || task?.title) && <span class="ttl">«{n.title || task?.title}»</span>}
          {stateText && <span class={`badge s-${status || 'queued'}`}>{status && <Gl n={status} />}{stateText}</span>}
        </div>
      ) : <div class="txt"><RefText text={tn.q} /></div>}
      {n?.note && <div class="txt note"><RefText text={n.note} /></div>}
      <Acts text={tn.q} who="q" n={tn.n} />
    </div>
  );
}

/** The Coordinator's answer: Markdown with chips, then the board events it ended with. */
function Answer(props: { turn: TurnData; unread: boolean }): JSX.Element {
  const tn = props.turn;
  const { body, events } = splitEvents(tn.a);
  return (
    <div class={`msg a ${body ? '' : 'evonly'}`} tabIndex={0} data-unread={props.unread ? tn.n : undefined} data-sig={sigOf(tn.a)}>
      <span class="who"><Gl n="quasar" />{t('quasar.short')}<Unread on={props.unread} /></span>
      <span class="time">{timeOf(tn.at_a || tn.at)}</span>
      {body && <Markdown class="txt" text={body} chips copy />}
      {events.length > 0 && <div class="cevs">{events.map((ev, i) => <EventLine key={i} ev={ev} />)}</div>}
      <Acts text={tn.a} who="a" n={tn.n} />
    </div>
  );
}

/**
 * One turn. `compact`: this notice follows another unanswered notice within a few minutes, so its label and
 * time are not repeated.
 */
export function ChatTurn(props: { turn: TurnData; hit: boolean; unread: boolean; compact: boolean; onReuse: (q: string) => void }): JSX.Element {
  const tn = props.turn;
  const notice = tn.from === 'board' ? parseNotice(tn.q) : null;
  return (
    <div class={`turn ${tn.from} ${props.hit ? 'hit' : ''}`} data-n={tn.n}>
      {tn.from === 'board'
        ? <Notice turn={tn} unread={props.unread} compact={props.compact} parsed={notice} />
        : <Human turn={tn} unread={props.unread} onReuse={props.onReuse} />}
      {tn.a && <Answer turn={tn} unread={props.unread} />}
    </div>
  );
}

/** The message just sent, before the transcript has it. */
export function PendingTurn(props: { text: string; attachments: { id: string; name: string }[] }): JSX.Element {
  const turn: TurnData = { n: -1, at: 0, from: 'human', q: props.text, a: '' };
  return (
    <div class="turn pending">
      <Human turn={turn} unread={false} onReuse={() => {}} pending files={props.attachments} />
    </div>
  );
}
