// @ts-check
// The «Новый проект» form: folder (with a folder browser), machine, name, goal,
// more repositories, the thread and coordinator profiles — what
// `herdr-projects new` takes. Behaviour and keys follow the Tern plugin (0.17.1).
//
// Nothing here creates anything by itself: «Создать» posts the form to the
// server, which runs `herdr-projects new`; the page then opens the project.

import { esc, h, post, getJson, keyOf } from './util.js';

/**
 * @typedef {{path:string, machine:string, machOpen:boolean}} RepoRow
 * @typedef {{all:string[], threads:string[], coordinators:string[], thread_default:string|null, coordinator_default:string|null}} Profiles
 * @typedef {{machines:string[]|null, profiles:Profiles|null}} Options
 * @typedef {{name:string, repo:boolean}} Entry
 * @typedef {{path:string, parent:string|null, entries:Entry[], hl:number, loading:boolean, err:string|null, seq:number}} Browse
 */

/** What the page gives the form: rendering, the status line, the root, a created project, closing. @type {{ render:()=>void, flash:(msg:string, ms?:number)=>void, root:HTMLElement, created:(slug:string, warnings:string[])=>void, close:()=>void }} */
let ctx;

/** @param {typeof ctx} c */
export function init(c) { ctx = c; }

const MAX_REPOS = 6;
const BROWSE_ROWS = 20;

/** @returns {RepoRow} */
const blankRepo = () => ({ path: '', machine: '', machOpen: false });

const f = {
  /** @type {RepoRow[]} */ repos: [blankRepo()],
  name: '',
  goal: '',
  nameAuto: true,
  tprof: '',
  cprof: '',
  /** @type {string|null} */ err: null,
  /** @type {string|null} */ hint: null,
  busy: false,
  /** `undefined` until read, `false` when the read failed. @type {Options|false|undefined} */ opts: undefined,
  /** @type {Browse|null} */ browse: null,
};

/** The form is open (a field may have the cursor). */
export function typing() { return true; }

export function open() {
  Object.assign(f, { repos: [blankRepo()], name: '', goal: '', nameAuto: true, tprof: '', cprof: '', err: null, hint: null, busy: false, opts: undefined, browse: null });
  getJson('/api/new/options')
    .then((o) => { f.opts = o; ctx.render(); })
    .catch(() => { f.opts = false; ctx.render(); });
  window.setTimeout(() => focusField('nf-path'), 0);
}

/** @param {string} id */
function focusField(id) {
  const el = /** @type {HTMLInputElement|null} */ (ctx.root.querySelector('#' + id));
  if (el) { el.focus(); const n = el.value.length; el.setSelectionRange(n, n); }
}

/** All the form's fields in tab order. */
function fieldIds() {
  return [...ctx.root.querySelectorAll('input[id^="nf-"]')].map((e) => e.id);
}

/** @param {string} id @param {number} delta */
function stepField(id, delta) {
  const ids = fieldIds();
  const at = ids.indexOf(id);
  if (at >= 0 && ids[at + delta]) focusField(ids[at + delta]);
}

const pathId = (/** @type {number} */ i) => (i === 0 ? 'nf-path' : `nf-path${i + 1}`);
const machId = (/** @type {number} */ i) => `nf-mach${i + 1}`;

/** @param {string} path «all_gis_services» → «All Gis Services» */
function nameFromPath(path) {
  const base = path.replace(/\/+$/, '').split('/').pop() || '';
  return base.split(/[_\-.\s]+/).filter(Boolean).map((w) => (/^[\x00-\x7f]/.test(w) ? w[0].toUpperCase() + w.slice(1) : w)).join(' ');
}

const remote = (/** @type {RepoRow} */ r) => r.machine.trim() !== '';

// ---------------------------------------------------------------------------
// The folder browser

/** Opens the browser at the field's folder (or where the server finds a start). */
function openBrowse() {
  const row = f.repos[0];
  if (remote(row)) return;
  f.browse = { path: '', parent: null, entries: [], hl: 0, loading: true, err: null, seq: 0 };
  scan(row.path, true, null);
  ctx.render();
}

