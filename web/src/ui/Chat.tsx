import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import {
  chat, galaxies, galaxyId, panels, snap, screens,
} from '../store';
import {
  addFiles, hasOlder, loadLatest, loadOlder, notePending, openChatAt, optimistic, pendingImages, pendingMessage, reconcile, removeImage, scrollTo,
  searchChat, send, sending,
} from '../chat';
import { parseOptions } from '../requests';
import { post, imgUrl, refreshNow, onChatEvent } from '../api';
import { fetchScreen, sendKey } from '../actions';
import { t } from '../i18n';
import { dayTimeOf, timeOf } from '../util/dates';
import { Gl } from './Gl';
import { RichText } from './RichText';
import { Memory } from './FocusOther';
import type { TurnData } from '../types';
import './chat.css';

const wide = signal<Set<number>>(new Set());
const stuck = signal(true);

function Shots(props: { list: { id: string; name: string }[] }): JSX.Element | null {
  const items = props.list.filter((a) => a && a.id);
  if (!items.length) return null;
  return (
    <div class="shots">
      {items.map((a) => (
        <a class="shot" href={imgUrl(a)} target="_blank" rel="noopener" key={a.id}>
          <img src={imgUrl(a)} alt={a.name || t('chat.image')} title={a.name} loading="lazy" />
        </a>
      ))}
    </div>
  );
}

