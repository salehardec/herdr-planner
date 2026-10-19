// @ts-check
// Herdr Planner — the page. Plain ES modules with JSDoc types (checked by
// `tsc --noEmit --allowJs --checkJs` when a TypeScript compiler is around); no
// build step, no framework: the state is small and the view is a single list.
//
// Server data arrives over SSE (/api/events): `theme`, `snapshot`, `workspace`.
// The page keeps only view state: filter, chip, sort, highlight, open project.

/** @typedef {{needs:number, ready:number, failed:number, review:number, working:number, queued:number, todo:number, done:number}} Counts */
/** @typedef {{id:string, tag:string, title:string, bucket:string, label:string, msg:string, progress:number|null, at:number}} TaskBrief */
/** @typedef {{slug:string, name:string, key:string, path:string, counts:Counts, tasks:TaskBrief[], done_hidden:number, waits:{id:string, text:string, since:number}[], attention_since:number, activity:number}} Project */
/** @typedef {{configured:boolean, list:string[], assign:Record<string,{g:string, auto:boolean}>, collapsed:string[], on:boolean|null}} Groups */
/** @typedef {{id:string|null, label:string|null, slug:string|null, seq:number}} Workspace */
/** @typedef {{slug:string, at:number}} Show */
/** @typedef {{key:string, label:string, hint:string, default:string, options:[string,string][]}} SettingDef */
/** @typedef {{version:string, projects:Project[], updated:number, now:number, loading:boolean, error:string|null, groups:Groups, settings:Record<string,string>, setting_defs:SettingDef[], group_suggest:{busy:boolean, error:string|null}, workspace:Workspace, show:Show|null}} Snapshot */
/** @typedef {{is_light:boolean, vars:Record<string,string>}} Side */
/** @typedef {{name:string, system:boolean, dark:Side, light:Side|null, ui:{family:string|null,size:number}, terminal:{family:string|null,size:number}, error:string|null}} Theme */

import { esc, h, plural, clip, textMatches, post, keyOf, ago } from './util.js';
import * as Card from './card.js';
import * as Settings from './settings.js';

const OTHER = 'Прочее';
const GROUP_PREFIX = 'g:';
const CHIPS = /** @type {const} */ (['all', 'waiting', 'active', 'quiet']);
const CHIP_LABEL = { all: 'Все', waiting: 'Ждут', active: 'В работе', quiet: 'Тихие' };
const SORT_LABEL = { new: 'новые', attention: 'внимание' };

// ---------------------------------------------------------------------------
// Pure helpers (mirror src/model.rs so the page and the server agree).

/** What waits on the user in a project: an answer, a yes, a look at what failed. @param {Counts} c */
export const attention = (c) => c.needs + c.ready + c.failed;

/** Something runs or waits. @param {Counts} c */
const active = (c) => attention(c) + c.review + c.working + c.queued;

/** @param {Counts} c */
const total = (c) => active(c) + c.todo + c.done;

/** @param {Project} p @param {string} chip */
export function chipMatch(p, chip) {
  if (chip === 'waiting') return attention(p.counts) > 0;
  if (chip === 'active') return p.counts.review + p.counts.working + p.counts.queued > 0;
  if (chip === 'quiet') return active(p.counts) === 0;
  return true;
}

/** @param {Project[]} list @param {'new'|'attention'} mode */
export function sortProjects(list, mode) {
  const need = (/** @type {Project} */ p) => (attention(p.counts) > 0 ? 1 : 0);
  const byName = (/** @type {Project} */ a, /** @type {Project} */ b) => (a.slug < b.slug ? -1 : a.slug > b.slug ? 1 : 0);
  if (mode === 'attention') {
    // Who needs a look comes first, the newest event on top.
    list.sort((a, b) => need(b) - need(a) || (b.attention_since || 0) - (a.attention_since || 0)
      || attention(b.counts) - attention(a.counts) || b.counts.review - a.counts.review || b.counts.working - a.counts.working
      || b.activity - a.activity || byName(a, b));
  } else {
    // The project whose tasks moved last leads.
    list.sort((a, b) => b.activity - a.activity || attention(b.counts) - attention(a.counts) || byName(a, b));
  }
  return list;
}

/** @param {Groups} g @param {string} name */
function groupIndex(g, name) {
  const k = name.toLowerCase();
  return g.list.findIndex((x) => x.toLowerCase() === k);
}

/** @param {Groups} g */
export function groupingOn(g) {
  if (g.on === true || g.on === false) return g.on;
  return Object.values(g.assign).some((e) => e.g !== '');
}

