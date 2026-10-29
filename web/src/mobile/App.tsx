/* The light page: a header with the limits and the settings, three tabs (what waits for me, the chat, the board) and the sheets. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { live, requests, snap } from '../store';
import { unreadCount } from '../chat-unread';
import { dismissToast, toasts } from '../toast';
import { t } from '../i18n';
import { Gl } from '../ui/Gl';
import { worstLimit } from '../ui/Limits';
import { listed } from './cards';
import { chatShown, listTab, session, sheet, tab, wide, type Tab } from './state';
import { WaitPage } from './Wait';
import { ChatPage } from './Chat';
import { BoardPage } from './Board';
import { Restroom } from '../ui/Restroom';
import { enter } from '../restroom/state';
import { Sheets } from './Sheets';

function since(ms: number): string {
  const total = Math.max(0, Math.floor((Date.now() - ms) / 60000));
  const h = Math.floor(total / 60);
  return h > 0 ? t('chat.elapsedHm', { h, m: total % 60 }) : t('dur.min', { n: Math.max(1, total) });
}

function Header(): JSX.Element {
  const [, tick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 30000);
    return () => window.clearInterval(id);
  }, []);
  const s = snap.value;
  const ses = session.value;
  const worst = worstLimit();
  const left = worst ? Math.max(0, 100 - Math.round(worst.window.percent * 100)) : null;
  const line = ses?.kind === 'device' && ses.gate_since_ms ? t('m.gateOpen', { time: since(ses.gate_since_ms) }) : t('m.local');
  const level = worst?.window.severity === 'crit' ? 'crit' : worst?.window.severity === 'warn' ? 'warn' : '';
  return (
    <header class="top">
      <div class="brand"><b>{t('m.brand')}</b><span id="hostline">{s?.host || ses?.computer || ''} · {line}</span></div>
      <button class={`chip ${level}`} type="button" id="limchip" aria-label={t('limits.title')} onClick={() => { sheet.value = { kind: 'limits' }; }}>
        <Gl n="limit" />{left === null ? <span>{t('limits.chip')}</span> : <><b>{left}%</b> {worst?.agent.name}</>}
      </button>
      <button class="ibtn" type="button" id="menu" aria-label={t('m.settings')} onClick={() => { sheet.value = { kind: 'settings' }; }}>
        <svg class="gl" aria-hidden="true" viewBox="0 0 16 16"><circle cx="3.5" cy="8" r=".9" /><circle cx="8" cy="8" r=".9" /><circle cx="12.5" cy="8" r=".9" /></svg>
      </button>
    </header>
  );
}

/** A computer that lands on the light page only because the full version is closed to it: the page says so. */
function FullNote(): JSX.Element | null {
  const s = session.value;
  if (s?.kind !== 'device' || s.form !== 'desktop' || s.view === 'lite') return null;
  if (s.full !== 'closed' && s.full !== 'nopassword') return null;
  return <div class="banner" id="fullnote" role="note">{t(`m.full.${s.full}`)}</div>;
}

function Nav(): JSX.Element {
  const waiting = listed(requests.value).length;
  const unread = unreadCount.value;
  const go = (to: Tab): void => { tab.value = to; };
  const all: [Tab, string, string][] = [['wait', 'waiting', t('wait.title')], ['chat', 'chat', t('m.tab.chat')], ['board', 'board', t('m.tab.board')]];
  const items = wide.value ? all.filter(([k]) => k !== 'chat') : all;
  const here = listTab.value;
  return (
    <nav class="nav" aria-label={t('m.sections')}>
      {items.map(([k, icon, label]) => (
        <button key={k} type="button" data-tab={k} class={here === k ? 'on' : ''} aria-current={here === k ? 'page' : undefined} onClick={() => go(k)}>
          <Gl n={icon} />{label}
          {k === 'wait' && waiting > 0 && <span class="n" id="nw">{waiting}</span>}
          {k === 'chat' && unread > 0 && tab.value !== 'chat' && <span class="u" id="nu" role="img" aria-label={t('chat.unreadDot')} />}
        </button>
      ))}
      <button id="m-rest" type="button" onClick={(e) => enter({ kind: 'mobile' }, e.currentTarget)}>
        <span class="rest-icon" aria-hidden="true">☕</span>{t('restroom.top')}
      </button>
    </nav>
  );
}

export function App(): JSX.Element {
  const ready = snap.value !== null;
  return (
    <>
    <div class="mob" data-ready={ready ? '1' : '0'}>
      <Header />
      {!live.value && ready && <div class="banner" id="offline" role="status">{t('m.offline')}</div>}
      <FullNote />
      {ready ? (
        <div class="pages" data-tab={listTab.value}>
          <div class={`pane ${listTab.value === 'wait' ? 'on' : ''}`}><WaitPage /></div>
          <div class={`pane chatpane ${chatShown.value ? 'on' : ''}`}><ChatPage /></div>
          <div class={`pane ${listTab.value === 'board' ? 'on' : ''}`}><BoardPage /></div>
        </div>
      ) : (
        <div class="pages"><div class="empty" id="loading"><b>{t('m.loading')}</b>{!live.value ? t('m.offline') : ''}</div></div>
      )}
      <Nav />
      <Sheets />
      <div class="toasts" id="toasts" role="status">
        {toasts.value.map((x) => (
          <div class="toast" key={x.id}>
            <Gl n={x.icon} /><span>{x.msg}</span>
            {x.action && <button class="undo" type="button" onClick={() => { dismissToast(x.id); x.action?.(); }}>{x.label}</button>}
          </div>
        ))}
      </div>
    </div>
    <Restroom mobile />
    </>
  );
}