function Turn(props: { turn: TurnData; hit: boolean; onReuse: (q: string) => void }): JSX.Element {
  const tn = props.turn;
  const long = tn.a.length > 520 || tn.a.split('\n').length > 11;
  const open = wide.value.has(tn.n);
  return (
    <div class={`turn ${tn.from} ${props.hit ? 'hit' : ''}`} data-n={tn.n}>
      <div class="q" data-reuse={tn.n} title={t('chat.reuse')} onClick={() => props.onReuse(tn.q)}>
        <span class="who">{t('chat.you')}</span>
        <span class="txt">{tn.q}</span>
      </div>
      <div class="meta"><span>{dayTimeOf(tn.at)}</span>{tn.from === 'board' && <span>· {t('chat.boardNotice')}</span>}</div>
      {tn.a && (
        <>
          <div class="a">
            <span class="who">{t('quasar.short')}</span>
            <RichText class={`txt ${long && !open ? 'clamp' : ''}`} text={tn.a} />
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

export function Chat(): JSX.Element {
  const c = chat.value;
  const open = panels.chat.value;
  const s = snap.value;
  const coord = s?.coordinator;
  const far = galaxyId.value !== '';
  const farG = far ? galaxies.value.find((g) => g.label === galaxyId.value) : null;
  const logRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const anchor = useRef<{ h: number; top: number } | null>(null);
  const showTurns = !far && c.available === true && c.turns.length > 0;
  const pending = pendingMessage();
  const opt = optimistic.value;
  const history = coord?.history ?? [];
  const reply = far ? (farG?.coordinator.reply ?? '') : (coord?.reply ?? '');
  const err = c.error || (coord?.error ?? '');
  const busy = !!coord?.busy && !far;
  const dialog = !!coord?.dialog && !far;
  // The Coordinator's last words: its transcript's, or (while that answer is not written yet) the tail of its window.
  const lastReply = (showTurns ? c.turns[c.turns.length - 1].a : '') || reply;
  const options = dialog || busy ? [] : parseOptions(lastReply);
  const scr = screens.value.coordinator;

  // Load the transcript when the chat opens and when it grows.
  useEffect(() => { if (open && !far) void loadLatest(); }, [open, far]);
  useEffect(() => onChatEvent(() => { if (panels.chat.value) void loadLatest(); }), []);
  useEffect(() => { if (open && dialog) void fetchScreen('coordinator'); }, [open, dialog, coord?.reply]);
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
      if (el) { stuck.value = false; el.scrollIntoView({ block: 'center', behavior: 'smooth' }); }
    });
  }, [scrollTo.value]);

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

  const submit = async (e: Event): Promise<void> => {
    e.preventDefault();
    const ta = taRef.current;
    if (!ta) return;
    stuck.value = true;
    const text = ta.value;
    const ok = await send(text);
    if (ok) { ta.value = ''; ta.style.height = '36px'; }
  };

  const onKey = (e: KeyboardEvent): void => {
    const ta = e.target as HTMLTextAreaElement;
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); void submit(e); return; }
    if (e.key === 'Escape') { ta.value = ''; ta.blur(); panels.chat.value = false; e.stopPropagation(); return; }
    const log = logRef.current;
    if (!log || !showTurns) return;
    const caret = ta.selectionStart;
    const firstRow = !ta.value.slice(0, caret).includes('\n');
    const lastRow = !ta.value.slice(ta.selectionEnd).includes('\n');
    const step = e.key === 'PageUp' || e.key === 'PageDown' ? log.clientHeight * 0.85 : 70;
    if ((e.key === 'ArrowUp' && firstRow) || e.key === 'PageUp') {
      e.preventDefault();
      stuck.value = false;
      if (log.scrollTop <= 1) void older(); else log.scrollTop -= step;
    } else if ((e.key === 'ArrowDown' && lastRow) || e.key === 'PageDown') {
      e.preventDefault();
      log.scrollTop += step;
    }
  };

  const grow = (e: Event): void => {
    const ta = e.target as HTMLTextAreaElement;
    ta.style.height = '36px';
    ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
  };

  const onPaste = (e: ClipboardEvent): void => {
    const files = [...(e.clipboardData?.items ?? [])]
      .filter((i) => i.kind === 'file' && i.type.startsWith('image/'))
      .map((i) => i.getAsFile())
      .filter((f): f is File => !!f);
    if (!files.length) return;
    e.preventDefault();
    addFiles(files);
  };

  const status = err ? t('chat.state.error') : dialog ? t('chat.state.dialog') : busy ? t('chat.state.busy') : (coord?.status || t('chat.state.ready'));
  const typed = (taRef.current?.value ?? '').trim().length > 0 || pendingImages.value.length > 0;
  const hits = c.hits;

  const variantSend = (value: string): void => {
    notePending(value);
    void post('/api/chat', { text: value, galaxy: galaxyId.value || undefined }).catch((er: Error) => {
      chat.value = { ...chat.value, pending: null, error: er.message };
    });
    setTimeout(() => { void refreshNow(); void loadLatest(); }, 900);
  };

  const dropProps = {
    onDragOver: (e: DragEvent) => { if (e.dataTransfer?.types.includes('Files')) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; } },
    onDrop: (e: DragEvent) => { if (e.dataTransfer?.files?.length) { e.preventDefault(); addFiles(e.dataTransfer.files); } },
  };

  return (
    <section class={`panel chat ${open ? '' : 'off'}`} id="chat" aria-label={t('chat.aria')} aria-hidden={!open} {...dropProps}>
      <header>
        <b>{far ? `${galaxyId.value} · ${t('chat.coordinator')}` : t('quasar.label')}</b>
        <span class={`st ${err ? 'bad' : busy ? 'busy' : ''}`} id="cstate">● {status}</span>
        <input
          id="chat-q" class="cq" type="search" value={c.q} placeholder={t('chat.search')} aria-label={t('chat.search')}
          onInput={(e) => searchChat((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Escape') { searchChat(''); (e.target as HTMLInputElement).blur(); e.stopPropagation(); } }}
        />
        <button class="btn sm" id="chide" type="button" title="/" onClick={() => { panels.chat.value = false; }}>{t('chat.hide')}</button>
      </header>
      {!far && <div class="cmem"><Memory compact /></div>}
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
              <Turn key={x.n} turn={x} hit={c.highlight === x.n} onReuse={(q) => { if (taRef.current) { taRef.current.value = q; taRef.current.focus(); grow({ target: taRef.current } as unknown as Event); } }} />
            ))}
          </>
        ) : (
          <Fallback
            far={far} history={history} reply={reply} talk={farG?.coordinator.talk ?? []} status={coord?.status ?? ''}
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
      {dialog && (
        <div class="dialog" id="cdialog">
          <div class="h">{t('chat.dialogTitle')}</div>
          {scr?.error ? <div class="hint">{scr.error}</div> : <pre>{scr?.text || (scr?.busy ? t('chat.reading') : '')}</pre>}
          <div class="keys">
            {['up', 'down', 'left', 'right', 'enter', 'esc', '1', '2', '3', 'y', 'n'].map((k) => (
              <button class="btn sm" type="button" key={k} data-act="keys" data-target="coordinator" data-keys={k} onClick={() => void sendKey('coordinator', k)}>
                {k === 'enter' ? '↵ Enter' : k === 'esc' ? 'Esc' : k}
              </button>
            ))}
          </div>
          {!s?.herdr.running && <div class="hint">{t('chat.dialogNoHerdr')}</div>}
        </div>
      )}
      {options.length > 0 && (
        <div class="variants" id="cvariants">
          {options.map((o, i) => (
            <button class="btn sm" type="button" key={i} data-variant={i} onClick={() => variantSend(o.value)}>{o.label} {o.value}</button>
          ))}
        </div>
      )}
      {pendingImages.value.length > 0 && (
        <div class="attach" id="attach">
          {pendingImages.value.map((p) => (
            <div class="att" data-state={p.state} key={p.key} title={p.error ? `${p.name} — ${p.error}` : p.name}>
              <img src={p.local} alt={p.name} />
              {p.state === 'uploading' && <span class="wait">{t('chat.uploading')}</span>}
              <button class="rm" type="button" data-att-rm={p.key} aria-label={`${t('chat.remove')} ${p.name}`} onClick={() => removeImage(p.key)}>×</button>
            </div>
          ))}
        </div>
      )}
      <form class="compose" id="ask" autocomplete="off" onSubmit={(e) => void submit(e)}>
        <textarea
          id="ask-in" ref={taRef} rows={1} placeholder={t('chat.placeholder')} aria-label={t('chat.aria')}
          onInput={grow} onKeyDown={onKey} onPaste={onPaste} onFocus={() => { panels.chat.value = true; }}
        />
        <label class="attachbtn" title={t('chat.attachTitle')}>
          <Gl n="attach" />
          <input type="file" id="file-in" accept="image/png,image/jpeg,image/webp,image/gif" multiple hidden onChange={(e) => { addFiles((e.target as HTMLInputElement).files); (e.target as HTMLInputElement).value = ''; }} />
        </label>
        <button class={`btn primary send`} type="submit" id="send" data-ready={String(typed)} disabled={sending.value} aria-label={t('chat.send')}>{t('chat.send')}</button>
      </form>
    </section>
  );
}