/** @param {string} path @param {boolean} start @param {string|null} want the folder just left */
async function scan(path, start, want) {
  const b = f.browse;
  if (!b) return;
  const seq = ++b.seq;
  b.loading = true;
  try {
    const r = await getJson(`/api/browse?path=${encodeURIComponent(path)}${start ? '&start=1' : ''}`);
    if (f.browse !== b || b.seq !== seq) return;
    Object.assign(b, { path: r.path, parent: r.parent, entries: r.entries, loading: false, err: null });
    const rows = browseRows(b);
    const at = want ? rows.findIndex((x) => x.name === want) : -1;
    b.hl = at >= 0 ? at : (rows[0] && rows[0].up ? (rows[1] ? 1 : 0) : 0);
  } catch (e) {
    if (f.browse !== b || b.seq !== seq) return;
    b.loading = false;
    if (start || !b.path) { f.browse = null; f.err = /** @type {Error} */ (e).message; } else b.err = /** @type {Error} */ (e).message;
  }
  ctx.render();
}

/** @param {Browse} b @returns {{up?:boolean, name?:string, repo?:boolean}[]} */
function browseRows(b) {
  /** @type {{up?:boolean, name?:string, repo?:boolean}[]} */ const rows = [];
  if (b.parent) rows.push({ up: true });
  for (const e of b.entries) rows.push(e);
  return rows;
}

/** @param {string} dir @param {string} name */
const joinDir = (dir, name) => (dir === '/' ? '' : dir) + '/' + name;

function browseUp() {
  const b = f.browse;
  if (!b || !b.parent) return;
  const left = b.path.replace(/\/+$/, '').split('/').pop() || null;
  scan(b.parent, false, left);
  ctx.render();
}

/** @param {string} name */
function browseEnter(name) {
  const b = f.browse;
  if (!b) return;
  scan(joinDir(b.path, name), false, null);
  ctx.render();
}

/** Takes `path` into the «Папка» field, the name from its folder, and moves to the name. @param {string} path */
function browsePick(path) {
  f.repos[0].path = path;
  if (f.nameAuto || f.name.trim() === '') { f.name = nameFromPath(path); f.nameAuto = true; }
  f.browse = null;
  f.hint = null;
  ctx.render();
  focusField('nf-name');
}

/** Keys while the browser is open: it takes the cursor keys, the rest is ignored. @param {KeyboardEvent} ev @param {string} k */
function browseKey(ev, k) {
  const b = f.browse;
  if (!b) return false;
  const rows = browseRows(b);
  const row = rows[b.hl];
  if (k === 'Escape') { f.browse = null; ctx.render(); focusField('nf-path'); return true; }
  if (k === 'Tab') { f.browse = null; ctx.render(); return false; }
  if (k === 'ArrowDown') b.hl = Math.min(rows.length - 1, b.hl + 1);
  else if (k === 'ArrowUp') b.hl = Math.max(0, b.hl - 1);
  else if (k === 'ArrowRight') { if (row && !row.up && row.name) browseEnter(row.name); return true; }
  else if (k === 'ArrowLeft') { browseUp(); return true; }
  else if (k === 'Enter') {
    if (row && row.up) browseUp();
    else if (row && row.name) browsePick(joinDir(b.path, row.name));
    return true;
  } else return ev.key.length === 1;
  ctx.render();
  ctx.root.querySelector('.brow.hl')?.scrollIntoView({ block: 'nearest' });
  return true;
}

// ---------------------------------------------------------------------------
// Machines, repositories, profiles

/** The next machine after `current`: «локально» → m1 → m2 → … → «локально». @param {string[]} list @param {string} current */
function nextOf(list, current) {
  if (current === '') return list[0] ?? '';
  const i = list.indexOf(current);
  return i >= 0 ? (list[i + 1] ?? '') : '';
}

/** @param {number} i */
function cycleMachine(i) {
  const row = f.repos[i];
  const machines = f.opts ? f.opts.machines : null;
  if (machines && machines.length > 0) {
    row.machine = nextOf(machines, row.machine.trim());
    row.machOpen = false;
    if (i === 0 && remote(row)) f.browse = null;
    ctx.render();
    return;
  }
  // No list: the name is typed.
  row.machOpen = !row.machOpen;
  if (!row.machOpen) row.machine = '';
  ctx.render();
  if (row.machOpen) focusField(machId(i));
}