/** @param {Groups} g @param {string} slug @returns {[string, boolean]} */
export function groupOf(g, slug) {
  const e = g.assign[slug];
  if (e && e.g !== '' && groupIndex(g, e.g) >= 0) return [e.g, e.auto];
  return [OTHER, e ? e.auto : false];
}

// ---------------------------------------------------------------------------
// State

const view = {
  /** @type {Snapshot|null} */ snap: null,
  /** @type {Theme|null} */ theme: null,
  query: '',
  /** @type {'all'|'waiting'|'active'|'quiet'} */ chip: 'all',
  /** @type {'new'|'attention'} */ sort: 'new',
  /** @type {string|null} */ hl: null,
  /** @type {string|null} */ selected: null,
  help: false,
  /** A full-screen form over the list or the card. @type {'settings'|null} */ screen: null,
  filtering: false,
  /** @type {{slug:string, at:number, err:string|null, adding:boolean}|null} */ gpick: null,
  /** @type {string|null} */ status: null,
  /** The workspace switch still waiting (typing, list not loaded). */
  /** @type {string|null} */ wsWant: null,
  wsSeq: -1,
  /** The last «show this project» request that was obeyed (its time). */
  showAt: 0,
  /** @type {number} */ ticker: 0,
};

const root = /** @type {HTMLElement} */ (document.getElementById('app'));

// ---------------------------------------------------------------------------
// Theme

/** @param {Theme} t */
function applyTheme(t) {
  view.theme = t;
  const prefersLight = window.matchMedia('(prefers-color-scheme: light)').matches;
  const side = t.system && t.light && prefersLight ? t.light : t.dark;
  const css = document.documentElement.style;
  for (const [k, v] of Object.entries(side.vars)) css.setProperty('--' + k, v);
  css.setProperty('color-scheme', side.is_light ? 'light' : 'dark');
  const ui = t.ui.family ? `"${t.ui.family}", system-ui, sans-serif` : 'system-ui, sans-serif';
  const mono = t.terminal.family ? `"${t.terminal.family}", ui-monospace, monospace` : 'ui-monospace, monospace';
  css.setProperty('--font-ui', ui);
  css.setProperty('--font-mono', mono);
  css.setProperty('--size-ui', `${Math.max(10, t.ui.size + 1)}px`);
}

window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => { if (view.theme) applyTheme(view.theme); });

// ---------------------------------------------------------------------------
// Server

function connect() {
  const es = new EventSource('/api/events');
  es.addEventListener('theme', (e) => { applyTheme(JSON.parse(/** @type {MessageEvent} */ (e).data)); render(); });
  es.addEventListener('snapshot', (e) => {
    const snap = /** @type {Snapshot} */ (JSON.parse(/** @type {MessageEvent} */ (e).data));
    view.snap = snap;
    followWorkspace(snap.workspace);
    if (snap.show) showRequest(snap.show);
    if (view.selected) Card.onSnapshot();
    render();
  });
  es.addEventListener('show', (e) => { showRequest(JSON.parse(/** @type {MessageEvent} */ (e).data)); render(); });
  es.addEventListener('workspace', (e) => {
    followWorkspace(JSON.parse(/** @type {MessageEvent} */ (e).data));
    render();
  });
  es.onerror = () => { view.status = 'нет связи с сервером, переподключение…'; render(); };
  es.onopen = () => { view.status = null; render(); };
}

// ---------------------------------------------------------------------------
// Following the active herdr workspace. Only a change counts: a refresh of
// the same workspace never switches the view back after the user left it.

/** @param {Workspace} ws */
function followWorkspace(ws) {
  if (ws.seq === view.wsSeq) return;
  view.wsSeq = ws.seq;
  view.wsWant = ws.slug;
  settleFollow();
}

function typing() {
  return view.filtering || view.gpick != null || (view.screen === 'settings' && Settings.typing())
    || (view.selected != null && Card.typing());
}

function settleFollow() {
  const slug = view.wsWant;
  if (slug == null || typing() || !view.snap || view.snap.updated === 0) return;
  view.wsWant = null;
  if (slug === view.selected) return;
  if (!view.snap.projects.some((p) => p.slug === slug)) return;
  selectProject(slug);
}

/** The bar popup asked for a project: it is shown once, whatever the page was doing. @param {Show} show */
function showRequest(show) {
  if (show.at <= view.showAt) return;
  view.showAt = show.at;
  view.wsWant = null;
  if (view.snap && view.snap.projects.some((p) => p.slug === show.slug)) selectProject(show.slug);
}

/** @param {string} slug */
function selectProject(slug) {
  view.selected = slug;
  view.help = false;
  view.screen = null;
  Card.open(slug);
}

function backToList() {
  view.hl = view.selected;
  view.selected = null;
  Card.close();
}