function Fallback(props: {
  far: boolean; history: { text: string; at: number; attachments: { id: string; name: string }[] }[]; reply: string;
  talk: { you: boolean; text: string }[]; status: string; sentAt: number; sent: string;
}): JSX.Element {
  const lines = props.far ? props.talk : [];
  return (
    <>
      {props.far && <div class="meta start">{t('chat.farNoTranscript')}</div>}
      {!props.far && !props.history.length && props.sent && (
        <div class="q"><span class="who">{t('chat.you')}</span><span class="txt">{props.sent}</span></div>
      )}
      {props.history.map((m) => (
        <div class="turn" key={m.at}>
          <div class="q"><span class="who">{t('chat.you')}</span><span class="txt">{m.text || t('chat.images')}</span></div>
          <Shots list={m.attachments} />
          <div class="meta"><span>{timeOf(m.at)}</span><span>· {t('chat.sentToQuasar')}</span></div>
        </div>
      ))}
      {lines.map((l, i) => (
        <div class={l.you ? 'q' : 'a'} key={i}><span class="who">{l.you ? t('chat.you') : t('quasar.short')}</span><span class="txt">{l.text}</span></div>
      ))}
      {props.reply && !props.far && (
        <>
          <div class="a"><span class="who">{t('quasar.short')}</span><RichText class="txt" text={props.reply} /></div>
          <div class="meta"><span>{t('chat.lastFromWindow')}</span><span>· {props.status}</span></div>
        </>
      )}
      {props.far && !lines.length && props.reply && (
        <div class="a"><span class="who">{t('quasar.short')}</span><RichText class="txt" text={props.reply} /></div>
      )}
      {!props.reply && !props.history.length && !lines.length && !props.sent && (
        <div class="a empty"><span class="who">{t('quasar.short')}</span><span class="txt">{t('chat.empty')}</span></div>
      )}
    </>
  );
}
