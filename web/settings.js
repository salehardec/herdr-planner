// @ts-check
// The settings screen: one card per setting (the server's table of settings
// drives it), then the project groups — order, rename, delete (the projects
// move to «Прочее» or to another group), «+ Группа» and «↻ Разложить заново».
// Behaviour and keys follow the Tern plugin's settings (0.17.1).

import { esc, h, plural, post, keyOf } from './util.js';

/**
 * @typedef {{key:string, label:string, hint:string, default:string, options:[string,string][]}} SettingDef
 * @typedef {{configured:boolean, list:string[], assign:Record<string,{g:string, auto:boolean}>, collapsed:string[], on:boolean|null}} Groups
 * @typedef {{version:string, projects:{slug:string}[], groups:Groups, settings:Record<string,string>, setting_defs:SettingDef[], group_suggest:{busy:boolean, error:string|null}}} Snap
 */

/** What the page gives the screen: rendering, the status line, the snapshot, closing. @type {{ render:()=>void, flash:(msg:string, ms?:number)=>void, root:HTMLElement, snap:()=>Snap|null, close:()=>void }} */
let ctx;

/** @param {typeof ctx} c */
export function init(c) { ctx = c; }

const OTHER = 'Прочее';

const st = {
  /** The selected row: `s:KEY`, `g:NAME`, `add` or `resort`. */
  sel: '',
  /** The group editor: adding a group or renaming one. @type {{mode:'add'|'rename', name:string, text:string}|null} */
  edit: null,
  /** A group waiting to be deleted and where its projects go ('' = «Прочее»). @type {{name:string, to:string}|null} */
  del: null,
  /** @type {string|null} */ err: null,
};

export function open() {
  st.edit = null;
  st.del = null;
  st.err = null;
  const rows = rowKeys();
  if (!rows.includes(st.sel)) st.sel = rows[0] || '';
}

/** A group is being named. */
export function typing() { return st.edit != null; }

/** @returns {string[]} the rows the keys walk through */
function rowKeys() {
  const s = ctx.snap();
  if (!s) return [];
  return [...s.setting_defs.map((d) => 's:' + d.key), ...s.groups.list.map((n) => 'g:' + n), 'add', 'resort'];
}

/** @param {string} name */
function projectCount(name) {
  const s = ctx.snap();
  if (!s) return 0;
  const g = s.groups;
  return s.projects.filter((p) => {
    const e = g.assign[p.slug];
    const own = e && e.g !== '' && g.list.some((x) => x.toLowerCase() === e.g.toLowerCase()) ? e.g : OTHER;
    return own === name;
  }).length;
}

/** @param {string} msg */
const fail = (msg) => { st.err = msg; ctx.render(); };

/** @param {string} key @param {number} delta */
function cycle(key, delta) {
  const s = ctx.snap();
  const def = s?.setting_defs.find((d) => d.key === key);
  if (!s || !def) return;
  const at = Math.max(0, def.options.findIndex(([v]) => v === s.settings[key]));
  const next = def.options[(at + delta + def.options.length) % def.options.length][0];
  setValue(key, next);
}

/** @param {string} key @param {string} value */
function setValue(key, value) {
  st.err = null;
  post('/api/settings', { key, value }).catch((e) => fail(e.message));
}

/** @param {string} op @param {Record<string, any>} body */
function groupOp(op, body) {
  st.err = null;
  return post('/api/groups', { op, ...body });
}

/** @param {number} delta */
function moveSel(delta) {
  const rows = rowKeys();
  const at = Math.max(0, rows.indexOf(st.sel));
  st.sel = rows[Math.max(0, Math.min(rows.length - 1, at + delta))];
  st.del = null;
  ctx.render();
  ctx.root.querySelector('.sel')?.scrollIntoView({ block: 'nearest' });
}

/** @param {string} name @param {number} delta */
function moveGroup(name, delta) {
  groupOp('move', { name, delta }).then(() => { st.sel = 'g:' + name; }).catch((e) => fail(e.message));
}