// ---------------------------------------------------------------------------
// Layout of the list

/** @typedef {{key:string, project?:Project, auto?:boolean, section?:Section}} Row */
/** @typedef {{name:string, other:boolean, collapsed:boolean, projects:Project[], autos:boolean[], waiting:number, review:number, working:number}} Section */

function visibleProjects() {
  const snap = view.snap;
  if (!snap) return [];
  const out = snap.projects.filter((p) => chipMatch(p, view.chip) && textMatches(`${p.name} ${p.slug} ${p.key}`, view.query));
  return sortProjects(out, view.sort);
}

function filteringActive() {
  return view.query !== '' || view.chip !== 'all';
}

/** @returns {{rows:Row[], visible:Project[], sections:Section[]|null}} */
function layout() {
  const visible = visibleProjects();
  const g = view.snap ? view.snap.groups : null;
  /** @type {Row[]} */ const rows = [];
  /** @type {Section[]|null} */ let sections = null;
  if (g && groupingOn(g)) {
    sections = [];
    /** @type {Map<string, {p:Project, auto:boolean}[]>} */ const by = new Map();
    for (const p of visible) {
      const [name, auto] = groupOf(g, p.slug);
      if (!by.has(name)) by.set(name, []);
      by.get(name)?.push({ p, auto });
    }
    const showEmpty = !filteringActive();
    const add = (/** @type {string} */ name, /** @type {boolean} */ other) => {
      const items = by.get(name) || [];
      if (items.length === 0 && !(showEmpty && !other)) return;
      const s = {
        name, other, collapsed: g.collapsed.includes(name), projects: items.map((i) => i.p), autos: items.map((i) => i.auto),
        waiting: items.reduce((n, i) => n + attention(i.p.counts), 0), review: items.reduce((n, i) => n + i.p.counts.review, 0),
        working: items.reduce((n, i) => n + i.p.counts.working + i.p.counts.queued, 0),
      };
      sections?.push(s);
      rows.push({ key: GROUP_PREFIX + name, section: s });
      if (!s.collapsed) for (const i of items) rows.push({ key: i.p.slug, project: i.p, auto: i.auto, section: s });
    };
    for (const name of g.list) add(name, false);
    add(OTHER, true);
  } else {
    for (const p of visible) rows.push({ key: p.slug, project: p });
  }
  return { rows, visible, sections };
}

/** @param {Row[]} rows */
function highlighted(rows) {
  return rows.some((r) => r.key === view.hl) ? view.hl : null;
}

/** @param {number} delta */
function moveHl(delta) {
  const { rows } = layout();
  const cur = highlighted(rows);
  let at = rows.findIndex((r) => r.key === cur);
  if (at < 0) at = delta > 0 ? -1 : rows.length;
  view.hl = rows[Math.max(0, Math.min(rows.length - 1, at + delta))].key;
  render();
  const el = root.querySelector('.row.hl');
  if (el) el.scrollIntoView({ block: 'nearest' });
}

/** The highlighted project, if the highlight is on one. */
function hlProject() {
  const { rows } = layout();
  const key = highlighted(rows);
  return rows.find((r) => r.key === key)?.project || null;
}

// ---------------------------------------------------------------------------
// Actions

function refresh() {
  if (view.snap) view.snap.loading = true;
  render();
  post('/api/refresh', {}).catch(() => {});
}

function toggleSort() {
  view.sort = view.sort === 'new' ? 'attention' : 'new';
  render();
}

function toggleGroups() {
  post('/api/groups', { op: 'toggle' }).catch((e) => flash(String(e.message)));
}

/** @param {string} name @param {boolean|null} folded */
function foldGroup(name, folded) {
  post('/api/groups', { op: 'fold', name, folded }).catch((e) => flash(String(e.message)));
}

/** @param {string} msg @param {number} [ms] */
function flash(msg, ms = 4000) {
  view.status = msg;
  render();
  setTimeout(() => { if (view.status === msg) { view.status = null; render(); } }, ms);
}

/** `unfocus` needs no project: the herdr sidebar shows every project again. */
function runUnfocus() {
  flash('Показываю все проекты…');
  post('/api/action', { kind: 'unfocus' })
    .then(() => flash('Боковая панель herdr снова показывает все проекты'))
    .catch((e) => flash(`Не вышло: ${e.message}`));
}

function openSettings() {
  view.screen = 'settings';
  view.help = false;
  Settings.open();
  render();
}

function closeScreen() {
  view.screen = null;
  render();
  settleFollow();
}

