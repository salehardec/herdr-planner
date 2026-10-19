// @ts-check
// The project card: the tasks of one repo of the herdr-projects board, by the
// stage the board puts them in, and — for the one picked — its details and the
// board's commands (`approve`, `review`, `retry`, `delegate`, `stop`, and a
// reply to the agent). The planner decides nothing: the server offers a
// command only for stages where the board takes it (src/board.rs).
//
// The tasks come with the snapshot (SSE); only the details of the picked task
// are asked for: /api/task?id=…. Commands go to /api/task/action.

import { esc, h, plural, clip, oneLine, textMatches, post, getJson, keyOf, ago } from './util.js';

/**
 * @typedef {{id:string, repo:string, n:number, tag:string, title:string, state:string, kind:string, stage:string, msg:string, app:string, progress:number|null, eta:number|null, pr:string, research:boolean, queue:number|null, review:{pass:boolean|null, tally:string}|null, round:number, bucket:Bucket, label:string, review_failed:boolean, merge_blocked:boolean, at:number, actions:string[]}} Task
 * @typedef {'needs'|'ready'|'failed'|'review'|'working'|'queued'|'todo'|'done'} Bucket
 * @typedef {{severity:string, subject:string, where:string|null, proof:string|null, id:string|null, preexisting:boolean|null}} Finding
 * @typedef {{task:{title:string, stage:string, msg:string, description:string}, failure:{title:string, detail:string, next:string}|null, review:{pass:boolean|null, tally:string, findings:Finding[], summary:string|null}|null, rounds:number, agent:string|null, research:boolean, report:any, plan:{text?:string}|null, pr:string|null, worktree:string|null, branch:string|null, limits:{time?:{used?:string, limit?:string}, tokens?:{used?:number, limit?:string}}|null, queue:{words?:string}|null, after:any, tag:string, actions:string[]}} Details
 */

/** What the page gives the card: rendering, the status line, the settings screen, the project record, the refresh stamp. @type {{ render:()=>void, flash:(msg:string, ms?:number)=>void, root:HTMLElement, openSettings:()=>void, project:(slug:string)=>any, stamp:()=>string }} */
let ctx;

/** @param {typeof ctx} c */
export function init(c) { ctx = c; }

/** The glyph, tone and heading of each stage; the order is the order on the card. */
const BUCKETS = /** @type {[Bucket, string, string, string][]} */ ([
  ['needs', '🔔', 'warning', 'Ждут тебя'],
  ['ready', '◆', 'success', 'Готовы к одобрению'],
  ['failed', '✖', 'error', 'Упали'],
  ['review', '◑', 'info', 'На ревью'],
  ['working', '◐', 'accent', 'В работе'],
  ['queued', '◌', 'muted', 'В очереди'],
  ['todo', '○', 'muted', 'Не начаты'],
  ['done', '✔', 'success', 'Сделано'],
]);
const META = Object.fromEntries(BUCKETS.map(([b, icon, tone, title]) => [b, { icon, tone, title }]));

/** The board's commands: the button, its key, the words for the answer. */
const COMMANDS = /** @type {Record<string,{label:string, key:string, tone:string}>} */ ({
  approve: { label: 'Одобрить', key: 'a', tone: 'accent' },
  anyway: { label: 'Одобрить всё равно', key: 'w', tone: '' },
  review: { label: 'Отправить ревью агенту', key: 'v', tone: '' },
  retry: { label: 'Повторить', key: 'e', tone: 'accent' },
  delegate: { label: 'Запустить', key: 'd', tone: 'accent' },
  tell: { label: 'Ответить', key: 'm', tone: 'info' },
  stop: { label: 'Остановить', key: 'x', tone: '' },
});

export const card = {
  /** @type {string|null} */ slug: null,
  /** @type {'todo'|'done'} */ tab: 'todo',
  query: '',
  filtering: false,
  /** The highlighted task id. @type {string|null} */ hl: null,
  /** The task whose details are open. @type {string|null} */ picked: null,
  /** @type {{id:string, data:Details|null, err:string|null, busy:boolean}|null} */ detail: null,
  /** The command waiting for a yes: `approve`, or `anyway` (the board's `approve --anyway`). @type {'approve'|'anyway'|null} */ confirming: null,
  /** The reply field is open. */ telling: false,
  ttext: '',
  /** The command being run. @type {string|null} */ busy: null,
};

