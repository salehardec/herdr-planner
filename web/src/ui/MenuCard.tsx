/* A selection menu of an agent's program (Claude Code's question with options, Codex's approval, omp's ask) as a card
   with buttons. A button gives the choice: the server reads the screen again and presses the keys that make it.
   Several questions come one at a time (the screen shows one); a multiple choice has boxes and «Отправить»;
   «Свой ответ» opens a field. After an answer the card shows the choice and cannot be pressed again; it goes away
   when the screen has no menu any more (also when the answer was given in the terminal). */
import { signal } from '@preact/signals';
import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { answerMenu } from '../actions';
import { t } from '../i18n';
import type { MenuAnswer, MenuData, MenuOption } from '../types';
import { Gl } from './Gl';
import { AutoTextarea } from './AutoText';
import './menucard.css';

/** A description longer than this is folded to two lines with «Ещё». */
const LONG_DESC = 90;
/** The card unlocks again when the same menu is still on the screen this long after an answer (the answer did not land). */
const RELOCK_MS = 8000;

/** How long the answered card stays when the program has moved on at once (the next screen is no menu): long enough to be read. */
const GRACE_MS = 6000;

interface Props {
  target: string;
  role?: string;
  menu: MenuData;
  /** Where the card stands: the small chat, the wide chat or the window of an agent's screen. */
  place: 'chat' | 'window';
  /** Called after an answer, to read the screen again. */
  onAnswered?: () => void;
  /** Set when the card is shown again after the screen lost its menu: the answer already given. */
  answered?: { label: string; at: number };
  /** Called with the label of the choice once the answer has gone. */
  onDone?: (label: string) => void;
  /** The terminal's screen for the person who wants to see it (the fallback): shown under the card on demand. */
  screen?: ComponentChildren;
}

interface Done {
  sig: string;
  label: string;
  at: number;
}

/** A new question (or the next of several) is a new card: it starts again, whatever the last one had ticked or typed. */
export function MenuCard(props: Props): JSX.Element {
  return <MenuCardBody key={props.menu.sig} {...props} />;
}

