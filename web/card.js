// @ts-check
// The project card: threads with their git lines and model badges, the tasks
// of TASKS.md with «Решить» / «Не решать» / «Задать вопрос», the card's filter.
// Behaviour and keys follow the Tern plugin's detail view (0.17.1).
//
// The server does the heavy work (git, profiles, trailers) and answers
// /api/project/{slug} at once from its cache; it sends a `card` event when it
// learned something, and the page asks again. TASKS.md is never edited here:
// the coordinator is asked (see src/actions.rs).

import { esc, h, plural, clip, oneLine, textMatches, post, getJson, markdown, keyOf } from './util.js';

/**
 * @typedef {{hash:string, subject:string}} Commit
 * @typedef {{err:string|null, base:string|null, branch:string|null, dirty:number, ahead:number|null, files:number, add:number, del:number, stat:string[], log:Commit[], wt:string[], co:string[], src:string|null}} Git
 * @typedef {{label:string, family:string, quiet:boolean}} Badge
 * @typedef {{badges:Badge[], source:string|null, src:string|null, launch_parts:string[]}} Models
 * @typedef {{id:string, title:string, group:string, state:string, note:string|null, closed:boolean, failed:boolean, error:string, rank:number, branch:string, worktree:string|null, base:string, kind:string, path:string|null, wt_exists:boolean, pr:string, pr_state:string, pr_number:number|null, profile:string, machine:string, percent:number|null, activity:string, summary:string|null, git:Git|null, models:Models, search:string}} Thread
 * @typedef {{idx:number, done:boolean, full:string, text:string, notes:string, owner:string|null, thread:string|null, thread_led:boolean}} Task
 * @typedef {{q:string, a?:string, error?:string, pending?:boolean}} Qa
 * @typedef {{project:any, tasks:Task[], tasks_read:boolean, threads:Thread[], qa:Record<string,Qa[]>, sent:Record<string,any>, agent:string, error:string|null}} CardData
 * @typedef {{kind:'t', key:string, t:Thread}|{kind:'k', key:string, k:Task}} Row
 */

/** What the page gives the card: rendering, the status line, the project record, the refresh stamp. @type {{ render:()=>void, flash:(msg:string, ms?:number)=>void, root:HTMLElement, project:(slug:string)=>any, stamp:()=>string }} */
let ctx;

/** @param {typeof ctx} c */
export function init(c) { ctx = c; }

const STATUS_LABEL = /** @type {Record<string,string>} */ ({ active: 'активен', paused: 'пауза', archived: 'архив' });
const STATUS_ICON = /** @type {Record<string,string>} */ ({ active: '◉', paused: '⏸', archived: '▣' });
/** The glyph and tone of an open thread by herdr group. */
const GROUP_META = /** @type {Record<string,[string,string]>} */ ({
  'Waiting on you': ['🔔', 'warning'], 'Needs you': ['🔔', 'warning'], 'Ready for review': ['☑', 'warning'],
  Blocked: ['✖', 'error'], Landing: ['⇪', 'info'], Working: ['◉', 'accent'], Idle: ['◌', 'muted'], Resolved: ['✔', 'success'],
});
const PR_WORD = /** @type {Record<string,string>} */ ({ MERGED: 'слит', OPEN: 'открыт', CLOSED: 'закрыт', DRAFT: 'черновик' });
const PR_TONE = /** @type {Record<string,string>} */ ({ MERGED: 'success', OPEN: 'info', CLOSED: 'muted', DRAFT: 'muted' });

export const card = {
  /** @type {string|null} */ slug: null,
  /** @type {CardData|null} */ data: null,
  inflight: false,
  again: false,
  /** @type {'todo'|'done'} */ tab: 'todo',
  query: '',
  filtering: false,
  /** The highlighted row: a thread id or `k<idx>` of a task. @type {string|null} */ hl: null,
  /** @type {Set<string>} */ expanded: new Set(),
  /** Opened threads' git in full, by thread id. @type {Record<string,{git:Git|null, busy:boolean, err:string|null}>} */ details: {},
  /** The picked task's full text. @type {string|null} */ picked: null,
  asking: false,
  qtext: '',
  removing: false,
  profileOpen: false,
  /** '' = the coordinator chooses. */ profile: '',
  /** @type {{slug:string, profiles:string[], error:string|null, loaded:boolean}|null} */ assign: null,
  /** @type {{task:string, stage:string}|null} */ solving: null,
  /** @type {number} */ pendingTimer: 0,
};

/** A task's key in the saved answers and marks is its text. */
const encode = encodeURIComponent;

// ---------------------------------------------------------------------------
// Loading

/** Opens the card of `slug`; a project that was open before is forgotten. @param {string} slug */
export function open(slug) {
  if (card.slug !== slug) {
    card.slug = slug;
    card.data = null;
    card.tab = 'todo';
    card.query = '';
    card.filtering = false;
    card.hl = null;
    card.expanded = new Set();
    card.details = {};
    dropPick();
    card.qtext = '';
    card.assign = null;
    card.solving = null;
  }
  load();
}