const encode = encodeURIComponent;

// ---------------------------------------------------------------------------
// The tasks on screen

/** @returns {{project:any, tasks:Task[]}} */
function current() {
  const project = card.slug ? ctx.project(card.slug) : null;
  return { project, tasks: project ? project.tasks : [] };
}

/** The tasks of the open tab that match the filter, in the card's order. @returns {Task[]} */
function rows() {
  const { tasks } = current();
  const q = card.query.trim();
  return tasks.filter((t) => (card.tab === 'done' ? t.bucket === 'done' : t.bucket !== 'done')
    && textMatches(`${t.tag} ${t.id} ${t.title} ${t.label} ${t.msg} ${t.app} ${t.pr}`, q));
}

/** @returns {Task|null} */
function pickedTask() {
  if (!card.picked) return null;
  return current().tasks.find((t) => t.id === card.picked) || null;
}

/** Opens the card of `slug`; a project that was open before is forgotten. @param {string} slug */
export function open(slug) {
  if (card.slug !== slug) {
    card.slug = slug;
    card.tab = 'todo';
    card.query = '';
    card.filtering = false;
    card.hl = null;
    dropPick();
  }
}

export function close() {
  card.slug = null;
  card.filtering = false;
  dropPick();
}

function dropPick() {
  card.picked = null;
  card.detail = null;
  card.confirming = null;
  card.telling = false;
  card.ttext = '';
}

/** A new snapshot arrived: a task that is gone drops the pick, the picked one is read again. */
export function onSnapshot() {
  if (!card.slug) return;
  const t = pickedTask();
  // It moved to the other tab (finished, or taken back) with the board's answer: the panel goes with it.
  if (card.picked && (!t || (t.bucket === 'done') !== (card.tab === 'done'))) dropPick();
  else if (card.picked) loadDetail(card.picked);
}

/** @param {string} id */
async function loadDetail(id) {
  const cur = card.detail && card.detail.id === id ? card.detail : (card.detail = { id, data: null, err: null, busy: false });
  if (cur.busy) return;
  cur.busy = true;
  try {
    cur.data = /** @type {Details} */ (await getJson(`/api/task?id=${encode(id)}`));
    cur.err = null;
  } catch (e) {
    cur.err = /** @type {Error} */ (e).message;
  }
  cur.busy = false;
  if (card.picked === id) ctx.render();
}

/** @param {Task} t */
function pick(t) {
  card.hl = t.id;
  if (card.picked !== t.id) {
    dropPick();
    card.picked = t.id;
    loadDetail(t.id);
  }
  ctx.render();
}

