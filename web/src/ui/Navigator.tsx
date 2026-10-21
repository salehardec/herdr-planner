import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { attention, galaxyId, navFolds, panels, persist, selected, snap, views } from '../store';
import { sameEntity } from '../types';
import { buildTree, type NavNode } from './navtree';
import { Gl } from './Gl';
import { t } from '../i18n';
import { enterGalaxy, navigateTo } from '../nav';
import { addDialogs } from '../dialogs';
import { openAddUniverse } from './AddUniverse';
import './navigator.css';

const focusKey = signal<string>('');

function setFold(key: string, open: boolean): void {
  navFolds.value = { ...navFolds.value, [key]: open };
  persist('herdr-planner.nav.folds', JSON.stringify(navFolds.value));
}

function activate(n: NavNode): void {
  switch (n.kind) {
    case 'addUniverse': openAddUniverse(n.galaxy ?? ''); return;
    case 'addGalaxy': addDialogs.galaxy.value = true; return;
    case 'group': case 'quiet': setFold(n.key, !n.open); return;
    case 'galaxy':
      if (n.entity) void (n.galaxy === galaxyId.value ? navigateTo({ kind: 'quasar', galaxy: n.galaxy ?? '', id: 'quasar' }) : enterGalaxy(n.galaxy ?? ''));
      return;
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
    case 'addUniverse': case 'addGalaxy': return <Gl n="plus" />;
    default: return <span class="tw" />;
  }
}

export function Navigator(): JSX.Element {
  const s = snap.value;
  const rows = buildTree({
    views: views.value, currentGalaxy: galaxyId.value, selected: selected.value, folds: navFolds.value,
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
          const here = n.kind === 'galaxy' && n.galaxy === galaxyId.value;
          return (
            <div
              key={n.key} role="treeitem" tabIndex={focusKey.value === n.key || (!focusKey.value && sel) ? 0 : -1}
              data-nav={n.key} data-kind={n.kind} aria-selected={sel} aria-expanded={n.expandable ? n.open : undefined}
              class={`ti ${n.kind} ${n.status ? `s-${n.status}` : ''} ${sel ? 'sel' : ''} ${here ? 'here' : ''} ${n.dim ? 'dim' : ''} ${n.off ? 'off' : ''} ${n.attention ? 'attn' : ''}`}
              style={`padding-left:${(6 + n.depth * 14) / 16}rem`}
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
              {n.kind === 'planet' && n.sub ? <span class="meta">{n.sub}</span> : null}
              {n.kind === 'moon' && n.sub ? <span class="meta">{n.sub}</span> : null}
              {n.kind === 'galaxy' && n.sub ? <span class="meta">{n.sub}</span> : null}
            </div>
          );
        })}
      </div>
    </aside>
  );
}