export function close() {
  card.slug = null;
  card.data = null;
  card.filtering = false;
  window.clearTimeout(card.pendingTimer);
}

/** Asks the server for the card; requests that overlap are folded into one more. */
export async function load() {
  const slug = card.slug;
  if (!slug) return;
  if (card.inflight) { card.again = true; return; }
  card.inflight = true;
  try {
    const d = /** @type {CardData} */ (await getJson(`/api/project/${encode(slug)}`));
    if (card.slug === slug) { card.data = d; normalize(); }
  } catch (e) {
    if (card.slug === slug && !card.data) card.data = /** @type {any} */ ({ error: String(/** @type {Error} */ (e).message || e), threads: [], tasks: [], qa: {}, sent: {} });
  }
  card.inflight = false;
  if (card.slug === slug) {
    ctx.render();
    loadOpened();
    watchPending();
  }
  if (card.again) { card.again = false; load(); }
}

/** After a load: forget a pick, a highlight or an opened thread that is gone. */
function normalize() {
  const d = card.data;
  if (!d) return;
  if (card.picked && !(d.tasks || []).some((k) => k.full === card.picked && !k.done)) dropPick();
  const ids = new Set((d.threads || []).map((t) => t.id));
  for (const id of [...card.expanded]) if (!ids.has(id)) card.expanded.delete(id);
}

/** An answer being written arrives by the `card` event; this is the fallback when the stream broke. */
function watchPending() {
  window.clearTimeout(card.pendingTimer);
  const d = card.data;
  if (d && Object.values(d.qa || {}).some((l) => l.some((e) => e.pending))) card.pendingTimer = window.setTimeout(load, 8000);
}

/** Every opened thread asks for its git in full again, as the plugin's does with each reload. */
function loadOpened() {
  for (const id of card.expanded) fetchDetail(id);
}

/** @param {string} id */
async function fetchDetail(id) {
  const slug = card.slug;
  if (!slug) return;
  const cur = card.details[id] || (card.details[id] = { git: null, busy: false, err: null });
  if (cur.busy) return;
  cur.busy = true;
  try {
    const r = await getJson(`/api/project/${encode(slug)}/thread/${encode(id)}`);
    cur.git = r.git; cur.err = null;
  } catch (e) {
    cur.err = /** @type {Error} */ (e).message;
  }
  cur.busy = false;
  if (card.slug === slug) ctx.render();
}

/** The server learned something about this project. @param {{slug:string}} ev */
export function onCardEvent(ev) {
  if (ev.slug === card.slug) load();
}

/** The «Решить» request moved on (the project is being opened). @param {{slug:string, task:string, stage:string}} ev */
export function onSolveEvent(ev) {
  if (ev.slug === card.slug && card.solving && card.solving.task === ev.task) {
    card.solving.stage = ev.stage;
    ctx.render();
  }
}

// ---------------------------------------------------------------------------
// What is on screen

/** @returns {{todo:Row[], done:Row[], threads:number, allTodo:number, allDone:number}} */
function model() {
  const d = card.data;
  /** @type {Row[]} */ const todo = [];
  /** @type {Row[]} */ const done = [];
  let threads = 0, allTodo = 0, allDone = 0;
  if (!d) return { todo, done, threads, allTodo, allDone };
  const q = card.query.trim();
  for (const t of d.threads || []) {
    threads++;
    if (t.closed) allDone++; else allTodo++;
    if (textMatches(t.search, q)) (t.closed ? done : todo).push({ kind: 't', key: t.id, t });
  }
  for (const k of d.tasks || []) {
    if (k.done) allDone++; else allTodo++;
    if (textMatches(k.full, q)) (k.done ? done : todo).push({ kind: 'k', key: `k${k.idx}`, k });
  }
  return { todo, done, threads, allTodo, allDone };
}

/** The rows of the open tab. */
function rows() {
  const m = model();
  return card.tab === 'done' ? m.done : m.todo;
}

/** @returns {Task|null} */
function pickedTask() {
  const d = card.data;
  if (!card.picked || !d) return null;
  return (d.tasks || []).find((k) => k.full === card.picked && !k.done) || null;
}

function dropPick() {
  card.picked = null;
  card.asking = false;
  card.removing = false;
  card.profileOpen = false;
  card.profile = '';
}

/** @param {number} delta */
function moveHl(delta) {
  const list = rows();
  if (list.length === 0) { card.hl = null; return; }
  let at = list.findIndex((r) => r.key === card.hl);
  if (at < 0) at = delta > 0 ? -1 : list.length;
  card.hl = list[Math.max(0, Math.min(list.length - 1, at + delta))].key;
  // A picked task would otherwise swallow Enter.
  dropPick();
  ctx.render();
  const el = ctx.root.querySelector('.row.hl');
  if (el) el.scrollIntoView({ block: 'nearest' });
}