function addRepo() {
  if (f.repos.length >= MAX_REPOS) { ctx.flash(`Репозиториев не больше ${MAX_REPOS}.`); return; }
  f.repos.push(blankRepo());
  ctx.render();
  focusField(pathId(f.repos.length - 1));
}

/** @param {number} i */
function dropRepo(i) {
  if (i === 0) return;
  f.repos.splice(i, 1);
  ctx.render();
}

/** @param {'tprof'|'cprof'} which */
function cycleProfile(which) {
  const p = f.opts ? f.opts.profiles : null;
  if (!p) return;
  const list = which === 'tprof' ? p.threads : p.coordinators;
  f[which] = nextOf(list, f[which]);
  ctx.render();
}

// ---------------------------------------------------------------------------
// Submit

async function submit() {
  if (f.busy) return;
  f.err = null;
  f.browse = null;
  f.busy = true;
  ctx.render();
  try {
    const r = await post('/api/new', {
      name: f.name,
      goal: f.goal,
      repos: f.repos.map((x) => ({ path: x.path, machine: x.machine })),
      thread_profile: f.tprof,
      coordinator_profile: f.cprof,
    });
    f.busy = false;
    ctx.created(r.slug, r.warnings || []);
  } catch (e) {
    f.busy = false;
    f.err = /** @type {Error} */ (e).message;
    ctx.render();
  }
}

// ---------------------------------------------------------------------------
// Keys, clicks, input

/**
 * Keys inside the form's fields. @param {KeyboardEvent} ev @param {HTMLElement} target
 * @returns {boolean} whether the event was handled
 */
export function inputKey(ev, target) {
  const k = ev.key;
  if (f.browse) return browseKey(ev, k);
  if (k === 'Escape') { ctx.close(); return true; }
  if (k === 'Enter') { submit(); return true; }
  const id = target.id;
  if (k === 'ArrowDown') {
    if (id === 'nf-path' && !remote(f.repos[0])) openBrowse();
    else stepField(id, 1);
    return true;
  }
  if (k === 'ArrowUp') { stepField(id, -1); return true; }
  if (k === 'Tab' && !ev.shiftKey && /^nf-path\d*$/.test(id)) {
    const i = id === 'nf-path' ? 0 : Number(id.slice(7)) - 1;
    if (remote(f.repos[i])) return false;
    completePath(i, id);
    return true;
  }
  return false;
}

/** Tab in a «Папка» field completes the path; with nothing to complete it moves on. @param {number} i @param {string} id */
async function completePath(i, id) {
  const row = f.repos[i];
  try {
    const r = await getJson(`/api/complete?path=${encodeURIComponent(row.path)}`);
    if (r.unchanged) { f.hint = null; stepField(id, 1); return; }
    if (r.path !== row.path) {
      row.path = r.path;
      if (i === 0 && f.nameAuto) f.name = nameFromPath(r.path);
    }
    const c = /** @type {string[]} */ (r.candidates || []);
    f.hint = c.length > 1 ? c.slice(0, 8).join('  ') + (c.length > 8 ? '  …' : '') : null;
    ctx.render();
    focusField(id);
  } catch {
    stepField(id, 1);
  }
}

/**
 * Keys outside the fields (the cursor is not in the form). @param {KeyboardEvent} ev
 * @returns {boolean}
 */
export function key(ev) {
  const k = keyOf(ev);
  if (f.browse) return browseKey(ev, ev.key);
  if (k === 'Escape') { ctx.close(); return true; }
  if (k === 'Enter') { submit(); return true; }
  return false;
}

/**
 * @param {HTMLElement} el the clicked element
 * @returns {boolean} whether the form took the click
 */
export function click(el) {
  const c = el.closest('[data-n]');
  if (!c) return false;
  const a = c.getAttribute('data-n');
  const i = Number(c.getAttribute('data-i') || 0);
  const name = c.getAttribute('data-name') || '';
  if (a === 'browse-toggle') { if (f.browse) { f.browse = null; ctx.render(); } else openBrowse(); }
  else if (a === 'browse-go') browseEnter(name);
  else if (a === 'browse-up') browseUp();
  else if (a === 'browse-pick') { if (f.browse) browsePick(f.browse.path); }
  else if (a === 'machine') cycleMachine(i);
  else if (a === 'machine-close') { f.repos[i].machOpen = false; f.repos[i].machine = ''; ctx.render(); }
  else if (a === 'repo-add') addRepo();
  else if (a === 'repo-drop') dropRepo(i);
  else if (a === 'tprof') cycleProfile('tprof');
  else if (a === 'cprof') cycleProfile('cprof');
  else if (a === 'submit') submit();
  else if (a === 'cancel') ctx.close();
  else return false;
  return true;
}