/** @param {'add'|'rename'} mode @param {string} [name] */
function startEdit(mode, name = '') {
  st.edit = { mode, name, text: mode === 'rename' ? name : '' };
  st.del = null;
  st.err = null;
  ctx.render();
  const input = /** @type {HTMLInputElement|null} */ (ctx.root.querySelector('#gedit'));
  if (input) { input.focus(); input.setSelectionRange(input.value.length, input.value.length); }
}

function commitEdit() {
  const e = st.edit;
  if (!e) return;
  const call = e.mode === 'rename'
    ? groupOp('rename', { name: e.name, to: e.text }).then((r) => r.group)
    : groupOp('add', { name: e.text }).then((r) => r.group);
  call.then((name) => { st.edit = null; st.sel = 'g:' + name; ctx.render(); }).catch((e2) => fail(e2.message));
}

function stopEdit() {
  st.edit = null;
  st.err = null;
  ctx.render();
}

/** @param {string} name */
function startDelete(name) {
  st.del = { name, to: '' };
  st.edit = null;
  st.err = null;
  ctx.render();
}

/** @param {number} delta */
function stepTarget(delta) {
  const s = ctx.snap();
  if (!st.del || !s) return;
  const targets = ['', ...s.groups.list.filter((g) => g !== st.del?.name)];
  const at = Math.max(0, targets.indexOf(st.del.to));
  st.del.to = targets[(at + delta + targets.length) % targets.length];
  ctx.render();
}

function confirmDelete() {
  const d = st.del;
  if (!d) return;
  const rows = rowKeys();
  const at = rows.indexOf('g:' + d.name);
  st.del = null;
  groupOp('delete', { name: d.name, to: d.to })
    .then(() => {
      const after = rowKeys();
      st.sel = after[Math.max(0, Math.min(at, after.length - 1))];
      ctx.render();
    })
    .catch((e) => fail(e.message));
}

function resort() {
  groupOp('resort', {}).catch((e) => fail(e.message));
}

// ---------------------------------------------------------------------------
// Keys

/**
 * Keys inside the group editor. @param {KeyboardEvent} ev
 * @returns {boolean} whether the event was handled
 */
export function inputKey(ev) {
  if (ev.key === 'Enter') { commitEdit(); return true; }
  if (ev.key === 'Escape') { stopEdit(); return true; }
  return false;
}

/**
 * Keys of the screen outside the field. @param {KeyboardEvent} ev
 * @returns {boolean} whether the event was handled
 */
export function key(ev) {
  const k = keyOf(ev);
  if (st.del) {
    if (k === 'y' || k === 'Y' || k === 'Enter') confirmDelete();
    else if (k === 'n' || k === 'N' || k === 'Escape') { st.del = null; ctx.render(); }
    else if (k === 'ArrowLeft') stepTarget(-1);
    else if (k === 'ArrowRight') stepTarget(1);
    else return false;
    return true;
  }
  if (k === 'Escape') { ctx.close(); return true; }
  const group = st.sel.startsWith('g:') ? st.sel.slice(2) : null;
  const setting = st.sel.startsWith('s:') ? st.sel.slice(2) : null;
  if (ev.shiftKey && (k === 'ArrowUp' || k === 'ArrowDown') && group) { moveGroup(group, k === 'ArrowUp' ? -1 : 1); return true; }
  if (k === 'ArrowDown') { moveSel(1); return true; }
  if (k === 'ArrowUp') { moveSel(-1); return true; }
  if ((k === 'ArrowLeft' || k === 'ArrowRight') && setting) { cycle(setting, k === 'ArrowLeft' ? -1 : 1); return true; }
  if (k === 'Enter') {
    if (setting) cycle(setting, 1);
    else if (group) startEdit('rename', group);
    else if (st.sel === 'add') startEdit('add');
    else if (st.sel === 'resort') resort();
    return true;
  }
  if ((k === '[' || k === ']') && group) { moveGroup(group, k === '[' ? -1 : 1); return true; }
  if ((k === 'x' || k === 'X') && group) { startDelete(group); return true; }
  if (k === 'a' || k === 'A') { startEdit('add'); return true; }
  return false;
}

// ---------------------------------------------------------------------------
// Clicks and input

/**
 * @param {HTMLElement} el the clicked element
 * @returns {boolean} whether the screen took the click
 */