/** @param {'todo'|'done'} tab */
function setTab(tab) {
  if (card.tab === tab) return;
  card.tab = tab;
  card.hl = null;
  dropPick();
  ctx.render();
}

/** @param {string} id */
function toggleThread(id) {
  card.hl = id;
  if (card.expanded.has(id)) card.expanded.delete(id);
  else { card.expanded.add(id); fetchDetail(id); }
  ctx.render();
}

/** @param {Task} k */
function pick(k) {
  if (card.picked === k.full) return;
  dropPick();
  card.picked = k.full;
  ctx.render();
}

// ---------------------------------------------------------------------------
// Actions

/** @param {string} id */
async function openWorktree(id) {
  const t = (card.data?.threads || []).find((x) => x.id === id);
  if (!t) { ctx.flash('Выбери тред: ↑ ↓ выбирают, W открывает его ворктри.'); return; }
  if (!t.path || !t.wt_exists) { ctx.flash(`Ворктри ${t.id} ${t.path ? 'удалён' : 'у треда нет'}.`); return; }
  try {
    await post(`/api/project/${encode(card.slug || '')}/thread/${encode(id)}/worktree`, {});
    ctx.flash(`Ворктри ${t.id} открыт в herdr`);
  } catch (e) {
    ctx.flash(`Не удалось открыть ворктри: ${/** @type {Error} */ (e).message}`, 8000);
  }
}

async function solve() {
  const k = pickedTask();
  const slug = card.slug;
  if (!k || !slug || card.solving) return;
  card.asking = false;
  card.solving = { task: k.full, stage: 'send' };
  ctx.render();
  try {
    await post(`/api/project/${encode(slug)}/solve`, { task: k.full, profile: card.profile || null });
    if (card.picked === k.full) dropPick();
    ctx.flash(`Задача отправлена координатору (${slug})`);
  } catch (e) {
    ctx.flash(`Координатор не принял задачу: ${/** @type {Error} */ (e).message}`, 10000);
  }
  card.solving = null;
  await load();
  ctx.render();
}

async function removeTask() {
  const k = pickedTask();
  const slug = card.slug;
  if (!k || !slug || !card.removing) return;
  card.removing = false;
  try {
    await post(`/api/project/${encode(slug)}/remove`, { task: k.full });
    if (card.picked === k.full) dropPick();
    ctx.flash(`Просьба убрать задачу отправлена координатору (${slug})`);
  } catch (e) {
    ctx.flash(`Координатор не принял просьбу: ${/** @type {Error} */ (e).message}`, 10000);
  }
  await load();
  ctx.render();
}

async function ask() {
  const k = pickedTask();
  const slug = card.slug;
  const q = card.qtext.trim();
  if (!k || !slug || q === '') return;
  card.qtext = '';
  card.asking = false;
  try {
    await post(`/api/project/${encode(slug)}/ask`, { task: k.full, question: q });
  } catch (e) {
    ctx.flash(`Вопрос не ушёл: ${/** @type {Error} */ (e).message}`, 8000);
  }
  await load();
  ctx.render();
}

function startAsking() {
  if (!pickedTask()) return;
  card.asking = true;
  card.filtering = false;
  ctx.render();
  focusId('cask');
}

function startFilter() {
  card.filtering = true;
  card.asking = false;
  ctx.render();
  focusId('cfilter');
}

/** @param {string} id */
function focusId(id) {
  const input = /** @type {HTMLInputElement|null} */ (ctx.root.querySelector('#' + id));
  if (input) { input.focus(); const n = input.value.length; input.setSelectionRange(n, n); }
}

function toggleProfileMenu() {
  card.profileOpen = !card.profileOpen;
  if (card.profileOpen) loadAssignable();
  ctx.render();
}

async function loadAssignable() {
  const slug = card.slug;
  if (!slug) return;
  if (card.assign && card.assign.slug === slug && card.assign.loaded && !card.assign.error) return;
  try {
    const r = await getJson(`/api/project/${encode(slug)}/assignable`);
    card.assign = { slug, profiles: r.profiles || [], error: r.error || null, loaded: true };
  } catch (e) {
    card.assign = { slug, profiles: [], error: /** @type {Error} */ (e).message, loaded: true };
  }
  if (card.slug === slug) ctx.render();
}

// ---------------------------------------------------------------------------
// Keys

/** The filter or the question field has the keyboard. */
export function typing() { return card.filtering || card.asking; }

/**
 * Keys inside the card's fields. @param {KeyboardEvent} ev @param {HTMLElement} target
 * @returns {boolean} whether the event was handled
 */