function startGroupPick() {
  const { rows } = layout();
  const key = highlighted(rows);
  const row = rows.find((r) => r.key === key);
  if (!row || !row.project) { flash('Подсветьте проект (↑ / ↓ или клик), затем T — группа.'); return; }
  view.gpick = { slug: row.project.slug, at: 0, err: null, adding: false };
  const g = view.snap?.groups;
  if (g) {
    const [cur] = groupOf(g, row.project.slug);
    const i = g.list.findIndex((n) => n === cur);
    view.gpick.at = i >= 0 ? i : g.list.length + 1;
  }
  render();
}

/** Rows of the picker: the groups, «новая группа…», «без группы». */
function gpickRows() {
  const g = view.snap?.groups;
  const list = g ? g.list.slice() : [];
  return [...list.map((n) => ({ key: n, label: n })), { key: '+', label: 'новая группа…' }, { key: '', label: 'без группы' }];
}

/** @param {string} key */
function gpickChoose(key) {
  const gp = view.gpick;
  if (!gp) return;
  if (key === '+') { gp.adding = true; render(); root.querySelector('input')?.focus(); return; }
  post('/api/groups', { op: 'assign', slug: gp.slug, name: key })
    .then(() => { view.gpick = null; view.hl = gp.slug; render(); settleFollow(); })
    .catch((e) => { gp.err = e.message; render(); });
}

/** @param {string} name */
function gpickAdd(name) {
  const gp = view.gpick;
  if (!gp) return;
  post('/api/groups', { op: 'assign', slug: gp.slug, name })
    .then(() => { view.gpick = null; view.hl = gp.slug; render(); settleFollow(); })
    .catch((e) => { gp.err = e.message; render(); });
}

// ---------------------------------------------------------------------------
// Keys (the list's keys as in the plugin; the filter field keeps its own)

document.addEventListener('keydown', (ev) => {
  const target = /** @type {HTMLElement} */ (ev.target);
  const inInput = target && target.tagName === 'INPUT';
  // Shortcuts mean the same on a Russian layout; fields get the keys as typed.
  const k = inInput ? ev.key : keyOf(ev);
  if (view.screen) {
    if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
    const handled = inInput ? Settings.inputKey(ev) : Settings.key(ev);
    if (handled) ev.preventDefault();
    return;
  }
  if (view.gpick) {
    if (view.gpick.adding) {
      if (k === 'Escape') { view.gpick.adding = false; render(); ev.preventDefault(); }
      return;
    }
    const rows = gpickRows();
    if (k === 'ArrowDown' || k === 'ArrowUp') { view.gpick.at = Math.max(0, Math.min(rows.length - 1, view.gpick.at + (k === 'ArrowDown' ? 1 : -1))); render(); }
    else if (k === 'Enter') gpickChoose(rows[view.gpick.at].key);
    else if (k === 'Escape') { view.gpick = null; render(); settleFollow(); }
    else return;
    ev.preventDefault();
    return;
  }
  if (inInput && view.selected) {
    // The card's fields (filter, question) handle their own keys.
    if (Card.inputKey(ev, target)) ev.preventDefault();
    return;
  }
  if (inInput) {
    // ↑ / ↓ leave the field and move the highlight; the text stays.
    if (k === 'ArrowDown' || k === 'ArrowUp') { target.blur(); view.filtering = false; moveHl(k === 'ArrowDown' ? 1 : -1); ev.preventDefault(); }
    else if (k === 'Escape') { target.blur(); view.filtering = false; render(); settleFollow(); ev.preventDefault(); }
    else if (k === 'Enter') {
      target.blur(); view.filtering = false;
      if (!view.selected) { const first = visibleProjects()[0]; if (first) selectProject(first.slug); }
      render(); ev.preventDefault();
    }
    return;
  }
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
  if (view.selected && Card.key(ev)) { ev.preventDefault(); return; }
  if (k === '/') { view.filtering = true; render(); focusFilter(); ev.preventDefault(); return; }
  if (k === 'r') { refresh(); return; }
  if (k === 'u') { runUnfocus(); return; }
  if (k === 'Escape') {
    // The list keeps its filter while a project is open; the card has its own.
    if (view.selected) backToList();
    else if (view.help) view.help = false;
    else if (view.query !== '' || view.chip !== 'all') { view.query = ''; view.chip = 'all'; }
    else if (view.hl) view.hl = null;
    render();
    return;
  }
  if (view.selected) {
    if (k === 'ArrowLeft') { backToList(); render(); }
    return;
  }
  const { rows } = layout();
  const at = highlighted(rows);
  if (k === 'ArrowDown' || k === 'ArrowUp') { moveHl(k === 'ArrowDown' ? 1 : -1); ev.preventDefault(); }
  else if (k === 'Enter') {
    if (at && at.startsWith(GROUP_PREFIX)) foldGroup(at.slice(GROUP_PREFIX.length), null);
    else if (at) selectProject(at);
    render();
  }
  else if ((k === 'ArrowLeft' || k === 'ArrowRight') && at && at.startsWith(GROUP_PREFIX)) foldGroup(at.slice(GROUP_PREFIX.length), k === 'ArrowLeft');
  else if (k >= '1' && k <= '4') { view.chip = CHIPS[Number(k) - 1]; render(); }
  else if (k === 's') toggleSort();
  else if (k === 'g') toggleGroups();
  else if (k === 't') startGroupPick();
  else if (k === ',') openSettings();
  else if (k === '?') { view.help = !view.help; render(); }
});