export function click(el) {
  const c = el.closest('[data-s]');
  if (!c) return false;
  const a = c.getAttribute('data-s');
  const name = c.getAttribute('data-name') || '';
  const row = c.closest('[data-row-key]')?.getAttribute('data-row-key');
  if (row && a !== 'close') st.sel = row;
  if (a === 'close') ctx.close();
  else if (a === 'opt') setValue(c.getAttribute('data-key') || '', c.getAttribute('data-value') || '');
  else if (a === 'gup') moveGroup(name, -1);
  else if (a === 'gdown') moveGroup(name, 1);
  else if (a === 'grename') startEdit('rename', name);
  else if (a === 'gdel') startDelete(name);
  else if (a === 'gdel-to') stepTarget(1);
  else if (a === 'gdel-yes') confirmDelete();
  else if (a === 'gdel-no') { st.del = null; ctx.render(); }
  else if (a === 'gadd') startEdit('add');
  else if (a === 'gresort') resort();
  else if (a === 'gedit-save') commitEdit();
  else if (a === 'gedit-cancel') stopEdit();
  else ctx.render();
  return true;
}

/** @param {HTMLInputElement} el */
export function input(el) {
  if (el.id === 'gedit' && st.edit) st.edit.text = el.value;
}

// ---------------------------------------------------------------------------
// Rendering

/** @param {string} text @param {string} [cls] */
const span = (text, cls) => (cls ? `<span class="${cls}">${esc(text)}</span>` : esc(text));

export function html() {
  const s = ctx.snap();
  let out = h('div', { class: 'header' },
    h('span', { class: 'badge back', 'data-s': 'close' }, '← Назад')
    + h('div', { class: 'title' }, h('strong', {}, 'Настройки'))
    + (s ? h('span', { class: 'muted' }, `v${esc(s.version)}`) : ''));
  if (!s) return out + h('div', { class: 'note' }, 'Загрузка…');
  let body = '';
  if (st.edit) {
    body += h('div', { class: 'gedit' },
      span(st.edit.mode === 'rename' ? 'Новое название группы:' : 'Название новой группы:', 'muted')
      + `<input id="gedit" type="text" maxlength="40" autocomplete="off" placeholder="область, например «Личное»" value="${esc(st.edit.text)}">`
      + h('span', { class: 'badge accent', 'data-s': 'gedit-save' }, 'Сохранить') + '<kbd>↵</kbd>'
      + h('span', { class: 'badge', 'data-s': 'gedit-cancel' }, 'Отмена') + '<kbd>esc</kbd>');
  }
  if (st.err) body += h('div', { class: 'note error' }, esc(st.err));
  for (const def of s.setting_defs) {
    const sel = st.sel === 's:' + def.key;
    const opts = def.options.map(([value, label]) => {
      const active = s.settings[def.key] === value;
      return h('span', { class: `badge${active ? ' active' : ''}`, 'data-s': 'opt', 'data-key': def.key, 'data-value': value }, esc((active ? '● ' : '') + label));
    }).join('');
    body += h('div', { class: `card${sel ? ' sel' : ''}`, 'data-row-key': 's:' + def.key },
      h('div', { class: 'strong' }, esc((sel ? '▸ ' : '') + def.label))
      + h('div', { class: 'muted' }, esc(def.hint))
      + h('div', { class: 'actions' }, opts));
  }
  body += groupsCard(s);
  body += h('div', { class: 'note dim' }, 'Настройки сохраняются в state.json и переживают перезапуск сервера.');
  return out + h('div', { class: 'list scroll', 'data-scroll': 'settings' }, body);
}