export function inputKey(ev, target) {
  const k = ev.key;
  if (target.id === 'cask') {
    if (k === 'Enter') { ask(); return true; }
    if (k === 'Escape') { card.asking = false; target.blur(); ctx.render(); return true; }
    return false;
  }
  if (target.id === 'cfilter') {
    // ↑ / ↓ belong to the list: they leave the field and move the highlight; the text stays.
    if (k === 'ArrowDown' || k === 'ArrowUp') { card.filtering = false; target.blur(); moveHl(k === 'ArrowDown' ? 1 : -1); return true; }
    if (k === 'Escape' || k === 'Enter') { card.filtering = false; target.blur(); ctx.render(); return true; }
  }
  return false;
}

/**
 * Keys of the card outside the fields. @param {KeyboardEvent} ev
 * @returns {boolean} whether the event was handled (esc is not when there is nothing left to close)
 */
export function key(ev) {
  const k = keyOf(ev);
  if (card.removing) {
    if (k === 'y' || k === 'Y') { removeTask(); return true; }
    if (k === 'n' || k === 'N' || k === 'Escape') { card.removing = false; ctx.render(); return true; }
    return true;
  }
  if (k === 'Escape') {
    if (card.profileOpen) { card.profileOpen = false; ctx.render(); return true; }
    if (card.picked) { dropPick(); ctx.render(); return true; }
    return false;
  }
  if (k === '/') { startFilter(); return true; }
  if (k === 'Tab') { setTab(card.tab === 'todo' ? 'done' : 'todo'); return true; }
  if (k === '1') { setTab('todo'); return true; }
  if (k === '2') { setTab('done'); return true; }
  if (k === 'ArrowDown' || k === 'ArrowUp') { moveHl(k === 'ArrowDown' ? 1 : -1); return true; }
  if (k === 'ArrowLeft') return false;
  const task = pickedTask();
  if (task && card.tab === 'todo') {
    if (k === 'Enter') { solve(); return true; }
    if (k === '?') { startAsking(); return true; }
    if (k === 'x' || k === 'X') { askRemove(task); return true; }
  }
  const cur = rows().find((r) => r.key === card.hl);
  if (k === 'Enter') {
    if (cur?.kind === 't') toggleThread(cur.t.id);
    else if (cur?.kind === 'k' && !cur.k.done) pick(cur.k);
    return true;
  }
  if (k === 'w' || k === 'W') { openWorktree(cur?.kind === 't' ? cur.t.id : ''); return true; }
  if (k === ',') { ctx.flash('Настройки (дешёвый агент, автопроверка, группы) появятся на этапе 3.'); return true; }
  if (k === 'u') { ctx.flash('«Траты агентов» — отдельная утилита, в планировщике появится позже.'); return true; }
  return false;
}

/** @param {Task} task */
function askRemove(task) {
  if (task.thread_led) { ctx.flash('Задачу ведёт тред: её не убрать из панели.'); return; }
  card.removing = true;
  card.asking = false;
  ctx.render();
}

// ---------------------------------------------------------------------------
// Clicks and input events

/**
 * @param {HTMLElement} el the clicked element
 * @returns {boolean} whether the card took the click
 */
export function click(el) {
  const c = el.closest('[data-c]');
  if (!c) return false;
  const a = c.getAttribute('data-c');
  const id = c.getAttribute('data-id') || '';
  if (a === 'tab') setTab(/** @type {any} */ (id));
  else if (a === 'thread') toggleThread(id);
  else if (a === 'task') {
    const k = (card.data?.tasks || []).find((x) => `k${x.idx}` === id);
    card.hl = id;
    if (k && !k.done) pick(k); else ctx.render();
  }
  else if (a === 'worktree') openWorktree(id);
  else if (a === 'solve') solve();
  else if (a === 'profile-toggle') toggleProfileMenu();
  else if (a === 'profile') { card.profile = id; card.profileOpen = false; ctx.render(); }
  else if (a === 'ask') startAsking();
  else if (a === 'remove-ask') { const t = pickedTask(); if (t) askRemove(t); }
  else if (a === 'remove-yes') removeTask();
  else if (a === 'remove-no') { card.removing = false; ctx.render(); }
  else if (a === 'unpick') { dropPick(); ctx.render(); }
  else if (a === 'filter-clear') { card.query = ''; ctx.render(); }
  else return false;
  return true;
}

/** @param {HTMLInputElement} el */
export function input(el) {
  if (el.id === 'cfilter') { card.query = el.value; card.hl = null; ctx.render(); }
  else if (el.id === 'cask') card.qtext = el.value;
}

/** @param {HTMLElement} el @param {boolean} on */
export function focusChange(el, on) {
  if (el.id === 'cfilter' && card.filtering !== on) { card.filtering = on; ctx.render(); }
  else if (el.id === 'cask' && card.asking !== on && on) { card.asking = true; ctx.render(); }
}

// ---------------------------------------------------------------------------
// Rendering