/** @param {HTMLInputElement} el */
export function input(el) {
  const id = el.id;
  f.err = null;
  if (id === 'nf-name') { f.name = el.value; f.nameAuto = false; }
  else if (id === 'nf-goal') f.goal = el.value;
  else if (/^nf-path\d*$/.test(id)) {
    const i = id === 'nf-path' ? 0 : Number(id.slice(7)) - 1;
    f.repos[i].path = el.value;
    f.hint = null;
    if (i === 0 && f.nameAuto) { f.name = nameFromPath(el.value); ctx.render(); }
  } else if (/^nf-mach\d+$/.test(id)) {
    const i = Number(id.slice(7)) - 1;
    if (f.repos[i]) f.repos[i].machine = el.value;
  }
}

// ---------------------------------------------------------------------------
// Rendering

/** @param {string} text @param {string} [cls] */
const span = (text, cls) => (cls ? `<span class="${cls}">${esc(text)}</span>` : esc(text));

/** @param {Browse} b */
function browseHtml(b) {
  const rows = browseRows(b);
  const total = rows.length;
  const from = Math.max(0, Math.min(b.hl - Math.floor(BROWSE_ROWS / 2), total - BROWSE_ROWS));
  const to = Math.min(total, from + BROWSE_ROWS);
  let out = h('div', { class: 'brow-head' }, span(b.path || '…', 'path') + (total > BROWSE_ROWS ? span(`  строки ${from + 1}–${to} из ${total}`, 'muted') : ''));
  if (b.loading && total === 0) out += h('div', { class: 'muted' }, '◌ Читаю папку…');
  if (b.err) out += h('div', { class: 'error' }, esc(b.err));
  for (let i = from; i < to; i++) {
    const r = rows[i];
    const hl = i === b.hl;
    out += r.up
      ? h('div', { class: `brow${hl ? ' hl' : ''}`, 'data-n': 'browse-up' }, h('span', { class: 'icon' }, hl ? '➜' : '') + h('span', { class: 'path' }, '..'))
      : h('div', { class: `brow${hl ? ' hl' : ''}`, 'data-n': 'browse-go', 'data-name': r.name || '' },
        h('span', { class: 'icon' }, hl ? '➜' : '') + h('span', {}, span((r.name || '') + '/', 'path') + (r.repo ? span('  ⎇ репозиторий', 'success') : '')));
  }
  if (total === 0 && !b.loading) out += h('div', { class: 'muted' }, 'Подпапок нет.');
  out += h('div', { class: 'actions' }, h('span', { class: 'badge accent', 'data-n': 'browse-pick' }, 'Выбрать эту папку') + '<kbd>↵</kbd>' + span('→ войти · ← вверх · esc закрыть обзор', 'muted'));
  return h('div', { class: 'browse' }, out);
}

/** @param {number} i */
function repoRow(i) {
  const r = f.repos[i];
  const machines = f.opts ? f.opts.machines : null;
  const label = remote(r) ? `@${r.machine.trim()}` : 'локально';
  const hasList = !!machines && machines.length > 0;
  let out = h('div', { class: 'nrow' },
    h('label', { for: pathId(i), class: 'k' }, i === 0 ? 'Папка' : `Папка ${i + 1}`)
    + `<input id="${pathId(i)}" type="text" autocomplete="off" spellcheck="false" placeholder="${remote(r) ? 'путь на машине' : '~/Projects/мой-проект'}" value="${esc(r.path)}"${i === 0 && f.browse ? ' readonly' : ''}>`
    + (i === 0 && !remote(r) ? h('span', { class: `badge${f.browse ? ' active' : ''}`, 'data-n': 'browse-toggle', title: 'Обзор папок (↓)' }, 'Обзор') : '')
    + h('span', { class: `badge${remote(r) ? ' active' : ''}`, 'data-n': 'machine', 'data-i': String(i), title: hasList ? 'Машина: клик перебирает' : 'Машина: имя вводится руками' }, esc(hasList || !r.machOpen ? `Машина: ${label}` : 'Машина…'))
    + (i > 0 ? h('span', { class: 'badge', 'data-n': 'repo-drop', 'data-i': String(i) }, '× убрать') : ''));
  if (r.machOpen && !hasList) {
    out += h('div', { class: 'nrow sub' },
      h('label', { for: machId(i), class: 'k' }, 'Машина')
      + `<input id="${machId(i)}" type="text" autocomplete="off" spellcheck="false" placeholder="имя машины, как в herdr machine list" value="${esc(r.machine)}">`
      + h('span', { class: 'badge', 'data-n': 'machine-close', 'data-i': String(i) }, '× локально'));
  }
  if (i === 0 && f.browse) out += browseHtml(f.browse);
  return out;
}

