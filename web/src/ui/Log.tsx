import type { JSX } from 'preact';
import { currentView, panels, snap } from '../store';
import { t } from '../i18n';
import { timeOf } from '../util/dates';
import { Gl } from './Gl';
import { RichText } from './RichText';
import { navigateTo } from '../nav';
import './log.css';

export function Log(): JSX.Element {
  const open = panels.log.value;
  const items = (snap.value?.journal ?? []).slice(0, 60);
  const tagOf = (id: string): { tag: string; ok: boolean; runner?: string } => {
    if (id.startsWith('runner/')) return { tag: t('runner.taskTitle'), ok: true, runner: id.slice('runner/'.length) };
    // The shipyards' alarms: a line of their own, about the yards as a group.
    // The switches of the Coordinator, the planner and the light tasks to a spare.
    if (id === 'crew') return { tag: t('st.crew'), ok: false };
    if (id === 'runner') return { tag: t('nav.runners'), ok: false };
    const tk = currentView.value.tasks.find((x) => x.id === id) ?? snap.value?.archived.find((x) => x.id === id);
    return { tag: tk?.tag ?? id, ok: !!tk };
  };
  return (
    <section class={`panel journal ${open ? 'open' : ''}`} id="drawer-log" aria-label={t('log.title')} aria-hidden={!open}>
      <header><span>{t('log.title')} <kbd>J</kbd></span><span>{snap.value?.board.ok ? t('log.live') : t('log.offline')}</span><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.log.value = false; }}><Gl n="close" /></button></header>
      <div class="jlist">
        {items.length === 0 && <div class="none">{t('log.quiet')}</div>}
        {items.map((e) => {
          const { tag, ok, runner } = tagOf(e.id);
          return (
            <div class={`logline s-${e.s}`} key={e.n}>
              <span class="n">{String(e.n).padStart(3, '0')}</span><span>{timeOf(e.t)}</span>
              <button type="button" class="kk" disabled={!ok} onClick={() => void navigateTo(runner ? { kind: 'runner', id: runner } : { kind: 'planet', id: e.id })}>{tag}</button>
              <span class="tx"><RichText text={e.text || ''} /></span>
            </div>
          );
        })}
      </div>
    </section>
  );
}