/** @param {string} text @param {string} [cls] */
const span = (text, cls) => (cls ? `<span class="${cls}">${esc(text)}</span>` : esc(text));

/** The line under a thread's title: branch, commits, files, uncommitted work, PR. @param {Thread} t */
function threadLine(t) {
  /** @type {string[]} */ const out = [];
  const sep = () => { if (out.length) out.push(span('  ·  ', 'dim')); };
  if (t.failed && t.closed) return '';
  if (t.failed) {
    out.push(span('✕ сбой запуска', 'error'));
    const e = oneLine(t.error);
    if (e) out.push(span(': ' + clip(e, 110), 'muted'));
    return out.join('');
  }
  if (t.worktree) out.push(span('⎇ ' + t.worktree, 'path'));
  const g = t.git;
  sep();
  if (!t.path) out.push(span('без ворктри', 'muted'));
  else if (!t.wt_exists) out.push(span('ворктри удалён', 'muted'));
  else if (!g) out.push(span('считаю…', 'muted'));
  else if (g.err) out.push(span('git: ' + g.err, 'muted'));
  else if (!g.base) out.push(span('база не найдена', 'warning'));
  else {
    const ahead = g.ahead || 0, dirty = g.dirty || 0;
    if (ahead === 0 && g.files === 0 && dirty === 0) out.push(span('изменений нет', 'muted'));
    else {
      out.push(span(`${ahead} ${plural(ahead, 'коммит', 'коммита', 'коммитов')}`, ahead > 0 ? 'strong' : 'muted'));
      sep();
      if (g.files > 0) out.push(esc(`${g.files} ${plural(g.files, 'файл', 'файла', 'файлов')} `) + span(`+${g.add}`, 'success') + ' ' + span(`−${g.del}`, 'error'));
      else out.push(span('файлов не изменено', 'muted'));
      sep();
      out.push(dirty > 0 ? span(`● не закоммичено: ${dirty}`, 'warning') : span('всё закоммичено', 'muted'));
    }
  }
  const pr = prLabel(t);
  if (pr) { sep(); out.push(span(pr[0], pr[1])); }
  return out.join('');
}