function MenuCardBody(props: Props): JSX.Element {
  const { menu } = props;
  const [picked, setPicked] = useState<Set<number>>(() => new Set(menu.options.filter((o) => o.checked && !o.own).map((o) => o.at)));
  const [ownOpen, setOwnOpen] = useState(false);
  const [ownText, setOwnText] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState('');
  const [ctx, setCtx] = useState(false);
  const [screenOpen, setScreenOpen] = useState(false);
  const [openDesc, setOpenDesc] = useState<Set<number>>(new Set());
  const [done, setDone] = useState<Done | null>(() => (props.answered ? { sig: menu.sig, ...props.answered } : null));
  const [, relock] = useState(0);
  const ownRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!done) return;
    const id = window.setTimeout(() => relock((n) => n + 1), Math.max(0, done.at + RELOCK_MS - Date.now()) + 50);
    return () => window.clearTimeout(id);
  }, [done]);

  useEffect(() => { if (ownOpen) ownRef.current?.focus(); }, [ownOpen]);

  const locked = !!done && done.sig === menu.sig && Date.now() - done.at < RELOCK_MS;
  const stale = !!done && done.sig === menu.sig && !locked;
  const off = locked || busy;

  const send = async (answer: MenuAnswer, label: string): Promise<void> => {
    if (off) return;
    setBusy(true);
    setErr('');
    const r = await answerMenu(props.target, props.role ?? '', menu.sig, answer);
    setBusy(false);
    if (r.ok) { setDone({ sig: menu.sig, label, at: Date.now() }); props.onDone?.(label); }
    else setErr(r.gone ? t('amenu.gone') : t('amenu.failed', { why: r.error }));
    props.onAnswered?.();
  };

  const toggle = (at: number): void => {
    if (off) return;
    // from the latest state: two quick presses are both counted
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(at)) next.delete(at); else next.add(at);
      return next;
    });
  };

  const own = menu.options.find((o) => o.own);
  const text = ownText.trim();
  const sendOwn = (): void => {
    if (!text) return;
    if (menu.multi) void sendMulti();
    else void send({ kind: 'own', text }, text);
  };
  const sendMulti = async (): Promise<void> => {
    const at = [...picked].sort((a, b) => a - b);
    const labels = at.map((a) => menu.options.find((o) => o.at === a)?.label ?? '').filter(Boolean);
    if (ownOpen && text) labels.push(text);
    await send({ kind: 'multi', picked: at, own: ownOpen && text ? text : undefined }, labels.join(', '));
  };
  const canSend = menu.multi ? picked.size > 0 || (ownOpen && !!text) : false;

  const row = (o: MenuOption, i: number): JSX.Element => {
    const long = o.desc.length > LONG_DESC;
    const open = openDesc.has(o.at);
    const chosen = locked && done?.label === o.label;
    if (o.own) {
      return (
        <div class="menu-row" key={o.at}>
          <button
            class={`menu-opt own ${ownOpen ? 'on' : ''} ${menu.multi ? 'check' : ''}`} type="button" data-menu-opt={o.at} data-own="1"
            role={menu.multi ? 'checkbox' : undefined} aria-checked={menu.multi ? ownOpen : undefined} aria-expanded={ownOpen} disabled={off}
            onClick={() => setOwnOpen(!ownOpen)}
          >
            {menu.multi ? <span class="mo-box" aria-hidden="true">{ownOpen ? '✓' : ''}</span> : <span class="mo-key" aria-hidden="true">{i + 1}</span>}
            <span class="mo-body"><span class="mo-l">{t('amenu.own')}</span></span>
          </button>
        </div>
      );
    }
    const label = (
      <span class="mo-body">
        <span class="mo-l">{o.label}</span>
        {o.desc && <span class={`mo-d ${long && !open ? 'clamp' : ''}`}>{o.desc}</span>}
      </span>
    );
    return (
      <div class="menu-row" key={o.at}>
        {menu.multi ? (
          <button
            class={`menu-opt check ${picked.has(o.at) ? 'on' : ''}`} type="button" role="checkbox" aria-checked={picked.has(o.at)}
            data-menu-opt={o.at} disabled={off} onClick={() => toggle(o.at)}
          >
            <span class="mo-box" aria-hidden="true">{picked.has(o.at) ? '✓' : ''}</span>{label}
          </button>
        ) : (
          <button
            class={`menu-opt ${chosen ? 'chosen' : ''}`} type="button" data-menu-opt={o.at} disabled={off}
            onClick={() => void send({ kind: 'pick', at: o.at }, o.label)}
          >
            <span class="mo-key" aria-hidden="true">{i + 1}</span>{label}
          </button>
        )}
        {long && (
          <button
            class="more sm" type="button" data-menu="more" data-menu-more={o.at}
            onClick={() => setOpenDesc((prev) => { const s = new Set(prev); if (s.has(o.at)) s.delete(o.at); else s.add(o.at); return s; })}
          >{open ? t('amenu.less') : t('amenu.more')}</button>
        )}
      </div>
    );
  };

  return (
    <div
      class={`menu-card ${props.place}`} id={`menu-card-${props.target.replace(/[^\w-]/g, '_')}`} data-menu-card="1"
      data-state={locked ? 'answered' : busy ? 'busy' : 'open'} data-sig={menu.sig} data-program={menu.program} data-multi={String(menu.multi)}
    >
      <div class="menu-head">
        <span class="menu-kicker"><Gl n="waiting" />{t('amenu.title')}</span>
        {menu.steps > 0 && <span class="menu-step" data-menu-step>{t('amenu.step', { n: menu.step, m: menu.steps })}</span>}
      </div>
      <div class="menu-q" data-menu-question>{menu.question}</div>
      {menu.context && (
        <>
          <button class="more sm menu-ctx-btn" type="button" data-menu="context" aria-expanded={ctx} onClick={() => setCtx(!ctx)}>
            {ctx ? t('amenu.contextLess') : t('amenu.context')}
          </button>
          {ctx && <pre class="menu-ctx" data-menu-context>{menu.context}</pre>}
        </>
      )}
      {menu.multi && <div class="menu-hint">{t('amenu.multiHint')}</div>}
      <div class="menu-opts" role={menu.multi ? 'group' : 'radiogroup'} aria-label={menu.question}>
        {menu.options.map((o, i) => row(o, i))}
      </div>
      {own && ownOpen && (
        <div class="menu-own">
          <AutoTextarea
            id="menu-own-in" class="field" elRef={ownRef} autocomplete="off" value={ownText} disabled={off}
            placeholder={t('amenu.ownPlaceholder')} aria-label={t('amenu.own')}
            onInput={(e) => setOwnText((e.target as HTMLTextAreaElement).value)}
            onSend={sendOwn}
          />
          {!menu.multi && (
            <button class="btn primary sm" type="button" data-menu="send-own" disabled={off || !text} onClick={sendOwn}>{t('amenu.send')}</button>
          )}
          <div class="keyhint">{t('text.keys.send')}</div>
        </div>
      )}
      {menu.multi && (
        <div class="menu-actions">
          <button class="btn primary sm" type="button" data-menu="send" disabled={off || !canSend} onClick={() => void sendMulti()}>{t('amenu.send')}</button>
        </div>
      )}
      <div class="menu-foot">
        <span class="menu-status" aria-live="polite">
          {busy ? <span class="hint" data-menu-status="busy">{t('amenu.sending')}</span>
            : locked ? <span class="menu-done" data-menu-status="done"><Gl n="done" />{menu.multi || !done?.label ? t('amenu.answeredMulti') : t('amenu.answered', { a: done.label })}</span>
            : stale ? <span class="hint" data-menu-status="stale">{t('amenu.stillThere')}</span> : null}
          {err && <span class="hint bad" data-menu-status="error">{err}</span>}
        </span>
        <button
          class="btn sm" type="button" data-menu="cancel" title={t('amenu.cancelTip')} disabled={off}
          onClick={() => void send({ kind: 'cancel' }, '')}
        >{t('amenu.cancel')}</button>
        {props.screen !== undefined && (
          <button class="btn sm" type="button" data-menu="screen" aria-expanded={screenOpen} onClick={() => setScreenOpen(!screenOpen)}>
            {screenOpen ? t('amenu.screenHide') : t('amenu.screen')}
          </button>
        )}
      </div>
      {screenOpen && props.screen}
    </div>
  );
}

