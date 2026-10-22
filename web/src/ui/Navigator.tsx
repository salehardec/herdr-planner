import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { attention, currentView, navFolds, panels, persist, selected, snap } from '../store';
import { sameEntity } from '../types';
import { buildTree, type NavNode } from './navtree';
import { Gl } from './Gl';
import { t } from '../i18n';
import { navigateTo } from '../nav';
import { NAV_W_MIN, dragNav, navMax, navWidthNow, setNavWidth } from '../nav-width';
import { openAddUniverse } from './AddUniverse';
import './navigator.css';

const focusKey = signal<string>('');

function setFold(key: string, open: boolean): void {
  navFolds.value = { ...navFolds.value, [key]: open };
  persist('herdr-planner.nav.folds', JSON.stringify(navFolds.value));
}

function activate(n: NavNode): void {
  switch (n.kind) {
    case 'addUniverse': openAddUniverse(); return;
    case 'group': case 'quiet': setFold(n.key, !n.open); return;
    case 'galaxy': void navigateTo({ kind: 'quasar', id: 'quasar' }); return;
    default:
      if (n.entity) void navigateTo(n.entity);
  }
}

function icon(n: NavNode): JSX.Element {
  switch (n.kind) {
    case 'galaxy': return <Gl n="galaxy" />;
    case 'quasar': return <Gl n="quasar" />;
    case 'universe': return <Gl n="universe" />;
    case 'planet': return <Gl n={n.status ?? 'queued'} cls={n.status === 'working' ? 'spin' : ''} />;
    case 'moon': return <Gl n="moon" />;
    case 'addUniverse': return <Gl n="plus" />;
    default: return <span class="tw" />;
  }
}

/** The tooltip of a row: the whole name (the row shows it as far as it fits) and the whole status. */
function tip(n: NavNode): string {
  if (n.kind === 'planet') return n.sub && n.sub !== n.label ? `${n.label}\n${n.sub}` : n.label;
  if (n.kind === 'moon') return [n.label, n.sub].filter(Boolean).join('\n');
  return n.sub && n.kind !== 'group' && n.kind !== 'quiet' ? `${n.label} · ${n.sub}` : n.label;
}

/** The short word after a task's name: the glyph in front of it already says the status. */
const shortStatus = (n: NavNode): string => (n.status ? t(`status.${n.status}.short`) : '');

/** The edge of the panel: dragged with the mouse, moved with the arrow keys, double click for the default. */
function Grip(): JSX.Element {
  const key = (e: KeyboardEvent): void => {
    const step = e.shiftKey ? 4 : 1;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault(); e.stopPropagation();
      setNavWidth(navWidthNow() + (e.key === 'ArrowRight' ? step : -step));
    } else if (e.key === 'Home' || e.key === 'Enter') {
      e.preventDefault(); e.stopPropagation();
      setNavWidth(null);
    }
  };
  return (
    <div
      class="nav-grip" role="separator" aria-orientation="vertical" aria-label={t('nav.resize')} title={t('nav.resizeTip')} tabIndex={0}
      aria-valuemin={NAV_W_MIN} aria-valuemax={Math.round(navMax())} aria-valuenow={Math.round(navWidthNow())}
      onPointerDown={(e) => dragNav(e as unknown as PointerEvent)} onDblClick={() => setNavWidth(null)} onKeyDown={key}
    />
  );
}

export function Navigator(): JSX.Element {
  const s = snap.value;
  const rows = buildTree({
    view: currentView.value, selected: selected.value, folds: navFolds.value,
    hostLabel: s?.host ?? '', coordinator: s ? { status: s.coordinator.status, subagents: s.coordinator.subagents } : null,
    attentionWaits: attention.value,
  });
  const onKey = (e: KeyboardEvent): void => {
    const keys = rows.map((r) => r.key);
    let i = keys.indexOf(focusKey.value);
    if (i < 0) i = Math.max(0, rows.findIndex((r) => r.entity && sameEntity(r.entity, selected.value)));
    const n = rows[i];
    const go = (j: number): void => {
      const k = Math.max(0, Math.min(rows.length - 1, j));
      focusKey.value = rows[k].key;
      (document.querySelector(`[data-nav="${CSS.escape(rows[k].key)}"]`) as HTMLElement | null)?.focus();
    };
    if (e.key === 'ArrowDown') { e.preventDefault(); go(i + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); go(i - 1); }
    else if (e.key === 'ArrowRight') {
      e.preventDefault();
      if (n?.expandable && !n.open) setFold(n.key, true); else go(i + 1);
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      if (n?.expandable && n.open) setFold(n.key, false);
      else { for (let j = i - 1; j >= 0; j--) if (rows[j].depth < (n?.depth ?? 0)) { go(j); break; } }
    } else if (e.key === 'Enter' && n) { e.preventDefault(); activate(n); }
    else if (e.key === 'Home') { e.preventDefault(); go(0); }
    else if (e.key === 'End') { e.preventDefault(); go(rows.length - 1); }
  };
  return (
    <aside class={`panel nav ${panels.nav.value ? '' : 'off'}`} id="nav" aria-label={t('nav.aria')} aria-hidden={!panels.nav.value}>
      <header><span>{t('nav.title')} <kbd>N</kbd></span><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { panels.nav.value = false; }}><Gl n="close" /></button></header>
      <div class="tree" role="tree" onKeyDown={onKey}>
        {rows.map((n) => {
          const sel = !!n.entity && sameEntity(n.entity, selected.value);
          const here = n.kind === 'galaxy';
          return (
            <div
              key={n.key} role="treeitem" tabIndex={focusKey.value === n.key || (!focusKey.value && sel) ? 0 : -1}
              data-nav={n.key} data-kind={n.kind} aria-selected={sel} aria-expanded={n.expandable ? n.open : undefined}
              class={`ti ${n.kind} ${n.status ? `s-${n.status}` : ''} ${sel ? 'sel' : ''} ${here ? 'here' : ''} ${n.dim ? 'dim' : ''} ${n.attention ? 'attn' : ''}`}
              style={`padding-left:${(6 + n.depth * 14) / 16}rem`} title={tip(n)}
              onFocus={() => { focusKey.value = n.key; }}
              onClick={() => activate(n)}
            >
              {n.expandable ? (
                <span class="tw" onClick={(e) => { e.stopPropagation(); setFold(n.key, !n.open); }}><Gl n={n.open ? 'chev' : 'chev-r'} /></span>
              ) : <span class="tw" />}
              {icon(n)}
              <span class="lab">{n.label}{n.sub && n.kind !== 'planet' ? <em>{n.sub}</em> : null}</span>
              {n.count !== undefined && <span class="cnt">{n.count}</span>}
              {n.badge ? <span class={n.kind === 'universe' ? 'lag' : 'cnt hot'}>{n.badge}</span> : null}
              {n.kind === 'planet' && n.status && n.status !== 'done' ? <span class="meta st">{shortStatus(n)}</span> : null}
              {n.kind === 'moon' && n.sub ? <span class="meta">{n.sub}</span> : null}
              {n.kind === 'galaxy' && n.sub ? <span class="meta">{n.sub}</span> : null}
            </div>
          );
        })}
      </div>
      <Grip />
    </aside>
  );
}