export function html() {
  let out = h('div', { class: 'header' },
    h('span', { class: 'badge back', 'data-n': 'cancel' }, '← Назад')
    + h('div', { class: 'title' }, h('strong', {}, 'Новый проект')));
  let body = repoRow(0);
  body += h('div', { class: 'nrow' }, h('label', { for: 'nf-name', class: 'k' }, 'Название')
    + `<input id="nf-name" type="text" autocomplete="off" placeholder="как в списке проектов" value="${esc(f.name)}">`);
  body += h('div', { class: 'nrow' }, h('label', { for: 'nf-goal', class: 'k' }, 'Цель')
    + `<input id="nf-goal" type="text" autocomplete="off" placeholder="одной строкой: зачем проект" value="${esc(f.goal)}">`);
  for (let i = 1; i < f.repos.length; i++) body += repoRow(i);
  if (f.hint) body += h('div', { class: 'muted hint-line' }, esc(f.hint));
  body += h('div', { class: 'actions' },
    f.repos.length < MAX_REPOS ? h('span', { class: 'badge', 'data-n': 'repo-add' }, '+ ещё репозиторий') : span(`репозиториев не больше ${MAX_REPOS}`, 'muted'));
  const p = f.opts ? f.opts.profiles : null;
  if (p) {
    const label = (/** @type {string} */ v, /** @type {string|null} */ def) => (v === '' ? `как в настройках${def ? ` (${def})` : ''}` : v);
    body += h('div', { class: 'actions' },
      span('Профили', 'k')
      + h('span', { class: `badge${f.tprof ? ' active' : ''}`, 'data-n': 'tprof', title: 'Профиль тредов проекта' }, esc(`Треды: ${label(f.tprof, p.thread_default)}`))
      + h('span', { class: `badge${f.cprof ? ' active' : ''}`, 'data-n': 'cprof', title: 'Профиль координатора проекта' }, esc(`Коорд.: ${label(f.cprof, p.coordinator_default)}`)));
  } else if (f.opts === undefined) {
    body += h('div', { class: 'muted' }, '◌ Читаю машины и профили…');
  }
  if (f.err) body += h('div', { class: 'note error' }, esc(f.err));
  body += h('div', { class: 'actions' },
    f.busy
      ? h('span', { class: 'muted' }, '<span class="spin">◌</span> Создаю проект…')
      : h('span', { class: 'badge accent', 'data-n': 'submit' }, 'Создать') + '<kbd>↵</kbd>'
        + h('span', { class: 'badge', 'data-n': 'cancel' }, 'Отмена') + '<kbd>esc</kbd>');
  body += h('div', { class: 'note dim' }, 'Создаётся папка проекта в корне herdr-projects и открывается его координатор. PROJECT.md правится только командой herdr-projects.');
  return out + h('div', { class: 'list scroll nform', 'data-scroll': 'new' }, body);
}

/** The dock's key hints. @param {(keys:string[], text:string)=>string} hint */
export function dock(hint) {
  if (f.browse) return hint(['↑', '↓'], 'строка') + hint(['→'], 'войти') + hint(['←'], 'вверх') + hint(['↵'], 'выбрать') + hint(['esc'], 'закрыть обзор') + hint(['tab'], 'дальше');
  return hint(['tab'], 'следующее поле (в «Папке» — дополнить путь)') + hint(['↓'], 'обзор папок / поле ниже') + hint(['↑'], 'поле выше') + hint(['↵'], 'создать') + hint(['esc'], 'отмена');
}