function focusFilter() {
  const input = /** @type {HTMLInputElement|null} */ (root.querySelector('#filter'));
  if (input) { input.focus(); input.select(); }
}

// ---------------------------------------------------------------------------
// Rendering

function ageText() {
  const s = view.snap;
  if (!s) return '';
  if (s.loading) return 'обновление…';
  if (s.updated > 0) {
    const age = Math.floor((Date.now() - s.updated) / 1000);
    return age < 5 ? 'только что' : `${age} с назад`;
  }
  return '';
}

/** The board in one line: what waits on the user, what runs. @param {Project[]} projects */
function totalsText(projects) {
  /** @type {Counts} */ const c = { needs: 0, ready: 0, failed: 0, review: 0, working: 0, queued: 0, todo: 0, done: 0 };
  for (const p of projects) for (const k of /** @type {(keyof Counts)[]} */ (Object.keys(c))) c[k] += p.counts[k];
  return Card.countersText(c);
}

function headerHtml() {
  const s = view.snap;
  const n = s ? s.projects.length : 0;
  let title = h('strong', {}, 'Планировщик проектов');
  if (n > 0) title += h('span', { class: 'muted' }, ` · ${n} ${plural(n, 'проект', 'проекта', 'проектов')}`);
  if (s) title += h('span', { class: 'muted' }, ` · v${esc(s.version)}`);
  const age = ageText();
  const stamp = s && s.loading ? 'обновление… ↻' : (age ? `обновлено ${age} ↻` : '↻');
  const totals = s && s.updated > 0 ? totalsText(s.projects) : '';
  return h('div', { class: 'header' },
    h('div', { class: 'title' }, title)
    + h('span', { class: 'badge', 'data-act': 'settings', title: 'Настройки (,)' }, '⚙ Настройки')
    + h('span', { class: 'stamp', 'data-act': 'refresh', title: 'Обновить (R)' }, esc(stamp)))
    + (s && s.updated > 0 ? h('div', { class: 'totals' }, totals ? esc(totals) : h('span', { class: 'muted' }, 'на доске всё тихо')) : '');
}

/** The second line of a project: what waits on the user, else what runs. @param {Project} p */
function projectDetail(p) {
  if (p.waits.length > 0) return p.waits.map((w) => w.text).join('; ');
  const run = p.tasks.find((t) => t.bucket === 'working' || t.bucket === 'review');
  if (run) return `${run.tag} ${run.title}${run.progress != null ? ` · ${run.progress}%` : ''}`;
  const queued = p.tasks.find((t) => t.bucket === 'queued');
  return queued ? `${queued.tag} ${queued.title} · ${queued.label}` : '';
}

