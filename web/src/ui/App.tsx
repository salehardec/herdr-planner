import type { JSX } from 'preact';
import { mode, panels, simple, linkMode } from '../store';
import { Top } from './Top';
import { Navigator } from './Navigator';
import { Focus } from './Focus';
import { AgentSummary } from './AgentSummary';
import { Chat } from './Chat';
import { Todo } from './Todo';
import { Wait } from './Wait';
import { Log } from './Log';
import { Kanban } from './Kanban';
import { LimitsPop } from './Limits';
import { Help } from './Help';
import { Toasts } from './Toasts';
import { Banner, FlightHud, HerdrPop, HintCard, LinkNotice, NoGl } from './Overlays';
import { Dialogs } from './Dialogs';
import { Cockpit } from './Cockpit';
import { Station, StationAuto } from './Station';
import { Ship } from './Ship';
import { RememberDialog } from './MemoryPanel';
import { Arrival, StationChip } from './Arrival';
import { Gl } from './Gl';
import { t } from '../i18n';
import { unreadCount } from './unread';
import './help.css';
import './chatwide.css';

function Dock(): JSX.Element {
  return (
    <div class="kbar" id="kbar">
      <button class={`btn ${panels.todo.value ? 'on' : ''}`} id="btodo" type="button" aria-pressed={panels.todo.value} onClick={() => { panels.todo.value = !panels.todo.value; }}>
        <Gl n="list" />{t('todo.button', { n: unreadCount() })}
      </button>
      <button class={`btn ${panels.chat.value ? 'on' : ''}`} id="bchat" type="button" aria-pressed={panels.chat.value} onClick={() => { panels.chat.value = !panels.chat.value; if (panels.chat.value) setTimeout(() => (document.getElementById('ask-in') as HTMLTextAreaElement | null)?.focus(), 30); }}>
        <Gl n="chat" />{t('chat.button')} <kbd>/</kbd>
      </button>
    </div>
  );
}

export function App(): JSX.Element {
  const classes = ['win-ui', mode.value === 'manual' ? 'manual' : '', simple.value ? 'calm' : '', linkMode.value ? 'linking' : ''].join(' ');
  void classes;
  return (
    <>
      <Cockpit />
      <Top />
      <Navigator />
      <Wait />
      <Log />
      <Focus />
      <AgentSummary />
      <Todo />
      <Chat />
      <Kanban />
      <Dock />
      <FlightHud />
      <LinkNotice />
      <HintCard />
      <LimitsPop />
      <HerdrPop />
      <Help />
      <Banner />
      <NoGl />
      <Dialogs />
      <Station />
      <Ship />
      <RememberDialog />
      <Arrival />
      <StationAuto />
      <Toasts />
    </>
  );
}