/** @param {Snap} s */
function groupsCard(s) {
  const list = s.groups.list;
  let body = h('div', { class: 'muted' }, 'Порядок групп — порядок в списке проектов. Удалённая группа отдаёт проекты в «Прочее» или в группу на выбор.');
  list.forEach((name, i) => {
    const on = st.sel === 'g:' + name;
    const n = projectCount(name);
    const btn = (/** @type {string} */ text, /** @type {string} */ act, /** @type {string} */ title) =>
      h('span', { class: 'badge', 'data-s': act, 'data-name': name, title }, esc(text));
    body += h('div', { class: `grow-row${on ? ' sel' : ''}`, 'data-row-key': 'g:' + name },
      h('span', { class: 'gname' }, span(`${on ? '▸ ' : '  '}${i + 1}. ${name}`, on ? 'strong' : '') + span(`  ${n} ${plural(n, 'проект', 'проекта', 'проектов')}`, 'muted'))
      + btn('↑', 'gup', 'Выше ([)') + btn('↓', 'gdown', 'Ниже (])') + btn('✎ Переименовать', 'grename', 'Переименовать (↵)') + btn('✕ Удалить', 'gdel', 'Удалить (X)'));
    if (st.del && st.del.name === name) {
      body += n > 0
        ? h('div', { class: 'actions confirm' },
          span(`В группе ${n} ${plural(n, 'проект', 'проекта', 'проектов')}. Перенести в:`, 'warning')
          + h('span', { class: 'badge accent', 'data-s': 'gdel-to' }, esc((st.del.to === '' ? OTHER : st.del.to) + ' ▾')) + '<kbd>←</kbd><kbd>→</kbd>'
          + h('span', { class: 'badge warn', 'data-s': 'gdel-yes', 'data-name': name }, 'Удалить') + '<kbd>y</kbd>'
          + h('span', { class: 'badge', 'data-s': 'gdel-no' }, 'Отмена') + '<kbd>n</kbd>')
        : h('div', { class: 'actions confirm' },
          span(`Удалить «${name}»? Группа пуста.`, 'warning')
          + h('span', { class: 'badge warn', 'data-s': 'gdel-yes', 'data-name': name }, 'Да') + '<kbd>y</kbd>'
          + h('span', { class: 'badge', 'data-s': 'gdel-no' }, 'Нет') + '<kbd>n</kbd>');
    }
  });
  if (list.length === 0) body += h('div', { class: 'muted' }, 'Групп нет: все проекты в «Прочее».');
  const addOn = st.sel === 'add', resortOn = st.sel === 'resort';
  body += h('div', { class: 'actions' },
    h('span', { class: `badge${addOn ? ' active' : ''}`, 'data-s': 'gadd', 'data-row-key': 'add' }, esc((addOn ? '▸ ' : '') + '+ Группа')) + '<kbd>a</kbd>'
    + h('span', { class: `badge${resortOn ? ' active' : ''}`, 'data-s': 'gresort', 'data-row-key': 'resort' }, esc((resortOn ? '▸ ' : '') + '↻ Разложить заново')));
  body += h('div', { class: 'dim' }, '«Разложить заново» пересчитывает только то, что агент разложил сам («авто»); ручной выбор остаётся.');
  const agent = s.setting_defs.find((d) => d.key === 'agent')?.options.find(([v]) => v === s.settings.agent)?.[1] || 'Агент';
  if (s.group_suggest.busy) body += h('div', { class: 'muted' }, `<span class="spin">◌</span> ${esc(agent)} раскладывает проекты…`);
  else if (s.group_suggest.error) body += h('div', {}, span('Раскладка не удалась: ', 'error') + span(s.group_suggest.error, 'muted'));
  return h('div', { class: `card${st.sel === 'add' || st.sel === 'resort' || st.sel.startsWith('g:') ? ' sel' : ''}` },
    h('div', { class: 'strong' }, 'Группы проектов') + body);
}

/** The dock's key hints. @param {(keys:string[], text:string)=>string} hint */
export function dock(hint) {
  if (st.edit) return hint(['↵'], 'сохранить') + hint(['esc'], 'отмена');
  if (st.del) return hint(['←', '→'], 'куда перенести') + hint(['y'], 'удалить') + hint(['n'], 'отмена');
  const group = st.sel.startsWith('g:');
  return hint(['↑', '↓'], 'строка') + hint(['←', '→', '↵'], 'значение')
    + (group ? hint(['[', ']'], 'порядок') + hint(['↵'], 'переименовать') + hint(['x'], 'удалить') : '')
    + hint(['a'], 'добавить группу') + hint(['esc'], 'назад');
}