/** What was answered through a card, by agent: the card goes on showing the choice for a while after the program has drawn
    its next screen (which is usually no menu), instead of vanishing into the raw screen at once. */
const answeredCards = signal<Record<string, { menu: MenuData; label: string; at: number }>>({});

interface SlotProps extends Omit<Props, 'menu' | 'answered' | 'onDone'> {
  /** The menu the screen shows now; null when there is none. */
  menu: MenuData | null;
  /** What to show when there is no menu and no answered card to keep: the raw dialog, or nothing. */
  fallback?: ComponentChildren;
}

/** The place of an agent's menu card: the menu on the screen, else the card just answered (locked, with the choice), else `fallback`. */
export function MenuSlot(props: SlotProps): JSX.Element | null {
  const key = `${props.target}|${props.role ?? ''}`;
  const rec = answeredCards.value[key];
  const [, tick] = useState(0);
  const fresh = !!rec && Date.now() - rec.at < GRACE_MS;
  useEffect(() => {
    if (!rec) return;
    const id = window.setTimeout(() => tick((n) => n + 1), Math.max(0, rec.at + GRACE_MS - Date.now()) + 50);
    return () => window.clearTimeout(id);
  }, [rec]);
  const { menu, fallback, ...rest } = props;
  const shown = menu ?? (fresh ? rec.menu : null);
  if (!shown) return <>{fallback ?? null}</>;
  const mine = rec && rec.menu.sig === shown.sig && fresh ? { label: rec.label, at: rec.at } : undefined;
  return (
    <MenuCard
      {...rest} menu={shown} answered={mine}
      onDone={(label) => { answeredCards.value = { ...answeredCards.value, [key]: { menu: shown, label, at: Date.now() } }; }}
    />
  );
}