/** @param {Project} p @param {boolean} hl @param {{indent?:boolean, auto?:boolean}} opts */
function projectRow(p, hl, opts) {
  const c = p.counts;
  let label = h('span', { class: 'strong' }, esc(p.name));
  if (p.key) label += h('span', { class: 'muted' }, `  ${esc(p.key)}`);
  if (opts.auto) label += h('span', { class: 'dim' }, '  авто');
  let value = '';
  if (c.needs > 0) value += h('span', { class: 'warning strong', title: p.waits.map((w) => w.text).join('; ') }, `${c.needs} ждут  `);
  if (c.ready > 0) value += h('span', { class: 'success strong' }, `${c.ready} к одобр.  `);
  if (c.failed > 0) value += h('span', { class: 'error' }, `${c.failed} ${plural(c.failed, 'упала', 'упали', 'упали')}  `);
  if (c.review > 0) value += h('span', { class: 'info' }, `${c.review} ревью  `);
  if (c.working > 0) value += h('span', { class: 'accent' }, `${c.working} в работе  `);
  if (c.queued > 0) value += h('span', { class: 'muted' }, `${c.queued} в очереди  `);
  if (attention(c) > 0 && p.attention_since) value += h('span', { class: 'muted ago', title: 'Когда задача пришла в эту стадию' }, `${ago(p.attention_since)}  `);
  const all = total(c);
  value += all > 0 ? h('span', { class: 'num' }, `${c.done}/${all}`) : h('span', { class: 'muted' }, '—');
  if (hl) value += h('span', { class: 'hint' }, 'Открыть <kbd>↵</kbd> или 2× клик · Показать все <kbd>U</kbd> · Группа <kbd>T</kbd>');
  let icon = '○', tone = 'muted';
  if (c.needs + c.ready > 0) { icon = '🔔'; tone = 'warning'; }
  else if (c.failed > 0) { icon = '✖'; tone = 'error'; }
  else if (c.review + c.working > 0) { icon = '◉'; tone = 'accent'; }
  else if (c.queued > 0) { icon = '◌'; tone = 'muted'; }
  if (hl) { icon = '➜'; tone = 'accent'; }
  const cls = `row${hl ? ' hl' : ''}${opts.indent ? ' indent' : ''}${active(c) === 0 ? ' quiet' : ''}`;
  const detail = projectDetail(p);
  return h('div', { class: cls, 'data-row': p.slug, title: p.path || p.slug },
    h('span', { class: `icon ${tone}` }, icon) + h('span', { class: 'label' }, label) + h('span', { class: 'value' }, value)
    + (detail ? h('span', { class: 'detail' }, esc(clip(detail, 160))) : ''));
}

/** @param {Section} s @param {boolean} hl */
function groupRow(s, hl) {
  /** @type {string[]} */ const parts = [];
  if (s.waiting > 0) parts.push(h('span', { class: 'warning strong' }, `${s.waiting} ждут`));
  if (s.review > 0) parts.push(h('span', { class: 'info' }, `${s.review} ревью`));
  if (s.working > 0) parts.push(h('span', { class: 'accent' }, `${s.working} в работе`));
  let value = parts.join(h('span', { class: 'sep' }, ' · '));
  if (hl) value += h('span', { class: 'hint' }, `${s.collapsed ? 'Развернуть' : 'Свернуть'} <kbd>↵</kbd> или 2× клик`);
  let label = h('span', {}, `${s.collapsed ? '▸' : '▾'} ${esc(s.name)}`) + h('span', { class: 'muted' }, ` · ${s.projects.length}`);
  if (s.collapsed && s.projects.length > 0) label += h('span', { class: 'muted names' }, esc(clip(s.projects.map((p) => p.name).join(', '), 80)));
  return h('div', { class: `row group${hl ? ' hl' : ''}`, 'data-group': s.name },
    h('span', { class: `icon ${hl ? 'accent' : ''}` }, hl ? '➜' : '▦') + h('span', { class: 'label' }, label) + h('span', { class: 'value' }, value));
}

function controlsHtml() {
  const s = view.snap;
  if (!s) return '';
  const counts = Object.fromEntries(CHIPS.map((c) => [c, s.projects.filter((p) => chipMatch(p, c)).length]));
  const chips = CHIPS.map((c) => h('span', { class: `badge${view.chip === c ? ' active' : ''}`, 'data-chip': c, title: `Раздел (${CHIPS.indexOf(c) + 1})` },
    `${view.chip === c ? '● ' : ''}${CHIP_LABEL[c]} ${counts[c]}`)).join('');
  const grouped = groupingOn(s.groups);
  const vis = visibleProjects().length;
  const shown = filteringActive() ? h('span', { class: 'muted' }, `${vis} из ${s.projects.length}`) : '';
  return h('div', { class: 'controls' },
    h('div', { class: 'filter' }, `<kbd>/</kbd>` + `<input id="filter" type="search" placeholder="фильтр: имя, ключ" value="${esc(view.query)}" autocomplete="off">` + shown)
    + chips
    + h('span', { class: 'toggle', 'data-act': 'sort' }, `⇅ ${SORT_LABEL[view.sort]} <kbd>S</kbd>`)
    + h('span', { class: 'toggle', 'data-act': 'groups' }, `▦ группы ${grouped ? 'вкл' : 'выкл'} <kbd>G</kbd>`)
    + h('span', { class: 'toggle', 'data-act': 'unfocus', title: 'Боковая панель herdr: снова показать все проекты' }, `◎ все проекты <kbd>U</kbd>`));
}