/** «PR #12 слит» and its tone, or null. @param {Thread} t @returns {[string,string]|null} */
function prLabel(t) {
  if (!t.pr) return null;
  const word = PR_WORD[t.pr_state] || (t.pr_state ? t.pr_state.toLowerCase() : '');
  return [`PR${t.pr_number ? ` #${t.pr_number}` : ''}${word ? ` ${word}` : ''}`, PR_TONE[t.pr_state] || 'muted'];
}

/** The model badges as html, each followed by a gap. @param {Models} m */
function badges(m) {
  return m.badges.map((b) => h('span', { class: `mdl ${b.quiet ? 'quiet' : 'fam-' + b.family}`, title: b.quiet ? 'по профилю: подписей в коммитах нет' : 'по коммитам и профилю' }, esc(b.label))).join('');
}

/** @param {Thread} t */
function threadRow(t) {
  const hl = card.hl === t.id;
  const opened = card.expanded.has(t.id);
  let icon, tone, value = '';
  if (t.failed && !t.closed) { icon = '🐞'; tone = 'error'; value = span('сбой запуска', 'error'); }
  else if (t.failed) { icon = '✕'; tone = 'muted'; value = span('✕ не запустился', 'dim'); }
  else if (t.closed) {
    icon = '✔'; tone = 'success';
    value = badges(t.models) + (t.summary ? span('итог', 'muted') : span('без отчёта', 'dim'));
  } else {
    [icon, tone] = GROUP_META[t.group] || ['○', 'muted'];
    value = badges(t.models) + span(t.state, tone);
    if (t.percent != null && t.group === 'Working') value += span(` ~${t.percent}%`, 'muted');
    if (t.note) value += span(` · ${t.note}`, 'muted');
  }
  value += span(opened ? ' ▾' : ' ▸', 'muted');
  if (hl) {
    icon = '➜'; tone = 'accent';
    value += `<span class="hint">${opened ? 'Свернуть' : 'Раскрыть'} <kbd>↵</kbd>${t.wt_exists ? ' · Ворктри <kbd>W</kbd>' : ''}</span>`;
  }
  const line = threadLine(t);
  return h('div', { class: `row thread${hl ? ' hl' : ''}`, 'data-c': 'thread', 'data-id': t.id },
    h('span', { class: `icon ${tone}` }, icon)
    + h('span', { class: 'label' }, h('span', { class: 'num' }, esc(t.id)) + '  ' + esc(t.title))
    + h('span', { class: 'value' }, value)
    + (line ? h('span', { class: 'detail' }, line) : ''));
}

/** @param {Task} k */
function taskRow(k) {
  const d = /** @type {CardData} */ (card.data);
  const hl = card.hl === `k${k.idx}`;
  if (k.done) {
    return h('div', { class: `row task done${hl ? ' hl' : ''}`, 'data-c': 'task', 'data-id': `k${k.idx}` },
      h('span', { class: `icon ${hl ? 'accent' : 'success'}` }, hl ? '➜' : '✔')
      + h('span', { class: 'label muted' }, esc(k.text)) + h('span', { class: 'value muted' }, 'TASKS.md'));
  }
  const picked = card.picked === k.full;
  const asked = (d.qa[k.full] || []).length;
  const mark = d.sent[k.full];
  let value = span('TASKS.md', 'muted');
  if (asked > 0) value = span(`${asked} вопр.`, 'info');
  if (mark === 'remove') value = span('просьба убрать отправлена', 'info');
  else if (mark) value = span('отправлено координатору', 'info');
  else if (picked) value = span('выбрана', 'accent');
  if (hl && !picked) value += `<span class="hint">Выбрать <kbd>↵</kbd></span>`;
  return h('div', { class: `row task${hl || picked ? ' hl' : ''}`, 'data-c': 'task', 'data-id': `k${k.idx}` },
    h('span', { class: `icon ${hl || picked ? 'accent' : ''}` }, hl || picked ? '➜' : '○')
    + h('span', { class: `label${picked ? ' strong' : ''}` }, esc(k.text)) + h('span', { class: 'value' }, value));
}

/** The block under an opened thread: where it lives, what it changed, what is not committed. @param {Thread} t */
function threadBlock(t) {
  const det = card.details[t.id];
  const g = det?.git || null;
  /** @type {[string,string][]} */ const kv = [];
  const add = (/** @type {string} */ k, /** @type {string} */ html) => kv.push([k, html]);
  if (t.summary) add('Итог', esc(t.summary));
  if (t.failed && t.error) add('Сбой запуска', span(clip(oneLine(t.error), 600), t.closed ? 'muted' : 'error'));
  add('Статус', esc(t.closed ? 'сделан' : t.state) + (t.note ? span(` · ${t.note}`, 'muted') : '') + (t.percent != null && !t.closed ? span(` · ~${t.percent}%`, 'muted') : '') + (t.activity && !t.closed ? span(` · ${t.activity}`, 'muted') : ''));
  if (t.branch) add('Ветка', span(t.branch, 'path'));
  if (g && g.base) add('База', span(g.base, 'path'));
  if (t.path) add('Ворктри', span(t.path, 'path') + (t.wt_exists ? '' : span('  удалён', 'muted')));
  const pr = prLabel(t);
  if (pr) add('PR', (/^https?:\/\//.test(t.pr) ? h('a', { href: t.pr, target: '_blank', rel: 'noopener' }, esc(t.pr)) : span(t.pr, 'path')) + span('  ' + pr[0], pr[1]));
  if (!t.failed) {
    if (t.models.launch_parts.length) add('Запуск', esc(t.models.launch_parts.join(' · ')));
    if (t.models.badges.length) {
      const why = t.models.source === 'commits' ? `по коммитам (${t.models.src || 'Co-Authored-By'})`
        : t.closed ? 'по профилю: подписей в коммитах нет' : 'по профилю; смена через /model не видна';
      add('Модель', badges(t.models) + span(why, 'muted'));
    }
  }
  let body = h('div', { class: 'kv' }, kv.map(([k, v]) => h('span', { class: 'k' }, esc(k)) + h('span', {}, v)).join(''));
  const heading = (/** @type {string} */ s) => h('div', { class: 'strong' }, esc(s));
  if (t.wt_exists && !t.failed) {
    if (!det || (det.busy && !g)) body += h('div', { class: 'muted' }, '◌ Читаю git…');
    else if (det.err) body += h('div', { class: 'muted' }, esc('git: ' + det.err));
    else if (!g) body += h('div', { class: 'muted' }, '◌ Читаю git…');
    else if (g.err) body += h('div', { class: 'muted' }, esc('git: ' + g.err));
    else {
      if (g.stat.length) {
        body += heading(`Изменения против ${g.base || 'базы'}`);
        // The last stat line is the total; keep it when the file list is cut.
        const shown = g.stat.length > 13 ? 12 : g.stat.length;
        for (let i = 0; i < shown; i++) body += h('div', { class: 'path line' }, esc(g.stat[i]));
        if (shown < g.stat.length) {
          body += h('div', { class: 'muted line' }, `… ещё ${g.stat.length - shown - 1} ${plural(g.stat.length - shown - 1, 'файл', 'файла', 'файлов')}`);
          body += h('div', { class: 'muted line' }, esc(g.stat[g.stat.length - 1]));
        }
      }
      if (g.log.length) {
        body += heading('Последние коммиты');
        for (const c of g.log) body += h('div', { class: 'line' }, h('span', { class: 'num' }, esc(c.hash)) + '  ' + esc(clip(c.subject, 140)));
      }
      if (g.wt.length) {
        body += heading('Не закоммичено');
        for (const l of g.wt) body += h('div', { class: 'path line' }, esc(l));
      }
      if (!g.stat.length && !g.log.length && !g.wt.length) body += h('div', { class: 'muted' }, 'Изменений нет.');
    }
    body += h('div', { class: 'actions' }, h('span', { class: 'badge accent', 'data-c': 'worktree', 'data-id': t.id }, 'Открыть ворктри') + '<kbd>w</kbd>');
  } else if (t.path && !t.failed) {
    body += h('div', { class: 'muted' }, 'Ворктри удалён: изменений не видно.');
  } else if (!t.failed) {
    body += h('div', { class: 'muted' }, 'У треда нет своего ворктри.');
  }
  return h('div', { class: 'outcome', 'data-thread': t.id }, body);
}

function panelHtml() {
  const d = /** @type {CardData} */ (card.data);
  const k = pickedTask();
  if (!k || card.tab !== 'todo') return '';
  const qa = d.qa[k.full] || [];
  let body = h('div', {}, span('Задача: ', 'muted') + esc(k.full));
  if (k.notes) body += h('div', { class: 'notes' }, esc(k.notes));
  if (card.removing) {
    body += h('div', { class: 'actions' }, h('strong', {}, 'Убрать из бэклога?')
      + h('span', { class: 'badge warn', 'data-c': 'remove-yes' }, 'Да') + '<kbd>y</kbd>'
      + h('span', { class: 'badge', 'data-c': 'remove-no' }, 'Нет') + '<kbd>n</kbd>'
      + span('координатор уберёт строку из TASKS.md', 'muted'));
  } else if (card.solving && card.solving.task === k.full) {
    body += h('div', { class: 'muted' }, `◌ ${card.solving.stage === 'open' ? 'Открываю проект…' : 'Отправляю координатору…'}`);
  } else {
    const profile = card.profile ? `профиль: ${card.profile} ▾` : 'профиль ▾';
    let actions = h('span', { class: 'badge accent', 'data-c': 'solve' }, 'Решить') + '<kbd>↵</kbd>'
      + h('span', { class: `badge${card.profileOpen ? ' active' : ''}`, 'data-c': 'profile-toggle' }, esc(profile))
      + h('span', { class: 'badge info', 'data-c': 'ask' }, 'Задать вопрос') + '<kbd>?</kbd>';
    if (!k.thread_led) actions += h('span', { class: 'badge', 'data-c': 'remove-ask' }, 'Не решать') + '<kbd>x</kbd>';
    if (qa.some((e) => e.a)) actions += span(`«Решить» передаст и ${qa.filter((e) => e.a).length} вопр.`, 'muted');
    actions += h('span', { class: 'grow' }, '') + h('span', { class: 'badge', 'data-c': 'unpick', title: 'Снять выбор (esc)' }, '✕');
    body += h('div', { class: 'actions' }, actions);
    if (card.profileOpen) body += h('div', { class: 'actions options' }, profileOptions());
  }
  if (card.asking) {
    body += `<input id="cask" type="text" class="ask" autocomplete="off" placeholder="${esc(`вопрос по задаче — ответит ${d.agent} по файлам проекта и репозитория`)}" value="${esc(card.qtext)}">`;
  }
  for (const e of qa) {
    let ans;
    if (e.pending) ans = h('div', { class: 'muted' }, `<span class="spin">◌</span> ${esc(d.agent)} думает…`);
    else if (e.error) ans = h('div', {}, span('Нет ответа: ', 'error') + span(e.error, 'muted'));
    else ans = h('div', { class: 'md' }, markdown(e.a || ''));
    body += h('div', { class: 'qa' }, h('div', {}, span('? ', 'info strong') + span(e.q, 'strong')) + ans);
  }
  return h('div', { class: 'panel' }, body);
}

function profileOptions() {
  const opt = (/** @type {string} */ value, /** @type {string} */ label) =>
    h('span', { class: `badge${card.profile === value ? ' active' : ''}`, 'data-c': 'profile', 'data-id': value }, esc((card.profile === value ? '● ' : '') + label));
  let out = opt('', 'на выбор координатора');
  const a = card.assign;
  for (const p of a?.profiles || []) out += opt(p, p);
  if (!a || !a.loaded) out += span('профили загружаются…', 'muted');
  else if (a.error) out += span('список профилей недоступен: ' + a.error, 'muted');
  return out;
}

export function html() {
  const slug = card.slug || '';
  const d = card.data;
  const snapProject = ctx.project(slug);
  const p = snapProject || d?.project;
  const name = p ? p.name : slug;
  let out = h('div', { class: 'header' },
    h('span', { class: 'badge back', 'data-act': 'back' }, '← Назад')
    + h('div', { class: 'title' }, h('strong', {}, esc(name)) + (p && p.status && p.status !== 'active' ? span('  ' + (STATUS_LABEL[p.status] || p.status), 'muted') : ''))
    + h('span', { class: 'badge', 'data-act': 'settings', title: 'Настройки (на этапе 3)' }, '⚙')
    + h('span', { class: 'badge', 'data-act': 'focus' }, 'Фокус <kbd>F</kbd>')
    + h('span', { class: 'badge accent', 'data-act': 'open' }, 'Открыть <kbd>O</kbd>')
    + h('span', { class: 'badge', 'data-act': 'usage' }, 'Траты')
    + h('span', { class: 'stamp', 'data-act': 'refresh' }, esc(ctx.stamp())));
  // Goal and repositories.
  if (p) {
    let kv = '';
    if (p.goal) kv += h('span', { class: 'k' }, 'Цель') + h('span', {}, esc(p.goal));
    const repos = p.repos || [];
    if (repos.length) kv += h('span', { class: 'k' }, repos.length === 1 ? 'Репозиторий' : 'Репозитории')
      + h('span', {}, repos.map((/** @type {{path:string, machine:string}} */ r) => span(r.path, 'path') + (r.machine ? span(` @${r.machine}`, 'muted') : '')).join('<br>'));
    if (kv) out += h('div', { class: 'kv' }, kv);
  }
  if (!d) return out + h('div', { class: 'note' }, '◌ Читаю треды и задачи…');
  if (d.error) return out + h('div', { class: 'note error' }, esc('Не удалось получить треды: ' + d.error));
  const m = model();
  const all = m.allTodo + m.allDone;
  if (all === 0) return out + h('div', { class: 'note' }, 'Ни тредов, ни задач в TASKS.md.');
  out += h('div', { class: 'progress' },
    h('div', { class: 'meter' }, `<div class="bar" style="width:${Math.round((m.allDone / all) * 100)}%"></div>`)
    + h('span', {}, h('span', { class: 'strong num' }, String(m.allDone)) + span(` из ${all} сделано  ·  ${m.threads} ${plural(m.threads, 'тред', 'треда', 'тредов')}`, 'muted')));
  const shown = m.todo.length + m.done.length;
  out += h('div', { class: 'controls' },
    h('div', { class: 'filter' }, `<kbd>/</kbd><input id="cfilter" type="search" placeholder="фильтр: слово, модель, профиль, «сбой запуска»" value="${esc(card.query)}" autocomplete="off">`
      + (card.query.trim() ? span(`${shown} из ${all}`, 'muted') + h('span', { class: 'badge', 'data-c': 'filter-clear' }, '✕') : '')));
  out += h('div', { class: 'tabs' },
    h('span', { class: `tab${card.tab === 'todo' ? ' active' : ''}`, 'data-c': 'tab', 'data-id': 'todo' }, 'Сделать ' + span(String(m.todo.length), 'muted'))
    + h('span', { class: `tab${card.tab === 'done' ? ' active' : ''}`, 'data-c': 'tab', 'data-id': 'done' }, 'Сделано ' + span(String(m.done.length), 'muted')));
  const list = card.tab === 'done' ? m.done : m.todo;
  let items = '';
  if (list.length === 0) {
    const q = card.query.trim();
    items = h('div', { class: q ? 'note' : 'note ' + (card.tab === 'done' ? '' : 'success') }, esc(q ? `Ничего не найдено по «${q}».` : card.tab === 'done' ? 'Пока ничего не сделано.' : 'Всё сделано.'));
  }
  for (const r of list) {
    items += r.kind === 't' ? threadRow(r.t) : taskRow(r.k);
    if (r.kind === 't' && card.expanded.has(r.t.id)) items += threadBlock(r.t);
  }
  out += h('div', { class: 'list scroll', 'data-scroll': `card-${card.tab}` }, items);
  out += panelHtml();
  return out;
}

/** The dock's key hints. @param {(keys:string[], text:string)=>string} hint */
export function dock(hint) {
  if (card.asking) return hint(['↵'], 'спросить') + hint(['esc'], 'убрать поле');
  if (card.filtering) return hint(['↵'], 'готово') + hint(['esc'], 'выйти из фильтра');
  if (card.removing) return hint(['y'], 'убрать') + hint(['n'], 'оставить в бэклоге');
  if (card.picked) return hint(['↵'], 'решить') + hint(['?'], 'задать вопрос') + hint(['x'], 'не решать') + hint(['esc'], 'снять выбор');
  return hint(['↑', '↓'], 'строка') + hint(['↵'], 'раскрыть / выбрать') + hint(['w'], 'ворктри') + hint(['/'], 'фильтр')
    + hint(['tab'], 'сделать / сделано') + hint(['o'], 'открыть') + hint(['f'], 'фокус') + hint(['r'], 'обновить') + hint(['esc'], 'назад');
}