/** @param {number} delta */
function moveHl(delta) {
  const list = rows();
  if (list.length === 0) { card.hl = null; return; }
  let at = list.findIndex((r) => r.id === card.hl);
  if (at < 0) at = delta > 0 ? -1 : list.length;
  card.hl = list[Math.max(0, Math.min(list.length - 1, at + delta))].id;
  // An open task would otherwise swallow the keys of the next one.
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

// ---------------------------------------------------------------------------
// The board's commands

/** @param {string} kind @param {{text?:string}} [extra] */
async function runCommand(kind, extra = {}) {
  const t = pickedTask();
  if (!t || card.busy) return;
  if (!t.actions.includes(kind)) { ctx.flash(`${t.tag}: «${COMMANDS[kind]?.label || kind}» на этой стадии не подходит.`); return; }
  card.busy = kind;
  card.confirming = null;
  ctx.render();
  try {
    const r = await post('/api/task/action', { id: t.id, kind, ...extra });
    if (kind === 'tell') { card.telling = false; card.ttext = ''; }
    ctx.flash(`${t.tag}: ${COMMANDS[kind].label.toLowerCase()} — ${clip(oneLine(String(r.message || 'готово')), 140)}`, 6000);
  } catch (e) {
    ctx.flash(`${t.tag}: не вышло — ${/** @type {Error} */ (e).message}`, 10000);
  }
  card.busy = null;
  loadDetail(t.id);
  ctx.render();
}

/** Approving asks first: it merges work (or accepts a plan). @param {Task} t @param {'approve'|'anyway'} kind */
function askApprove(t, kind) {
  if (!t.actions.includes(kind)) { ctx.flash(`${t.tag}: «${COMMANDS[kind].label.toLowerCase()}» сейчас нельзя (${t.label}).`); return; }
  card.confirming = kind;
  card.telling = false;
  ctx.render();
}

function confirmApprove() {
  if (card.confirming) runCommand(card.confirming);
}

function startTelling() {
  const t = pickedTask();
  if (!t || !t.actions.includes('tell')) return;
  card.telling = true;
  card.confirming = null;
  card.filtering = false;
  ctx.render();
  focusId('ctell');
}

function sendTell() {
  const text = card.ttext.trim();
  if (text === '') return;
  runCommand('tell', { text });
}

function startFilter() {
  card.filtering = true;
  card.telling = false;
  ctx.render();
  focusId('cfilter');
}

/** @param {string} id */
function focusId(id) {
  const input = /** @type {HTMLInputElement|null} */ (ctx.root.querySelector('#' + id));
  if (input) { input.focus(); const n = input.value.length; input.setSelectionRange(n, n); }
}

// ---------------------------------------------------------------------------
// Keys

/** The filter or the reply field has the keyboard. */
export function typing() { return card.filtering || card.telling; }

/**
 * Keys inside the card's fields. @param {KeyboardEvent} ev @param {HTMLElement} target
 * @returns {boolean} whether the event was handled
 */
export function inputKey(ev, target) {
  const k = ev.key;
  if (target.id === 'ctell') {
    if (k === 'Enter') { sendTell(); return true; }
    if (k === 'Escape') { card.telling = false; target.blur(); ctx.render(); return true; }
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
  if (card.confirming) {
    if (k === 'y' || k === 'Y') { confirmApprove(); return true; }
    if (k === 'n' || k === 'N' || k === 'Escape') { card.confirming = null; ctx.render(); return true; }
    return true;
  }
  if (k === 'Escape') {
    if (card.picked) { dropPick(); ctx.render(); return true; }
    return false;
  }
  if (k === '/') { startFilter(); return true; }
  if (k === 'Tab') { setTab(card.tab === 'todo' ? 'done' : 'todo'); return true; }
  if (k === '1') { setTab('todo'); return true; }
  if (k === '2') { setTab('done'); return true; }
  if (k === 'ArrowDown' || k === 'ArrowUp') { moveHl(k === 'ArrowDown' ? 1 : -1); return true; }
  if (k === 'ArrowLeft') return false;
  const t = pickedTask();
  if (t) {
    if (k === 'a') { askApprove(t, 'approve'); return true; }
    if (k === 'w') { askApprove(t, 'anyway'); return true; }
    if (k === 'v') { runCommand('review'); return true; }
    if (k === 'e') { runCommand('retry'); return true; }
    if (k === 'd') { runCommand('delegate'); return true; }
    if (k === 'x') { runCommand('stop'); return true; }
    if (k === 'm' || k === '?') { startTelling(); return true; }
  }
  if (k === 'Enter') {
    const cur = rows().find((r) => r.id === card.hl);
    if (cur) { if (card.picked === cur.id) { dropPick(); ctx.render(); } else pick(cur); }
    return true;
  }
  if (k === ',') { ctx.openSettings(); return true; }
  return false;
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
  else if (a === 'task') {
    const t = current().tasks.find((x) => x.id === id);
    if (!t) return true;
    if (card.picked === id) { card.hl = id; dropPick(); ctx.render(); } else pick(t);
  }
  else if (a === 'cmd') {
    if (id === 'approve' || id === 'anyway') { const t = pickedTask(); if (t) askApprove(t, id); }
    else if (id === 'tell') startTelling();
    else runCommand(id);
  }
  else if (a === 'approve-yes') confirmApprove();
  else if (a === 'approve-no') { card.confirming = null; ctx.render(); }
  else if (a === 'tell-send') sendTell();
  else if (a === 'unpick') { dropPick(); ctx.render(); }
  else if (a === 'filter-clear') { card.query = ''; ctx.render(); }
  else return false;
  return true;
}

/** @param {HTMLInputElement} el */
export function input(el) {
  if (el.id === 'cfilter') { card.query = el.value; card.hl = null; ctx.render(); }
  else if (el.id === 'ctell') card.ttext = el.value;
}

/** @param {HTMLElement} el @param {boolean} on */
export function focusChange(el, on) {
  if (el.id === 'cfilter' && card.filtering !== on) { card.filtering = on; ctx.render(); }
  else if (el.id === 'ctell' && card.telling !== on && on) { card.telling = true; ctx.render(); }
}

// ---------------------------------------------------------------------------
// Rendering

/** @param {string} text @param {string} [cls] */
const span = (text, cls) => (cls ? `<span class="${cls}">${esc(text)}</span>` : esc(text));

/** @param {string} pr */
function prTag(pr) {
  if (!pr) return '';
  const n = pr.replace(/\/+$/, '').split('/').pop() || '';
  return /^\d+$/.test(n) ? `PR#${n}` : 'PR';
}

/** The right-hand side of a task row: the stage in words, how far, the review, the pull request, how long. @param {Task} t */
function taskValue(t) {
  const tone = META[t.bucket].tone;
  let v = span(t.label, tone === 'muted' ? 'muted' : `${tone} strong`);
  if ((t.bucket === 'working' || t.bucket === 'review') && t.progress != null) v += span(`  ${t.progress}%`, 'muted');
  if (t.eta != null && (t.bucket === 'working' || t.bucket === 'review')) v += span(` · ещё ${t.eta} мин`, 'muted');
  if (t.review) {
    const rtone = t.bucket === 'done' ? 'muted' : t.bucket === 'review' ? 'info' : t.review.pass === false ? 'error' : t.review.pass === true ? 'success' : 'muted';
    v += span(`  R#${t.round || 1}`, rtone);
  }
  if (t.pr) v += span(`  ${prTag(t.pr)}`, t.bucket === 'done' ? 'muted' : 'info');
  if (t.at && t.bucket !== 'todo') v += span(`  ${ago(t.at)}`, 'muted ago');
  return v;
}

/** @param {Task} t */
function taskRow(t) {
  const hl = card.hl === t.id;
  const open = card.picked === t.id;
  const m = META[t.bucket];
  let value = taskValue(t);
  if (hl && !open) value += `<span class="hint">Подробнее <kbd>↵</kbd></span>`;
  const detail = t.review && t.review.tally && t.bucket !== 'done' ? `ревью: ${t.review.tally}` : '';
  const line = oneLine(t.msg) || detail;
  return h('div', { class: `row task${hl || open ? ' hl' : ''}${t.bucket === 'done' ? ' done' : ''}`, 'data-c': 'task', 'data-id': t.id, title: t.id },
    h('span', { class: `icon ${hl || open ? 'accent' : m.tone}` }, hl || open ? '➜' : m.icon)
    + h('span', { class: `label${open ? ' strong' : ''}` }, h('span', { class: 'num' }, esc(t.tag)) + '  ' + esc(t.title)
      + (t.research ? span('  исследование', 'dim') : ''))
    + h('span', { class: 'value' }, value)
    + (line ? h('span', { class: 'detail' }, esc(clip(line, 180)) + (detail && line !== detail && !line.includes(t.review?.tally || '') ? span('  ·  ' + detail, 'dim') : '')) : ''));
}

/** The review of a task, in words: how it came out and what it found. @param {Details} d */
function reviewBlock(d) {
  const rv = d.review;
  if (!rv) return '';
  const verdict = rv.pass === true ? span('прошло', 'success strong') : rv.pass === false ? span('не прошло', 'error strong') : span('без вердикта', 'muted');
  let out = h('div', {}, span('Ревью: ', 'muted') + verdict + (rv.tally ? span(` · ${rv.tally}`, 'muted') : span(' · замечаний нет', 'muted'))
    + (d.rounds > 1 ? span(` · кругов: ${d.rounds}`, 'muted') : ''));
  const own = (rv.findings || []).filter((f) => !f.preexisting);
  const tone = (/** @type {string} */ s) => (s === 'critical' || s === 'major' ? 'error' : s === 'minor' ? 'warning' : 'muted');
  for (const f of own.slice(0, 8)) {
    out += h('div', { class: 'finding' }, span(f.severity, tone(f.severity) + ' strong') + ' ' + esc(clip(oneLine(f.subject || ''), 220))
      + (f.where ? span('  ' + clip(f.where, 80), 'path') : ''));
  }
  if (own.length > 8) out += h('div', { class: 'muted' }, `… ещё ${own.length - 8} ${plural(own.length - 8, 'замечание', 'замечания', 'замечаний')}`);
  return out;
}

/** The panel of the picked task: what the board says about it and the commands for its stage. @param {Task} t */
function panelHtml(t) {
  const det = card.detail && card.detail.id === t.id ? card.detail : null;
  const d = det ? det.data : null;
  const m = META[t.bucket];
  let body = h('div', {}, span(t.tag, 'num strong') + '  ' + esc(t.title)
    + h('span', { class: 'grow' }, '') + h('span', { class: 'badge', 'data-c': 'unpick', title: 'Закрыть (esc)' }, '✕'));
  /** @type {[string,string][]} */ const kv = [];
  kv.push(['Стадия', span(t.label, `${m.tone} strong`) + span(`  (${t.stage || t.state})`, 'dim')]);
  const msg = oneLine((d && d.task.msg) || t.msg);
  if (msg) kv.push(['Сообщение', esc(msg)]);
  if (t.app) kv.push(['Агент', esc(t.app)]);
  if (d && d.branch) kv.push(['Ветка', span(d.branch, 'path')]);
  if (d && d.worktree) kv.push(['Ворктри', span(d.worktree, 'path')]);
  const pr = (d && d.pr) || t.pr;
  if (pr) kv.push(['PR', /^https?:\/\//.test(pr) ? h('a', { href: pr, target: '_blank', rel: 'noopener' }, esc(pr)) : span(pr, 'path')]);
  if (t.queue) kv.push(['Очередь', esc(`место ${t.queue}`)]);
  if (d && d.limits && d.limits.time && d.limits.time.used) kv.push(['Время агента', esc(d.limits.time.used)]);
  body += h('div', { class: 'kv' }, kv.map(([k, v]) => h('span', { class: 'k' }, esc(k)) + h('span', {}, v)).join(''));
  if (d && d.failure) {
    body += h('div', { class: 'failure' }, h('div', {}, span('Сбой: ', 'error strong') + esc(d.failure.title || ''))
      + (d.failure.detail ? h('div', { class: 'notes' }, esc(clip(d.failure.detail, 900))) : '')
      + (d.failure.next ? h('div', { class: 'muted' }, esc(clip(oneLine(d.failure.next), 300))) : ''));
  }
  if (d) body += reviewBlock(d);
  else if (t.review) body += h('div', {}, span('Ревью: ', 'muted') + (t.review.pass === false ? span('не прошло', 'error') : t.review.pass ? span('прошло', 'success') : '') + (t.review.tally ? span(` · ${t.review.tally}`, 'muted') : ''));
  if (d && d.plan && d.plan.text) body += h('div', { class: 'plan' }, h('div', { class: 'strong' }, 'План') + h('div', { class: 'notes' }, esc(clip(d.plan.text, 1800))));
  if (t.stage === 'pick') body += h('div', { class: 'muted' }, `Варианты выбираются на доске: herdr-projects pick ${esc(t.id)} <буква>`);
  if (det && det.err) body += h('div', { class: 'muted' }, esc('Подробности не прочитались: ' + det.err));
  else if (!d) body += h('div', { class: 'muted' }, '◌ Читаю подробности…');
  body += commandsHtml(t);
  return h('div', { class: 'panel' }, body);
}

/** @param {Task} t */
function commandsHtml(t) {
  if (card.busy) return h('div', { class: 'muted' }, `<span class="spin">◌</span> ${esc(COMMANDS[card.busy]?.label || card.busy)}…`);
  if (card.confirming) {
    const what = t.stage === 'plan' ? 'Принять план? Агент начнёт работу.'
      : card.confirming === 'anyway'
        ? (t.review_failed ? 'Ревью не прошло. Влить всё равно?'
          : t.merge_blocked ? 'Доска не вливает без «всё равно» (проверки PR, слияние или отставание от main). Влить всё равно?'
            : 'Влить всё равно, даже если проверки PR или слияние не пройдут?')
        : 'Одобрить и влить работу?';
    return h('div', { class: 'actions' }, h('strong', {}, esc(`${t.tag}: ${what}`))
      + h('span', { class: 'badge warn', 'data-c': 'approve-yes' }, 'Да') + '<kbd>y</kbd>'
      + h('span', { class: 'badge', 'data-c': 'approve-no' }, 'Нет') + '<kbd>n</kbd>');
  }
  let out = '';
  if (t.actions.length === 0) out += h('div', { class: 'muted' }, t.bucket === 'done' ? 'Задача закончена: команд нет.' : 'Для этой стадии команд нет.');
  else {
    out += h('div', { class: 'actions' }, t.actions.map((a) => {
      const c = COMMANDS[a];
      if (!c) return '';
      const label = a === 'approve' && t.stage === 'plan' ? 'Принять план' : c.label;
      return h('span', { class: `badge${c.tone ? ' ' + c.tone : ''}`, 'data-c': 'cmd', 'data-id': a }, esc(label)) + `<kbd>${c.key}</kbd>`;
    }).join(''));
  }
  if (card.telling) {
    out += `<div class="actions"><input id="ctell" type="text" class="ask" autocomplete="off" placeholder="${esc('что сказать агенту ' + t.tag)}" value="${esc(card.ttext)}">`
      + h('span', { class: 'badge info', 'data-c': 'tell-send' }, 'Отправить') + '<kbd>↵</kbd></div>';
  }
  return out;
}

/** The heading line over a stage's tasks. @param {Bucket} b @param {number} n */
function sectionHtml(b, n) {
  const m = META[b];
  return h('div', { class: 'sect' }, span(`${m.icon} ${m.title}`, m.tone === 'muted' ? 'muted' : m.tone) + span(`  ${n}`, 'muted'));
}

/** The counters of a project: «2 ждут тебя · 1 к одобрению …», nothing for zero. @param {any} c */
export function countersText(c) {
  /** @type {string[]} */ const parts = [];
  if (c.needs) parts.push(`${c.needs} ждут тебя`);
  if (c.ready) parts.push(`${c.ready} к одобрению`);
  if (c.failed) parts.push(`${c.failed} ${plural(c.failed, 'упала', 'упали', 'упали')}`);
  if (c.review) parts.push(`${c.review} на ревью`);
  if (c.working) parts.push(`${c.working} в работе`);
  if (c.queued) parts.push(`${c.queued} в очереди`);
  return parts.join(' · ');
}

export function html() {
  const slug = card.slug || '';
  const { project: p, tasks } = current();
  const name = p ? p.name : slug;
  let out = h('div', { class: 'header' },
    h('span', { class: 'badge back', 'data-act': 'back' }, '← Назад')
    + h('div', { class: 'title' }, h('strong', {}, esc(name)) + (p && p.key ? span('  ' + p.key, 'muted') : ''))
    + h('span', { class: 'badge', 'data-act': 'settings', title: 'Настройки (,)' }, '⚙')
    + h('span', { class: 'badge', 'data-act': 'unfocus', title: 'Боковая панель herdr: снова показать все проекты' }, 'Все <kbd>U</kbd>')
    + h('span', { class: 'stamp', 'data-act': 'refresh' }, esc(ctx.stamp())));
  if (!p) return out + h('div', { class: 'note' }, 'Проекта нет на доске.');
  if (p.path) out += h('div', { class: 'kv' }, h('span', { class: 'k' }, 'Папка') + h('span', {}, span(p.path, 'path')));
  const c = p.counts;
  const total = c.needs + c.ready + c.failed + c.review + c.working + c.queued + c.todo + c.done;
  if (total === 0) return out + h('div', { class: 'note' }, 'На доске нет задач этого репозитория.');
  const live = countersText(c);
  out += h('div', { class: 'progress' },
    h('div', { class: 'meter' }, `<div class="bar" style="width:${Math.round((c.done / total) * 100)}%"></div>`)
    + h('span', {}, h('span', { class: 'strong num' }, String(c.done)) + span(` из ${total} сделано`, 'muted') + (live ? span('  ·  ', 'muted') + esc(live) : '')));
  const list = rows();
  const open = tasks.filter((t) => t.bucket !== 'done').length;
  out += h('div', { class: 'controls' },
    h('div', { class: 'filter' }, `<kbd>/</kbd><input id="cfilter" type="search" placeholder="фильтр: ключ, название, стадия, сообщение" value="${esc(card.query)}" autocomplete="off">`
      + (card.query.trim() ? span(`${list.length} из ${card.tab === 'done' ? c.done : open}`, 'muted') + h('span', { class: 'badge', 'data-c': 'filter-clear' }, '✕') : '')));
  out += h('div', { class: 'tabs' },
    h('span', { class: `tab${card.tab === 'todo' ? ' active' : ''}`, 'data-c': 'tab', 'data-id': 'todo' }, 'В деле ' + span(String(open), 'muted'))
    + h('span', { class: `tab${card.tab === 'done' ? ' active' : ''}`, 'data-c': 'tab', 'data-id': 'done' }, 'Сделано ' + span(String(c.done), 'muted')));
  let items = '';
  if (list.length === 0) {
    const q = card.query.trim();
    items = h('div', { class: q ? 'note' : 'note ' + (card.tab === 'done' ? '' : 'success') }, esc(q ? `Ничего не найдено по «${q}».` : card.tab === 'done' ? 'Пока ничего не сделано.' : 'Всё сделано.'));
  }
  let last = '';
  for (const t of list) {
    if (card.tab === 'todo' && t.bucket !== last) {
      last = t.bucket;
      items += sectionHtml(t.bucket, list.filter((x) => x.bucket === t.bucket).length);
    }
    items += taskRow(t);
  }
  if (card.tab === 'done' && p.done_hidden > 0 && card.query.trim() === '') {
    items += h('div', { class: 'note' }, `Показаны последние ${c.done - p.done_hidden}, ещё ${p.done_hidden} ${plural(p.done_hidden, 'сделана', 'сделаны', 'сделаны')} — на доске.`);
  }
  out += h('div', { class: 'list scroll', 'data-scroll': `card-${card.tab}` }, items);
  const t = pickedTask();
  if (t) out += panelHtml(t);
  return out;
}

/** The dock's key hints. @param {(keys:string[], text:string)=>string} hint */
export function dock(hint) {
  if (card.telling) return hint(['↵'], 'отправить агенту') + hint(['esc'], 'убрать поле');
  if (card.filtering) return hint(['↵'], 'готово') + hint(['esc'], 'выйти из фильтра');
  if (card.confirming) return hint(['y'], 'да') + hint(['n'], 'нет');
  const t = pickedTask();
  if (t) {
    return t.actions.map((a) => hint([COMMANDS[a]?.key || '?'], (COMMANDS[a]?.label || a).toLowerCase())).join('') + hint(['esc'], 'закрыть');
  }
  return hint(['↑', '↓'], 'задача') + hint(['↵'], 'подробнее и команды') + hint(['/'], 'фильтр')
    + hint(['tab'], 'в деле / сделано') + hint(['u'], 'показать все') + hint(['r'], 'обновить') + hint(['esc'], 'назад');
}