function listHtml() {
  const s = view.snap;
  if (!s) return h('div', { class: 'loading' }, 'Загрузка…');
  let out = '';
  if (s.error) {
    out += h('div', { class: 'note error' }, `Не удалось получить проекты: ${esc(s.error)}` + h('span', { class: 'badge', 'data-act': 'refresh' }, 'Повторить'));
  }
  if (s.projects.length === 0) {
    out += h('div', { class: 'note' }, s.loading ? '◌ Загрузка проектов…' : 'Нет проектов.');
    return out;
  }
  const { rows, visible } = layout();
  const hl = highlighted(rows);
  if (visible.length === 0) {
    out += h('div', { class: 'note' }, view.query !== '' ? `Ничего не найдено по «${esc(view.query)}».` : `Нет проектов в разделе «${CHIP_LABEL[view.chip]}».`);
  }
  let items = '';
  for (const r of rows) {
    if (r.project) items += projectRow(r.project, hl === r.key, { indent: r.section != null, auto: r.auto });
    else if (r.section) items += groupRow(r.section, hl === r.key);
  }
  out += h('div', { class: 'list' }, items);
  return out;
}

function dockHtml() {
  /** @param {string[]} keys @param {string} text */
  const hint = (keys, text) => h('span', {}, keys.map((k) => `<kbd>${esc(k)}</kbd>`).join('') + ' ' + esc(text));
  if (view.screen === 'settings') return h('div', { class: 'dock' }, Settings.dock(hint));
  if (view.gpick) return h('div', { class: 'dock' }, hint(['↑', '↓'], 'группа') + hint(['↵'], 'выбрать') + hint(['esc'], 'назад'));
  if (view.filtering && !view.selected) return h('div', { class: 'dock' }, hint(['↵'], 'открыть первый') + hint(['esc'], 'выйти из фильтра'));
  if (view.selected) return h('div', { class: 'dock' }, Card.dock(hint));
  if (!view.help) return h('div', { class: 'dock' }, hint(['?'], 'все клавиши'));
  return h('div', { class: 'dock' },
    hint(['↑', '↓'], 'выбор') + hint(['↵'], 'открыть (или двойной клик)') + hint(['u'], 'показать все проекты')
    + hint(['/'], 'фильтр') + hint(['1', '2', '3', '4'], 'раздел') + hint(['s'], 'сортировка') + hint(['g'], 'группы вкл / выкл')
    + hint(['t'], 'группа проекта') + hint(['←', '→'], 'свернуть / развернуть группу') + hint([','], 'настройки') + hint(['r'], 'обновить')
    + hint(['esc'], 'сбросить / закрыть'));
}

function gpickHtml() {
  const gp = view.gpick;
  if (!gp) return '';
  const p = view.snap?.projects.find((x) => x.slug === gp.slug);
  const g = view.snap?.groups;
  const [cur, auto] = g ? groupOf(g, gp.slug) : [OTHER, false];
  let rows = '';
  gpickRows().forEach((r, i) => {
    const isCur = (r.key === cur) || (r.key === '' && cur === OTHER && g?.assign[gp.slug] != null);
    const hl = i === gp.at;
    rows += h('div', { class: `row${hl ? ' hl' : ''}`, 'data-gpick': r.key },
      h('span', { class: `icon ${hl ? 'accent' : ''}` }, hl ? '➜' : (isCur ? '●' : '')) + h('span', { class: 'label' }, esc(r.label))
      + h('span', { class: 'value muted' }, isCur ? (auto ? 'сейчас (авто)' : 'сейчас') : ''));
  });
  let body = rows;
  if (gp.adding) body += `<input id="gnew" type="text" placeholder="название новой группы" maxlength="40">`;
  if (gp.err) body += h('div', { class: 'err' }, esc(gp.err));
  return h('div', { class: 'overlay', 'data-act': 'gpick-close' },
    h('div', { class: 'dialog', role: 'dialog' }, h('h2', {}, `Группа для «${esc(p ? p.name : gp.slug)}»`) + body));
}

/** The fields whose text and caret survive a re-render. */
const FIELDS = ['filter', 'cfilter', 'ctell', 'gedit'];
const isField = (/** @type {string} */ id) => FIELDS.includes(id);

/** True while render() swaps the page: replacing a focused field fires focus events that are not the user's. */
let rendering = false;

function render() {
  if (rendering) return;
  rendering = true;
  try { paint(); } finally { rendering = false; }
}

function paint() {
  const active = /** @type {HTMLInputElement|null} */ (document.activeElement);
  const fieldId = active && isField(active.id) ? active.id : null;
  const sel = fieldId && active ? [active.selectionStart, active.selectionEnd] : null;
  // The scrolled list keeps its place, as long as the same list is on screen.
  const scrolled = /** @type {HTMLElement|null} */ (root.querySelector('[data-scroll]'));
  const scroll = scrolled ? { key: scrolled.getAttribute('data-scroll'), top: scrolled.scrollTop } : null;
  let html = '';
  if (view.screen === 'settings') html = Settings.html();
  else if (view.selected) html = Card.html();
  else html = headerHtml() + controlsHtml() + (view.status ? h('div', { class: 'note' }, esc(view.status)) : '') + listHtml();
  if ((view.selected || view.screen) && view.status) html += h('div', { class: 'note' }, esc(view.status));
  html += dockHtml() + gpickHtml();
  root.innerHTML = html;
  const again = /** @type {HTMLElement|null} */ (root.querySelector('[data-scroll]'));
  if (scroll && again && again.getAttribute('data-scroll') === scroll.key) again.scrollTop = scroll.top;
  if (fieldId) {
    const input = /** @type {HTMLInputElement|null} */ (root.querySelector('#' + fieldId));
    if (input) { input.focus(); if (sel && sel[0] != null && sel[1] != null) input.setSelectionRange(sel[0], sel[1]); }
  }
  if (view.gpick?.adding) root.querySelector('#gnew')?.addEventListener('keydown', (ev) => {
    const kev = /** @type {KeyboardEvent} */ (ev);
    if (kev.key === 'Enter') { gpickAdd(/** @type {HTMLInputElement} */ (kev.target).value); kev.preventDefault(); }
  });
}

// Clicks: one click highlights a row, a double click opens it; badges act.
root.addEventListener('click', (ev) => {
  const el = /** @type {HTMLElement} */ (ev.target);
  if (view.screen === 'settings' && Settings.click(el)) return;
  const act = el.closest('[data-act]');
  if (act && act.getAttribute('data-act') === 'gpick-close' && el !== act) { /* inside the dialog */ } else if (act) {
    const a = act.getAttribute('data-act');
    if (a === 'refresh') refresh();
    else if (a === 'sort') toggleSort();
    else if (a === 'groups') toggleGroups();
    else if (a === 'settings') openSettings();
    else if (a === 'back') { backToList(); render(); }
    else if (a === 'unfocus') runUnfocus();
    else if (a === 'gpick-close') { view.gpick = null; render(); settleFollow(); }
    return;
  }
  if (view.selected && Card.click(el)) return;
  const gp = el.closest('[data-gpick]');
  if (gp) { gpickChoose(gp.getAttribute('data-gpick') || ''); return; }
  const chip = el.closest('[data-chip]');
  if (chip) { view.chip = /** @type {any} */ (chip.getAttribute('data-chip')); render(); return; }
  const grow = el.closest('[data-group]');
  if (grow) { view.hl = GROUP_PREFIX + grow.getAttribute('data-group'); render(); return; }
  const row = el.closest('[data-row]');
  if (row) { view.hl = row.getAttribute('data-row'); render(); }
});

root.addEventListener('dblclick', (ev) => {
  const el = /** @type {HTMLElement} */ (ev.target);
  const grow = el.closest('[data-group]');
  if (grow) { foldGroup(grow.getAttribute('data-group') || '', null); return; }
  const row = el.closest('[data-row]');
  if (!row) return;
  const key = row.getAttribute('data-row');
  if (key) selectProject(key);
  render();
});

root.addEventListener('input', (ev) => {
  const el = /** @type {HTMLInputElement} */ (ev.target);
  if (el.id === 'filter') { view.query = el.value; render(); }
  else if (view.screen === 'settings') Settings.input(el);
  else Card.input(el);
});
// render() re-creates the field and refocuses it, so only a change of state re-renders.
root.addEventListener('focusin', (ev) => {
  if (rendering) return;
  const el = /** @type {HTMLElement} */ (ev.target);
  if (view.screen) return;
  if (el.id === 'filter' && !view.filtering) { view.filtering = true; render(); }
  else if (view.selected) Card.focusChange(el, true);
});
root.addEventListener('focusout', (ev) => {
  if (rendering || view.screen) return;
  const el = /** @type {HTMLElement} */ (ev.target);
  if (el.id === 'filter' && view.filtering) { view.filtering = false; render(); settleFollow(); }
  else if (view.selected && el.id === 'cfilter') { Card.focusChange(el, false); settleFollow(); }
});

// The «обновлено … с назад» stamp ticks without a server event.
setInterval(() => { if (!view.filtering && !view.gpick && !view.screen && !Card.typing()) { const st = root.querySelector('.stamp'); if (st && view.snap && !view.snap.loading) st.textContent = `обновлено ${ageText()} ↻`; } }, 5000);

Settings.init({ render, flash, root, snap: () => view.snap, close: closeScreen });
Card.init({
  render,
  flash,
  root,
  openSettings,
  project: (slug) => view.snap?.projects.find((p) => p.slug === slug) || null,
  stamp: () => (view.snap?.loading ? 'обновление… ↻' : `обновлено ${ageText()} ↻`),
});
render();
connect();
