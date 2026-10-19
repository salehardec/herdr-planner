'use strict';
/* Herdr Planner — 3D-мир доски herdr-projects.
   Мир: web/world.js. Данные: GET /api/state; команды: POST /api/action,
   POST /api/chat. Панели и чат — обычный DOM поверх сцены. */

import { createWorld } from './world.js';
import { collectRequests, parseOptions, firstSentence, PERMISSION_ALLOW, PERMISSION_DENY } from './requests.js';

const $ = (id) => document.getElementById(id);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const QS = new URLSearchParams(location.search);
const mq = matchMedia('(prefers-reduced-motion: reduce)');
const pad = (n) => String(n).padStart(2, '0');
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const gl = (n, cls = '') => `<svg class="gl ${cls}" aria-hidden="true"><use href="#g-${n}"/></svg>`;
const localClient = () => ['127.0.0.1', '::1', '[::1]'].includes(location.hostname);

const ST = {
  waiting: { label: 'Ждёт тебя', title: 'Ждёт тебя: вопрос, разрешение или вход' },
  failed: { label: 'Упала', title: 'Упала' },
  ready: { label: 'К одобрению', title: 'Готова к одобрению (мерж)' },
  plan: { label: 'План готов', title: 'План готов — ждёт «да»' },
  review: { label: 'На ревью', title: 'На ревью' },
  working: { label: 'В работе', title: 'В работе' },
  queued: { label: 'В очереди', title: 'В очереди (after)' },
  done: { label: 'Сделано', title: 'Сделано' },
};
const ORDER = ['waiting', 'failed', 'ready', 'plan', 'review', 'working', 'queued', 'done'];
const ATTN = ['waiting', 'failed', 'ready', 'plan'];
const STAGES = [
  { k: 'queue', label: 'Очередь', st: ['queued'] },
  { k: 'work', label: 'План и работа', st: ['plan', 'working', 'waiting', 'failed'] },
  { k: 'review', label: 'Ревью', st: ['review'] },
  { k: 'appr', label: 'К одобрению', st: ['ready'] },
  { k: 'done', label: 'Сделано', st: ['done'] },
];
const ACTION_LABEL = {
  approve: 'Одобрить',
  anyway: 'Всё равно влить',
  retry: 'Повторить',
  review: 'Вернуть ревью агенту',
  stop: 'Остановить',
  delegate: 'Запустить сейчас',
  archive: 'В архив',
  restore: 'Вернуть из архива',
  answer: 'Ответить',
  fix: 'Исправить ошибку',
  finish: 'Завершить как есть',
};
const SIMPLE_KEY = 'herdr-planner.simple';
const NOTIFY_KEY = 'herdr-planner.notify';

const state = {
  tasks: [],
  archived: [],
  projects: [],
  links: [],
  todo: [],
  limits: null,
  summary: {},
  log: [],
  coordinator: {},
  requests: [],
  theme: null,
  board: { ok: true, error: '', stale_at: 0 },
  auth: { required: false, user: '' },
  server: {},
  herdr: {},
  filter: null,
  pop: null,
  sel: null,
  linkSel: null,
  armed: null,
  armedAt: 0,
  details: {},
  detailsLoading: {},
  archiveOpen: false,
  local: localClient(),
  prev: {},
  justChanged: [],
  fresh: {},
  firstLoad: true,
  lastLog: 0,
  newLog: new Set(),
  optimisticSent: '',
  chatError: '',
  tellDraft: {},
  expanded: false,
  refreshing: false,
  motion: 'auto',
  simple: false,
  notify: false,
  showQuiet: true,
  panels: { focus: true, log: false, wait: false, nav: false, help: false, todo: innerWidth >= 900 },
  narrow: innerWidth < 900,
  screen: {},
  screenBusy: {},
  answered: new Set(),
  reqSeen: new Set(),
  variantOptions: [],
  linkMode: false,
  dragTodo: null,
  todoEditing: null,
  suppressFly: false,
};

state.simple = QS.has('calm') || mq.matches || localStorage.getItem(SIMPLE_KEY) === 'on';
state.notify = localStorage.getItem(NOTIFY_KEY) === 'on';

/* ───────── API ───────── */

async function api(path, opts = {}) {
  const r = await fetch(path, { credentials: 'same-origin', ...opts });
  if (r.status === 401) {
    location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search);
    throw new Error('unauthorized');
  }
  const data = await r.json().catch(() => ({}));
  if (!r.ok) {
    const e = new Error(data.error || `HTTP ${r.status}`);
    e.status = r.status;
    e.data = data;
    throw e;
  }
  return data;
}

const post = (path, body) => api(path, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify(body || {}),
});

/* ───────── theme ───────── */

function applyTheme(t) {
  if (!t || !t.colors) return;
  const r = document.documentElement.style;
  for (const [k, v] of Object.entries(t.colors)) {
    if (typeof v === 'string' && v.startsWith('#')) r.setProperty('--c-' + k, v);
  }
  document.documentElement.dataset.mode = t.mode === 'light' ? 'light' : 'dark';
  state.theme = t;
  if (world) world.setPalette(t.colors, t.mode);
}

/* ───────── model ───────── */

function mapTask(t) {
  return {
    id: t.id,
    repo: t.repo,
    n: t.n,
    tag: t.tag,
    title: t.title || '',
    s: t.status,
    label: t.label || ST[t.status]?.label || t.status,
    board_label: t.board_label || '',
    state: t.state || '',
    kind: t.kind || '',
    stage: t.stage || '',
    agent: t.app || '',
    model: '',
    line: t.msg || '',
    progress: Number.isFinite(t.progress) ? t.progress : null,
    eta: Number.isFinite(t.eta) ? t.eta : null,
    archived: !!t.archived,
    review: t.review || null,
    actions: t.actions || [],
    round: t.round || 0,
    review_failed: !!t.review_failed,
    merge_blocked: !!t.merge_blocked,
  };
}

function applyState(s) {
  applyTheme(s.theme);
  const prev = new Map(state.tasks.map((t) => [t.id, t.s]));
  const wasFirst = state.firstLoad;
  const now = Date.now();
  state.tasks = (s.tasks || []).map(mapTask);
  state.archived = (s.archived || []).map(mapTask);
  state.projects = s.projects || [];
  state.links = s.links || [];
  state.todo = s.todo || state.todo;
  state.limits = s.limits ?? null;
  if (world) world.setLimits(state.limits);
  state.summary = s.summary || {};
  state.board = s.board || state.board;
  state.auth = s.auth || state.auth;
  state.server = s.server || state.server;
  state.herdr = s.herdr || state.herdr;
  state.coordinator = s.coordinator || {};
  state.motion = s.motion || state.motion;

  for (const t of state.tasks) {
    if (prev.has(t.id) && prev.get(t.id) !== t.s) delete state.details[t.id];
    if (t.s === 'done' && !wasFirst && prev.has(t.id) && prev.get(t.id) !== 'done') {
      state.fresh[t.id] = now;
      setTimeout(() => {
        if (state.fresh[t.id]) {
          delete state.fresh[t.id];
          pushWorld();
          renderTop();
        }
      }, 9000);
    }
  }
  for (const id of Object.keys(state.fresh)) if (!state.tasks.some((t) => t.id === id)) delete state.fresh[id];

  const log = s.journal || [];
  state.newLog = new Set(log.filter((e) => e.n > state.lastLog).map((e) => e.n));
  state.lastLog = Math.max(state.lastLog, ...log.map((e) => e.n), 0);
  state.log = log;

  state.requests = collectRequests({ tasks: state.tasks, coordinator: state.coordinator });
  if (!state.sel || !task(state.sel)) state.sel = state.requests.find((r) => r.id !== 'coordinator')?.id || state.tasks[0]?.id || null;
  state.firstLoad = false;
  if (state.sel) fetchDetails(state.sel);
  autoScreen();
  notifyNew(wasFirst);
  pushWorld();
  renderAll();
}

function task(id) { return state.tasks.find((t) => t.id === id) || state.archived.find((t) => t.id === id); }
const tagOf = (id) => task(id)?.tag || id;
const byTag = (tag) => [...state.tasks, ...state.archived].find((t) => t.tag === tag);
const urgent = (s) => ORDER.indexOf(s);
const timeOf = (ms) => {
  if (!ms) return '';
  const d = new Date(ms);
  return pad(d.getHours()) + ':' + pad(d.getMinutes());
};
const keysHTML = (text) => esc(text).replace(/\b([A-Z][A-Z0-9]{1,6}-\d+)\b/g, (m) => `<button class="k" data-tag="${m}" type="button">${m}</button>`);
const linkLabel = (l) => l.label || (l.kind === 'far' ? l.from : tagOf(l.from));
const remoteLink = (l) => l.kind === 'far' && String(l.from).includes(':');

function pushWorld() {
  if (!world) return;
  world.setData({
    tasks: state.tasks,
    projects: state.projects,
    links: state.links,
    selected: state.sel,
    filter: state.filter,
    fresh: state.fresh,
    attention: state.summary.attention || state.requests.filter((r) => r.type !== 'coordinator-options').length,
    coordinator: state.coordinator,
    showQuiet: state.showQuiet,
  });
  if (!state.suppressFly && state.linkMode !== undefined) world.linkModeOn(state.linkMode);
}

/* ───────── world ───────── */

const world = createWorld({
  canvas: $('scene'),
  labelsEl: $('labels'),
  simple: state.simple,
  reduced: mq.matches,
});

world.onSelect = (id) => { select(id, false); };
world.onUniverse = (name) => { state.sel = null; world.select(null); world.flyTo(name); renderTop(); renderFocus(); renderNavigator(); };
world.onGod = () => { world.flyTo('god'); state.sel = null; world.select(null); renderTop(); renderFocus(); };
world.onEmpty = () => { world.flyTo('overview'); state.linkSel = null; renderTop(); renderFocus(true); };
world.onLink = (l) => { state.linkSel = l; state.sel = l.to; state.archiveOpen = false; world.select(l.to); renderFocus(); };
world.onConnect = (from, to) => connectTasks(from, to);
world.onAutoSimple = () => {
  state.simple = true;
  applySimple();
  toast('Включён упрощённый режим · вернуть', { action: 'simple-off', label: 'Вернуть' });
};
if (!world.webgl) {
  $('scene').hidden = true;
  $('nogl').hidden = false;
  state.panels.nav = true;
  state.panels.focus = false;
  $('win').classList.add('nogl-mode');
}
window.world = world;
window.plannerDebug = () => ({
  local: state.local, narrow: state.narrow, requests: state.requests.length,
  sel: state.sel, filter: state.filter, linkMode: state.linkMode,
  todo: state.todo.length, dragTodo: state.dragTodo,
});

function applySimple() {
  $('win').classList.toggle('calm', state.simple || QS.has('calm'));
  world.setSimple(state.simple);
}

/* ───────── mascot (still the Coordinator's face) ───────── */

const MASCOT = [
  '......tt......', '......XX......', '..XXXXXXXXXX..', '.XXXXXXXXXXXX.', '.XXEEXXXXEEXX.', '.XXEEXXXXEEXX.',
  '.XXXXXXXXXXXX.', '.XXXXXXXXXXXX.', '.XXXX....XXXX.', '..XX..XX..XX..',
];

function mascotHTML() {
  let h = '<g>', ey = '';
  MASCOT.forEach((row, y) => [...row].forEach((c, x) => {
    if (c === '.') return;
    const r = `<rect x="${x * 10}" y="${y * 10 + 10}" width="10" height="10"`;
    if (c === 'X') h += r + ' class="b"/>';
    else if (c === 't') h += r + ' class="t"/>';
    else ey += r + ' class="e"/>';
  }));
  return `<svg class="mascot" viewBox="0 0 140 120" aria-hidden="true">${h}</g><g class="eyes">${ey}</g></svg>`;
}

/* ───────── HUD ───────── */

function renderTop() {
  const s = state.summary;
  const quiet = state.projects.filter((p) => p.quiet);
  const total = s.tasks || 0;
  const stageCount = (st) => state.tasks.filter((t) => st.includes(t.s)).length;
  const sub = total
    ? `${s.active_projects || 0} ${plural(s.active_projects || 0, 'вселенная', 'вселенные', 'вселенных')} в работе. Ждут тебя: ${s.attention || 0}.`
    : 'Мир пуст. Скажи Координатору, что сделать, — вселенные появятся здесь.';
  const stat = (label, value, cls = '') => `<div class="stat"><dt class="lbl">${label}</dt><dd class="${cls}">${value}</dd></div>`;
  const quietStat = `<div class="stat"><dt class="lbl">Тихих</dt><dd class="small"><button class="dd" data-pop="quiet" type="button"><span class="pd"></span>${quiet.length}</button></dd></div>`;
  const herdr = state.herdr.running
    ? `<button class="dd" data-pop="herdr" type="button"><span class="pd on"></span>herdr запущен</button>`
    : `<button class="dd" data-pop="herdr" type="button"><span class="pd"></span>herdr не запущен</button>`;
  const stages = STAGES.map((g) => {
    const n = stageCount(g.st);
    const w = total ? Math.round(n / total * 100) : 0;
    const pressed = state.filter && state.filter.length === g.st.length && g.st.every((x) => state.filter.includes(x));
    return `<button class="stage" data-stage="${g.k}" aria-pressed="${!!pressed}" type="button"><span class="t"><span class="lbl">${g.label}</span><b>${n}</b></span><span class="ln"><i style="--w:${w}%"></i></span></button>`;
  }).join('');
  const pop = state.pop === 'quiet'
    ? `<div class="pop"><b>Тихие вселенные</b> — без активных задач, на внешнем кольце.<div class="qlist">${quiet.length ? quiet.map((p) => `<span class="qp">${esc(p.name)} <em>${p.done} сделано</em></span>`).join('') : '<span class="qp">нет</span>'}</div></div>`
    : state.pop === 'herdr'
      ? `<div class="pop"><b>Планировщик работает отдельно от herdr.</b> Данные и действия идут напрямую в <code>herdr-projects</code>. Экран диалога и клавиши доступны только при запущенном herdr${state.herdr.running ? ', и он сейчас запущен' : '; сейчас он не запущен'}. Файлы открываются в yazi${state.local ? '' : ' только на машине с доской'}.</div>`
      : '';
  const cur = world.current || { kind: 'overview', universe: null, id: null };
  const sel = state.sel ? task(state.sel) : null;
  const crumbs = [`<button data-crumb="overview" type="button">Мир</button>`];
  const uniName = cur.universe || sel?.repo || null;
  if (uniName) {
    crumbs.push('<span class="sep">›</span>', `<button data-crumb="universe:${esc(uniName)}" type="button">${esc(uniName)}</button>`);
  }
  if (sel) crumbs.push('<span class="sep">›</span>', `<span>${esc(sel.tag)}</span>`);
  $('hud').innerHTML = `
    <div class="hud-title">
      <div class="lbl">HERDR-PROJECTS · 3D-МИР</div>
      <h1>Herdr Planner</h1>
      <p class="sub">${esc(sub)}</p>
      <div class="crumbs">${crumbs.join('')}</div>
    </div>
    <div class="stats">
      ${stat('Вселенных', s.active_projects || 0)}
      ${stat('Задач', total)}
      ${stat('Ждут тебя', s.attention || 0, 'hot')}
      ${quietStat}
      <div class="stat"><dt class="lbl">Связь</dt><dd class="small">${herdr}</dd></div>
    </div>
    <div class="pipe">${stages}</div>
    <div class="head-tools">
      <button class="tool" data-act="refresh" type="button">${gl('refresh')}Обновить</button>
      <button class="tool ${state.archiveOpen ? 'hot' : ''}" data-act="archive-view" type="button">${gl('archive')}Архив${s.archived ? ` · ${s.archived}` : ''}</button>
      ${s.done ? `<button class="tool" data-act="archive-done-all" type="button" title="Убрать все сделанные задачи в архив">${gl('archive')}Убрать сделанные</button>` : ''}
      <button class="tool" data-act="quiet" aria-pressed="${state.showQuiet}" type="button">${gl('universe')}Тихие</button>
      <button class="tool" data-act="link-mode" aria-pressed="${state.linkMode}" type="button">${gl('link')}Связь · L</button>
      <button class="tool" data-act="todo" aria-pressed="${state.panels.todo}" type="button">${gl('list')}Дела Божьи ${state.todo.length}</button>
      <button class="tool" data-act="simple" aria-pressed="${state.simple}" type="button" title="Меньше эффектов, рендер по требованию">${gl('planner')}Упрощённый</button>
      <button class="tool" data-act="notify" aria-pressed="${state.notify}" type="button" title="Уведомление и звук на новый запрос">${gl('bell')}Уведомления</button>
      <button class="tool" data-act="help" type="button">${gl('help')}Клавиши</button>
      <span class="tool" id="hud-limits" hidden title="Лимиты подписок: место PLN-5"></span>
      ${state.auth.required ? `<span class="who">${esc(state.auth.user)}</span><button class="tool" data-act="logout" type="button">${gl('logout')}Выйти</button>` : ''}
    </div>
    ${pop}`;
  $('banner').innerHTML = state.board.ok ? '' : `<div class="banner"><span>${gl('failed')}</span><span class="sp">Доска не отвечает — показаны данные от ${timeOf(state.board.stale_at) || '—'}; действия выключены.</span><button class="tool" data-act="refresh" type="button">${gl('refresh')}Повторить</button></div>`;
  document.title = (s.attention ? `(${s.attention}) ` : '') + 'Herdr Planner';
}

function plural(n, one, few, many) {
  const m10 = n % 10, m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/* ───────── requests ───────── */

function requestFor(id) {
  return state.requests.find((r) => r.id === id && r.type !== 'coordinator-options' && r.type !== 'coordinator-dialog') || null;
}

function requestBlock(req, t) {
  if (!req) return '';
  const dis = state.board.ok ? '' : ' aria-disabled="true"';
  const head = `<div class="rh">${gl(t.s)}${esc(req.title)}</div>`;
  let body = `<p class="full">${keysHTML(req.msg || req.text)}</p>`;
  if (req.type === 'permission') {
    body += `<div class="row"><button class="btn go" data-act="perm-allow" data-id="${t.id}"${dis} type="button">Разрешить</button><button class="btn danger" data-act="perm-deny" data-id="${t.id}"${dis} type="button">Запретить</button><span class="hint">Текст подставится в поле ниже — его можно поправить.</span></div>`;
    if (t.stage === 'approval') {
      const armedA = state.armed === `${t.id}:anyway`;
      const armedR = state.armed === `${t.id}:review`;
      body += `<div class="row"><button class="btn danger ${armedA ? 'arm' : ''}" data-act="anyway" data-id="${t.id}"${dis} type="button">${armedA ? 'Подтвердить «всё равно»' : 'Всё равно влить'}</button><button class="btn" data-act="review" data-id="${t.id}"${dis} type="button">${armedR ? 'Подтвердить возврат' : 'Вернуть ревью'}</button></div>`;
    }
  } else if (req.type === 'auth') {
    body += `<div class="row"><button class="btn" data-act="answer" data-id="${t.id}" type="button">Написать агенту</button><button class="btn" data-act="stop" data-id="${t.id}"${dis} type="button">Остановить</button></div><div class="hint">Войти можно на машине с доской: планировщик не хранит твоих логинов.</div>`;
  } else if (req.type === 'limit') {
    body += `<div class="row"><button class="btn go" data-act="retry" data-id="${t.id}"${dis} type="button">Продолжить</button><button class="btn" data-act="finish" data-id="${t.id}"${dis} type="button">Завершить как есть</button></div><div class="hint">Агент остановился на лимите подписки: продолжение начнёт новую попытку.</div>`;
  } else if (req.type === 'failed') {
    body += `<div class="row"><button class="btn go" data-act="retry" data-id="${t.id}"${dis} type="button">Повторить</button><button class="btn" data-act="fix" data-id="${t.id}"${dis} type="button">Исправить ошибку</button><button class="btn" data-act="finish" data-id="${t.id}"${dis} type="button">Завершить как есть</button><button class="btn danger" data-act="stop" data-id="${t.id}"${dis} type="button">Остановить</button></div>`;
  } else if (req.type === 'plan') {
    const armed = state.armed === `${t.id}:approve`;
    body += `<div class="row"><button class="btn go ${armed ? 'arm' : ''}" data-act="approve" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить одобрить план' : 'Одобрить план'}</button><button class="btn" data-act="answer" data-id="${t.id}" type="button">Вернуть с замечаниями</button></div>`;
  } else if (req.type === 'approval') {
    const armed = state.armed === `${t.id}:approve`;
    const armedAnyway = state.armed === `${t.id}:anyway`;
    body += `<div class="row"><button class="btn go ${armed ? 'arm' : ''}" data-act="approve" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить одобрить' : 'Одобрить'}</button><button class="btn danger ${armedAnyway ? 'arm' : ''}" data-act="anyway" data-id="${t.id}"${dis} type="button">${armedAnyway ? 'Подтвердить «всё равно»' : 'Всё равно влить'}</button></div>`;
  } else if (req.type === 'pick') {
    const d = state.details[t.id];
    const attempts = Array.isArray(d?.attempts) ? d.attempts : [];
    body += `<div class="picks">${attempts.length ? attempts.map((a) => `<div class="pick"><span class="l">${esc(a.letter)}</span><div><div>${esc(a.agent || '')}</div><div class="st ${a.standing === 'passed' ? 'pass' : a.standing === 'failed' ? 'fail' : ''}">${esc(a.standing || '')} · ${esc(a.stat || '')}</div></div><button class="btn" data-act="pick" data-id="${t.id}" data-letter="${esc(a.letter)}"${dis} type="button">Выбрать ${esc(a.letter)}</button><div class="links">${a.diff ? `<a href="${esc(a.diff)}" target="_blank" rel="noopener">diff</a>` : ''}${a.report ? `<a href="${esc(a.report)}" target="_blank" rel="noopener">отчёт</a>` : ''}</div></div>`).join('') : '<div class="hint">Попытки загружаются вместе с деталями…</div>'}</div>`;
  } else if (req.type === 'reviewneeds') {
    body += screenBlock(t.id, 'Показать экран ревьюера', 'reviewer');
  }
  return `<div class="req s-${t.s}">${head}${body}</div>`;
}

function screenBlock(target, label, role = '') {
  const s = state.screen[target];
  const r = role ? ` data-role="${esc(role)}"` : '';
  if (!s) {
    const busy = state.screenBusy[target];
    return `<div class="row"><button class="btn" data-act="screen" data-target="${esc(target)}"${r} type="button"${busy ? ' aria-disabled="true"' : ''}>${gl('screen')}${esc(label)}</button><span class="hint">${busy ? 'читаю экран…' : 'Содержимое диалога доска не отдаёт: его читает herdr.'}</span></div>`;
  }
  if (s.error) {
    return `<div class="screen"><div class="hint">${esc(s.error)}</div><div class="row"><button class="btn" data-act="screen" data-target="${esc(target)}"${r} type="button">${gl('refresh')}Ещё раз</button></div></div>`;
  }
  return `<div class="screen"><pre>${esc(s.text || '')}</pre><div class="keys">${['up', 'down', 'left', 'right', 'enter', 'esc', '1', '2', '3', 'y', 'n'].map((k) => `<button class="btn" data-act="keys" data-target="${esc(target)}" data-keys="${k}"${r} type="button">${k === 'enter' ? '↵ Enter' : k === 'esc' ? 'Esc' : k}</button>`).join('')}</div><div class="row"><button class="btn" data-act="screen" data-target="${esc(target)}"${r} type="button">${gl('refresh')}Обновить экран</button></div></div>`;
}

/* ───────── focus ───────── */

function failureText(f) {
  if (!f) return '';
  if (typeof f === 'string') return f;
  return f.text || f.message || f.error || f.detail || f.summary || JSON.stringify(f);
}

function renderFocus(keepScroll = false) {
  const panel = $('focus');
  panel.hidden = !state.panels.focus;
  const scroll = keepScroll ? panel.scrollTop : 0;
  const active = document.activeElement;
  const keep = active && active.id === 'tell' && panel.contains(active)
    ? { id: active.dataset.id, value: active.value, start: active.selectionStart, end: active.selectionEnd }
    : null;
  if (state.archiveOpen) {
    panel.innerHTML = `<div class="fhd"><span class="lbl">03 / АРХИВ</span><span class="hint">${state.archived.length}</span></div><h3>Архив доски</h3><p>Сделанные задачи, убранные с доски. Верни любую — она появится планетой.</p><div class="f-actions"><button class="tool" data-act="archive-close" type="button">${gl('close')}Закрыть архив</button></div><div class="arc-list">${state.archived.length ? state.archived.map((t) => `<div class="arc-row"><span class="k">${esc(t.tag)}</span><div><div class="t">${esc(t.title)}</div><div class="m">${esc(t.msg || t.board_label)}</div></div><button class="btn" data-act="restore" data-id="${t.id}" type="button">Вернуть</button></div>`).join('') : '<div class="arc-empty">Архив пуст.</div>'}</div>`;
    panel.scrollTop = scroll;
    return;
  }
  const t = state.sel ? task(state.sel) : null;
  if (!t) {
    panel.innerHTML = `<div class="fhd"><span class="lbl">03 / ФОКУС</span></div><h3>Выбери планету</h3><p>Щёлкни по планете в мире или выбери её в навигаторе (<b>N</b>). Здесь появятся запрос, план, попытки, ветка и действия.</p>`;
    return;
  }
  const d = state.details[t.id];
  const dis = state.board.ok ? '' : ' aria-disabled="true"';
  const review = d?.review && typeof d.review === 'object' ? d.review : (t.review || null);
  const checks = Array.isArray(review?.checks) ? review.checks : [];
  const verdict = review?.tally ? `Ревью: ${review.tally}` : (review?.text ? 'Ревью' : '');
  const worktree = d?.worktree || '';
  const reports = d?.reports_dir || '';
  const req = requestFor(t.id);
  const actions = (t.actions || []).filter((a) => !['tell', 'answer'].includes(a));
  const actionButtons = actions.map((a) => {
    const armed = state.armed === `${t.id}:${a}`;
    if (a === 'approve') {
      const label = t.s === 'plan' ? 'Одобрить план' : 'Одобрить';
      return `<button class="btn go ${armed ? 'arm' : ''}" data-act="approve" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить ' + label.toLowerCase() : label}</button>`;
    }
    if (a === 'anyway') return `<button class="btn danger ${armed ? 'arm' : ''}" data-act="anyway" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить «всё равно»' : 'Всё равно влить'}</button>`;
    return `<button class="btn" data-act="${a}" data-id="${t.id}"${dis} type="button">${ACTION_LABEL[a] || a}</button>`;
  }).join('');
  const files = t.s !== 'queued' ? `<button class="btn" data-act="files" data-id="${t.id}" type="button">${gl('folder')}Worktree</button><button class="btn" data-act="reports" data-id="${t.id}" type="button">${gl('folder')}Отчёты</button>` : '';
  const remote = state.local ? '' : `<div class="remote">Открыть файлы можно только на машине с доской. Путь: <b>${esc(worktree || reports || 'ещё нет')}</b></div>`;
  const tell = `<div class="tell"><input id="tell" data-id="${t.id}" value="${esc(state.tellDraft[t.id] || '')}" placeholder="Написать агенту ${esc(t.tag)}…" aria-label="Написать агенту" autocomplete="off"${dis}><button class="btn" data-act="tell-agent" data-id="${t.id}"${dis} type="button">Отправить</button></div>`;
  const reviewBlock = review?.text
    ? `<div class="h">Вердикт</div><p class="review">${keysHTML(review.text)}</p>`
    : (t.review_failed ? `<div class="h">Ревью</div><p>Ревью не прошло — можно вернуть замечания агенту или влить «всё равно».</p>` : '');
  const checksBlock = checks.length ? `<div class="h">Проверки</div>${checks.map((c) => `<div class="check">${keysHTML(c)}</div>`).join('')}` : '';
  const failureBlock = d?.failure ? `<div class="h">Ошибка</div><p class="review">${keysHTML(failureText(d.failure))}</p>` : '';
  const next = d?.failure?.next || d?.review?.next || '';
  const planText = d?.plan?.text || '';
  const deps = state.links.filter((l) => l.to === t.id);
  const depBlock = `<div class="h">Связи · эта начнётся после</div>${deps.length ? deps.map((l) => `<div class="row"><span class="hint">${esc(linkLabel(l))}</span><button class="btn icon" data-act="after-remove" data-id="${t.id}" data-from="${esc(l.from)}" title="Снять связь" aria-label="Снять связь" type="button">${gl('close')}</button></div>`).join('') : '<div class="hint">Ни от кого не зависит. Потяни линию от этой планеты к другой (Shift или режим «Связь»), чтобы сказать «начнётся после».</div>'}`;
  const linkCard = state.linkSel ? `<div class="req s-ready"><div class="rh">${gl('link')}Связь</div><p class="full">${esc(tagOf(state.linkSel.to))} начнётся после ${esc(linkLabel(state.linkSel))}</p><div class="row"><button class="btn" data-act="link-focus" data-id="${state.linkSel.to}" type="button">Открыть ${esc(tagOf(state.linkSel.to))}</button>${remoteLink(state.linkSel) ? '<span class="hint">Это чужая машина: снять там.</span>' : `<button class="btn danger" data-act="after-remove" data-id="${esc(state.linkSel.to)}" data-from="${esc(state.linkSel.from)}" type="button">Снять</button>`}</div></div>` : '';
  panel.innerHTML = `<div class="fhd"><span class="lbl">03 / ФОКУС · ${esc(t.tag)}</span>${badge(t.s)}<span class="agent">${esc(t.agent)}</span><button class="btn icon" data-act="focus-close" title="Свернуть (F)" aria-label="Свернуть фокус" type="button">${gl('close')}</button></div>
    <h3>${esc(t.title)}</h3>
    <p>${keysHTML(t.line || t.board_label)}</p>
    ${linkCard}
    ${requestBlock(req, t)}
    <div class="kv">${d?.branch ? `<span>ветка</span><b>${esc(d.branch)}</b>` : ''}<span>worktree</span><b>${esc(worktree || '—')}</b>${d?.agent_model ? `<span>модель</span><b>${esc(d.agent_model)}</b>` : ''}<span>этап</span><b>${esc(t.stage || t.state || '—')}</b></div>
    <div class="f-actions">${req ? '' : actionButtons}${files}</div>
    ${tell}
    ${remote}
    ${depBlock}
    ${verdict ? `<div class="h">${esc(verdict)}</div>` : ''}
    ${reviewBlock}${checksBlock}${failureBlock}
    ${planText ? `<div class="h">План</div><p class="review">${keysHTML(planText)}</p>` : ''}
    ${next ? `<div class="h">Дальше</div><p>${keysHTML(next)}</p>` : ''}
    ${d ? '' : '<div class="hint">Загружаю детали задачи…</div>'}
    ${d?.report ? `<div class="row"><button class="btn" data-act="report" data-url="${esc(String(d.report.url || d.report))}" type="button">${gl('ext')}Отчёт</button></div>` : ''}`;
  panel.scrollTop = scroll;
  if (keep) {
    const el = $('tell');
    if (el && el.dataset.id === keep.id) {
      if (el.value !== keep.value) { el.value = keep.value; state.tellDraft[keep.id] = keep.value; }
      el.focus({ preventScroll: true });
      try { el.setSelectionRange(keep.start, keep.end); } catch { /* not a text field */ }
    }
  }
}

function badge(s) { return `<span class="badge">${gl(s, s === 'working' ? 'spin' : '')}${ST[s].label}</span>`; }

/* ───────── console ───────── */

function renderConsole() {
  const c = state.coordinator || {};
  const sent = state.optimisticSent || c.last_sent || '';
  const reply = c.reply || '';
  const err = state.chatError || c.error || '';
  const options = c.dialog ? [] : parseOptions(reply);
  state.variantOptions = options;
  let log = '';
  // PLN-6: a message is `{ text, attachments: [] }`; attachments render as thumbnails here.
  const message = { text: sent, attachments: [] };
  if (sent) {
    log += `<div class="you"><span>›</span><span>${esc(message.text)}</span></div>`;
    if (c.sent_at) log += `<div class="meta"><span>${timeOf(c.sent_at)}</span><span>· отправлено Координатору</span></div>`;
  }
  if (c.busy && !err) {
    log += `<div class="rep"><span class="pr">›</span><span class="tx">читает доску<span class="dots"><i></i><i></i><i></i></span></span></div>`;
  } else if (reply) {
    log += `<div class="rep"><span class="pr">›</span><span class="tx">${keysHTML(reply)}</span></div>`;
    log += `<div class="meta"><span>последнее из окна Координатора</span><span>· ${esc(c.status || '')}</span></div>`;
    if (reply.length > 220) log += `<button class="more" data-chat="toggle" type="button">${state.expanded ? 'свернуть ▴' : 'развернуть ▾'}</button>`;
  } else if (err) {
    log += `<div class="rep err"><span class="pr">›</span><span class="tx">${esc(err)}</span></div><button class="more" data-chat="retry" type="button">отправить ещё раз</button>`;
  } else {
    log += `<div class="rep empty"><span class="pr">›</span><span class="tx">Скажи Координатору, что сделать, — ответ появится здесь.</span></div>`;
  }
  $('clog').innerHTML = log;
  $('clog').classList.toggle('expanded', state.expanded);

  const variants = $('cvariants');
  if (options.length) {
    variants.hidden = false;
    variants.innerHTML = options.map((o, i) => `<button class="btn" data-variant="${i}" type="button">${esc(o.label)} ${esc(o.value)}</button>`).join('');
  } else {
    variants.hidden = true;
    variants.innerHTML = '';
  }

  const dlg = $('cdialog');
  if (c.dialog) {
    dlg.hidden = false;
    const s = state.screen.coordinator;
    dlg.innerHTML = `<div class="h">Координатор показывает диалог</div>${s?.error ? `<div class="hint">${esc(s.error)}</div>` : `<pre>${esc(s?.text || (state.screenBusy.coordinator ? 'читаю экран…' : ''))}</pre>`}<div class="keys">${['up', 'down', 'left', 'right', 'enter', 'esc', '1', '2', '3', 'y', 'n'].map((k) => `<button class="btn" data-act="keys" data-target="coordinator" data-keys="${k}" type="button">${k === 'enter' ? '↵ Enter' : k === 'esc' ? 'Esc' : k}</button>`).join('')}</div>${state.herdr.running ? '' : '<div class="hint">Ответить можно только в окне herdr: он не запущен.</div>'}`;
  } else {
    dlg.hidden = true;
    dlg.innerHTML = '';
  }

  const busy = !!c.busy;
  $('cstate').textContent = err ? '● ошибка' : c.dialog ? '● диалог' : busy ? '● думает' : `● ${c.status || 'готов'}`;
  $('send').dataset.ready = String($('ask-in').value.trim().length > 0);
}

function setupChat() {
  const ta = $('ask-in'), send = $('send');
  const sync = () => {
    send.dataset.ready = String(ta.value.trim().length > 0);
    ta.style.height = '18px';
    ta.style.height = Math.min(ta.scrollHeight, 76) + 'px';
  };
  ta.addEventListener('input', sync);
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); $('ask').requestSubmit(); }
    else if (e.key === 'ArrowUp' && ta.value === '' && state.optimisticSent) { ta.value = state.optimisticSent; sync(); e.preventDefault(); }
    else if (e.key === 'Escape') { ta.value = ''; sync(); ta.blur(); }
  });
  $('ask').addEventListener('submit', async (e) => {
    e.preventDefault();
    const v = ta.value.trim();
    if (!v) return;
    state.chatError = '';
    state.optimisticSent = v;
    ta.value = '';
    sync();
    renderConsole();
    try {
      await post('/api/chat', { text: v });
      setTimeout(refresh, 700);
      setTimeout(refresh, 2200);
    } catch (err) {
      state.chatError = 'Не получилось передать: ' + err.message;
      ta.value = v;
      sync();
      renderConsole();
    }
  });
}

/* ───────── log, queue, navigator ───────── */

function renderLog() {
  $('drawer-log').hidden = !state.panels.log;
  if (!state.panels.log) return;
  const items = state.log.slice(0, 60);
  $('drawer-log').innerHTML = `<div class="lbl"><span>01 / ЖУРНАЛ ДОСКИ</span><span>${state.board.ok ? 'живой' : 'нет связи'}</span></div>${items.length ? items.map((e) => `<div class="logline ${state.newLog.has(e.n) ? 'new' : ''}"><span class="n">${String(e.n).padStart(3, '0')}</span><span>${timeOf(e.t)}</span><span class="kk s-${e.s}">${esc(tagOf(e.id))}</span><span class="tx">${keysHTML(e.text || '')}</span></div>`).join('') : '<div class="none">Пока тихо.</div>'}`;
}

function renderQueue() {
  $('drawer-wait').hidden = !state.panels.wait;
  if (!state.panels.wait) return;
  const q = state.requests.filter((r) => r.type !== 'coordinator-options');
  $('drawer-wait').innerHTML = `<div class="lbl"><span>02 / ЖДЁТ ТЕБЯ</span><span>${q.length}</span></div>${q.length ? q.map((r) => `<button class="q s-${r.task?.s || 'waiting'} ${state.answered.has(r.id) ? 'resolved' : ''}" data-sel="${r.id === 'coordinator' ? '' : esc(r.id)}" data-request="${esc(r.key)}" type="button"><span>${gl(r.task?.s || 'waiting')}</span><span class="kk">${esc(r.tag)}</span><span class="tx">${esc(r.text || r.title)}</span><span class="pj">${esc(r.repo)}</span></button>`).join('') : '<div class="none">Никто не ждёт. Хорошо.</div>'}`;
}

function renderTodo() {
  const dock = $('todo-dock');
  const list = $('todo-list');
  if (!dock || !list) return;
  dock.hidden = !state.panels.todo;
  $('todo-count').textContent = String(state.todo.length);
  world.showTodo(state.panels.todo);
  // A refresh must not replace the open editor: it lives inside this list.
  if (state.todoEditing) return;
  list.innerHTML = state.todo.length
    ? state.todo.map((it) => `<div class="todo-item" draggable="true" data-todo="${esc(it.id)}"><span class="grip" aria-hidden="true">⋮⋮</span><span class="txt" data-todo-text="${esc(it.id)}" title="Двойной щелчок — изменить">${esc(it.text)}</span><button class="del" data-act="todo-del" data-todo-id="${esc(it.id)}" title="Убрать" aria-label="Убрать дело" type="button">×</button></div>`).join('')
    : '<div class="none">Пусто. Запиши, что ещё не стало задачей.</div>';
  world.showTodo(state.panels.todo);
}

function renderNavigator() {
  $('drawer-nav').hidden = !state.panels.nav;
  if (!state.panels.nav) return;
  const by = new Map();
  for (const t of state.tasks) {
    if (!by.has(t.repo)) by.set(t.repo, []);
    by.get(t.repo).push(t);
  }
  const names = [...new Set([...state.projects.map((p) => p.name), ...by.keys()])].sort();
  $('drawer-nav').innerHTML = `<div class="lbl"><span>04 / НАВИГАТОР МИРА</span><span>${names.length}</span></div>${names.map((name) => {
    const list = (by.get(name) || []).slice().sort((a, b) => urgent(a.s) - urgent(b.s) || a.n - b.n);
    const p = state.projects.find((x) => x.name === name) || {};
    return `<div class="tree-uni"><button data-nav-universe="${esc(name)}" type="button"><span>${gl('universe')}</span><span>${esc(p.key || '')}</span><span>${esc(name)}</span></button><div class="tree-tasks">${list.map((t) => `<button class="tree-task s-${t.s}" data-sel="${t.id}" aria-selected="${state.sel === t.id}" type="button"><span>${gl(t.s)}</span><span class="kk">${esc(t.tag)}</span><span class="tx">${esc(t.title)}</span><span class="pj">${esc(ST[t.s].label)}</span></button>`).join('')}</div></div>`;
  }).join('')}`;
}

function renderHelp() {
  $('help').hidden = !state.panels.help;
  if (!state.panels.help) return;
  const rows = [
    ['ЛКМ / ПКМ / колесо', 'вращать, панорама, приближать мир'],
    ['← → ↑ ↓', 'орбита камеры'],
    ['+ / −', 'приблизить / отдалить'],
    ['0 / Home', 'общий вид'],
    ['Enter', 'подлететь к выбранной планете'],
    ['Tab / Shift+Tab', 'следующая / предыдущая планета по срочности'],
    ['1–9', 'N-й запрос из «Ждёт тебя»'],
    ['[ / ]', 'предыдущая / следующая вселенная'],
    ['N', 'навигатор мира (список вселенных и планет)'],
    ['F', 'панель «Фокус»'],
    ['J / W', 'журнал / «Ждёт тебя»'],
    ['A', 'архив'],
    ['L', 'режим связи: тянуть линию мышью'],
    ['T', 'дела божьи: список рядом с чатом, перетащи дело во вселенную'],
    ['Shift + перетащить', 'связь без режима L: от ждущей планеты к той, что раньше'],
    ['/', 'в чат к Координатору'],
    ['?', 'эта подсказка'],
    ['Esc', 'закрыть панель, выйти из режима, общий вид'],
    ['', 'Связь = herdr-projects after: «эта начнётся после той». Клик по линии открывает её в «Фокусе».'],
  ];
  $('help').innerHTML = `<h2>Управление миром</h2><table>${rows.map(([k, v]) => `<tr><td>${esc(k)}</td><td>${esc(v)}</td></tr>`).join('')}</table><button class="tool" data-act="help" type="button">${gl('close')}Закрыть</button>`;
}

/* ───────── actions ───────── */

function toast(msg, opts = {}) {
  const n = document.createElement('div');
  n.className = 'toast';
  n.innerHTML = gl(opts.icon || 'done') + `<span>${esc(msg)}</span>`;
  if (opts.action) {
    const b = document.createElement('button');
    b.className = 'undo';
    b.type = 'button';
    b.textContent = opts.label || 'Отменить';
    b.addEventListener('click', () => { n.remove(); act(opts.action, null, opts.data); });
    n.appendChild(b);
  }
  $('toasts').appendChild(n);
  setTimeout(() => n.remove(), opts.timeout || (opts.action ? 7000 : 3600));
}

function refreshSoon() {
  setTimeout(refresh, 500);
  setTimeout(refresh, 1800);
}

function sendAction(action, extra = {}) {
  return post('/api/action', { action, ...extra })
    .then((r) => { refreshSoon(); return r; })
    .catch((e) => {
      if (e.status === 403 && e.data && e.data.path) toast(e.data.error + ': ' + e.data.path, { icon: 'failed' });
      else toast(e.message, { icon: 'failed' });
      throw e;
    });
}

function selectedTask() { return state.sel ? task(state.sel) : null; }

async function act(cmd, id, data = {}) {
  if (cmd === 'undo-after') { await saveAfter(data.id, data.refs || [], null); return; }
  const t = id ? task(id) : selectedTask();
  if (cmd === 'refresh') { await api('/api/refresh', { method: 'POST' }).catch(() => {}); await refresh(); toast('Доска прочитана'); return; }
  if (cmd === 'logout') { await post('/api/logout').catch(() => {}); location.href = '/login'; return; }
  if (cmd === 'help') { state.panels.help = !state.panels.help; if (state.panels.help) closePanels('help'); renderHelp(); return; }
  if (cmd === 'simple') {
    if (QS.has('calm')) { toast('?calm держит упрощённый режим до перезагрузки'); return; }
    state.simple = !state.simple;
    localStorage.setItem(SIMPLE_KEY, state.simple ? 'on' : 'off');
    applySimple();
    renderTop();
    return;
  }
  if (cmd === 'simple-off') { state.simple = false; localStorage.setItem(SIMPLE_KEY, 'off'); applySimple(); renderTop(); return; }
  if (cmd === 'notify') {
    state.notify = !state.notify;
    localStorage.setItem(NOTIFY_KEY, state.notify ? 'on' : 'off');
    if (state.notify && typeof Notification !== 'undefined' && Notification.permission === 'default') Notification.requestPermission().catch(() => {});
    renderTop();
    return;
  }
  if (cmd === 'quiet') { state.showQuiet = !state.showQuiet; pushWorld(); renderTop(); return; }
  if (cmd === 'todo') { state.panels.todo = !state.panels.todo; renderTodo(); renderTop(); return; }
  if (cmd === 'todo-del') {
    try { const r = await post('/api/action', { action: 'todo-remove', id: data.todoId }); state.todo = r.todo || []; renderTodo(); renderTop(); } catch (e) { toast(e.message, { icon: 'failed' }); }
    return;
  }
  if (cmd === 'link-mode') { setLinkMode(!state.linkMode); return; }
  if (cmd === 'focus-close') { state.panels.focus = false; renderFocus(); renderTop(); return; }
  if (cmd === 'archive-view') { state.archiveOpen = !state.archiveOpen; state.panels.focus = true; if (state.narrow) closePanels('focus'); renderFocus(); renderTop(); return; }
  if (cmd === 'archive-close') { state.archiveOpen = false; renderFocus(); renderTop(); return; }
  if (cmd === 'archive-done-all') {
    try { const r = await sendAction('archive-done', { scope: 'all' }); toast(`В архив убрано: ${r.archived || 0}`); } catch (e) {}
    return;
  }
  if (cmd === 'answer') { if (t) { select(t.id, true); } setTimeout(() => { const i = $('tell'); if (i) i.focus(); }, 30); return; }
  if (cmd === 'pick') { if (!t) return; try { await sendAction('pick', { id: t.id, letter: data.letter }); state.answered.add(t.id); toast(`${t.tag}: выбрана попытка ${data.letter}`); renderQueue(); } catch (e) {} return; }
  if (cmd === 'screen') { fetchScreen(data.target, data.role); return; }
  if (cmd === 'keys') {
    try {
      await sendAction('keys', { target: data.target, keys: [data.keys], role: data.role });
      setTimeout(() => fetchScreen(data.target, data.role), 500);
      toast(`Клавиша ${data.keys} отправлена`);
    } catch (e) {}
    return;
  }
  if (cmd === 'perm-allow' || cmd === 'perm-deny') {
    if (!t) return;
    const text = cmd === 'perm-allow' ? PERMISSION_ALLOW : PERMISSION_DENY;
    state.tellDraft[t.id] = text;
    renderFocus(true);
    const i = $('tell');
    if (i) { i.value = text; i.focus(); i.select(); }
    toast(cmd === 'perm-allow' ? 'Проверь текст и отправь разрешение' : 'Проверь текст и отправь запрет', { icon: 'waiting' });
    return;
  }
  if (cmd === 'after-remove') {
    const waiting = id;
    const from = data.from;
    await linkOp(async () => {
      await refreshNow();
      const all = state.links.filter((l) => l.to === waiting).map((l) => l.from);
      const refs = all.filter((x) => x !== from);
      await saveAfter(waiting, refs, all);
      await refreshNow();
    });
    state.linkSel = null;
    return;
  }
  if (cmd === 'after-clear') {
    if (!t) return;
    await saveAfter(t.id, [], state.links.filter((l) => l.to === t.id).map((l) => l.from));
    return;
  }
  if (cmd === 'link-focus') { if (t) select(t.id, true); return; }
  if (!t) return;
  if (!state.board.ok && !['files', 'reports', 'screen', 'keys'].includes(cmd)) {
    toast('Нет связи с доской: действие недоступно', { icon: 'failed' });
    return;
  }
  if (cmd === 'approve' || cmd === 'anyway') {
    const key = `${t.id}:${cmd}`;
    if (state.armed !== key) {
      state.armed = key;
      state.armedAt = Date.now();
      renderFocus(true); renderTop();
      setTimeout(() => {
        if (state.armed === key) { state.armed = null; renderFocus(true); renderTop(); }
      }, 3500);
      return;
    }
    if (Date.now() - state.armedAt < 600) {
      toast('Нажмите ещё раз, чтобы подтвердить');
      return;
    }
    state.armed = null;
    state.armedAt = 0;
    renderFocus(true); renderTop();
    try { await sendAction(cmd, { id: t.id, confirm: true }); toast(cmd === 'approve' ? `${t.tag}: одобрено` : `${t.tag}: влито «всё равно»`); } catch (e) {}
    return;
  }
  if (cmd === 'archive') { try { await sendAction('archive', { id: t.id }); toast(`${t.tag} убрана в архив`); } catch (e) {} return; }
  if (cmd === 'restore') { try { await sendAction('restore', { id: t.id }); toast(`${t.tag} возвращена на доску`); } catch (e) {} return; }
  if (cmd === 'files' || cmd === 'reports') {
    if (!state.local) {
      const d = state.details[t.id] || {};
      const p = cmd === 'reports' ? d.reports_dir : d.worktree;
      toast(p ? 'Файлы только на машине с доской: ' + p : 'Файлы откроются на машине с доской');
      return;
    }
    try {
      const r = await sendAction('yazi', { id: t.id, kind: cmd === 'reports' ? 'reports' : 'worktree' });
      toast('yazi: ' + (r.path || 'открыт'));
    } catch (e) {}
    return;
  }
  if (cmd === 'tell-agent') {
    const i = $('tell');
    const text = i ? i.value.trim() : '';
    if (!text) return;
    try {
      await sendAction('tell', { id: t.id, text });
      i.value = '';
      delete state.tellDraft[t.id];
      state.answered.add(t.id);
      renderQueue();
      toast(`Агенту ${t.tag} отправлено`);
      setTimeout(() => { state.answered.delete(t.id); renderQueue(); }, 4000);
    } catch (e) {}
    return;
  }
  if (cmd === 'report') {
    const r = state.details[t.id]?.report;
    const url = data.url || (r && (r.url || r.link || r.href || r));
    if (url && /^https?:\/\//.test(String(url))) window.open(String(url), '_blank', 'noopener');
    return;
  }
  try {
    await sendAction(cmd, { id: t.id });
    toast(`${t.tag}: ${ACTION_LABEL[cmd] || cmd}`);
  } catch (e) {}
}

let linkQueue = Promise.resolve();
function linkOp(fn) {
  const p = linkQueue.then(fn, fn);
  linkQueue = p.catch(() => {});
  return p;
}

/** A link edit must see the board as it is now, not a 4-second-old snapshot. */
async function refreshNow() {
  for (let i = 0; i < 50; i++) {
    if (state.refreshing) { await new Promise((r) => setTimeout(r, 80)); continue; }
    await refresh();
    if (!state.refreshing) return;
  }
}

async function saveAfter(id, refs, undoRefs) {
  const t = task(id);
  try {
    await post('/api/action', { action: 'after', id, after: refs });
    refreshSoon();
    const prev = (undoRefs || []).filter((x) => !state.links.some((l) => l.to === id && l.from === x));
    void prev;
    toast(`${t ? t.tag : id}: ${refs.length ? 'начнётся после ' + refs.map((r) => task(r)?.tag || r).join(', ') : 'больше никого не ждёт'}`, {
      action: undoRefs ? 'undo-after' : null,
      data: { id, refs: undoRefs || [] },
      label: 'Отменить',
      icon: 'link',
      timeout: 12000,
    });
    return true;
  } catch (e) {
    toast(e.message, { icon: 'failed', timeout: 8000 });
    return false;
  }
}

async function connectTasks(from, to) {
  return linkOp(async () => {
    await refreshNow();
    const a = task(from);
    if (!a) { toast('Связь не нарисована: задачи нет на доске', { icon: 'failed' }); return; }
    const bName = task(to)?.tag || to;
    const current = state.links.filter((l) => l.to === from).map((l) => l.from);
    if (current.includes(to)) { toast(`${a.tag} уже ждёт ${bName}`); return; }
    await saveAfter(from, [...current, to], current);
    await refreshNow();
  });
}

function setLinkMode(v) {
  state.linkMode = !!v;
  world.linkModeOn(state.linkMode);
  renderTop();
  if (state.linkMode) toast('Режим связи: тяни от ждущей планеты к той, что раньше', { icon: 'link', timeout: 5000 });
}

/* ───────── the God's todo ───────── */

async function createTodoPlanet(id, repo) {
  if (!id || !repo) return;
  try {
    const r = await post('/api/action', { action: 'todo-create', id, repo });
    state.todo = r.todo || state.todo.filter((x) => x.id !== id);
    renderTodo();
    renderTop();
    world.dropFlash(repo);
    toast(`${repo}: планета создана из дела`, { icon: 'universe' });
    refreshSoon();
  } catch (e) {
    toast(e.message, { icon: 'failed', timeout: 8000 });
  } finally {
    state.dragTodo = null;
    world.setDropTarget(null);
  }
}

function setupTodo() {
  const form = $('todo-new');
  if (form) {
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const input = $('todo-in');
      const text = input.value.trim();
      if (!text) return;
      try {
        const r = await post('/api/action', { action: 'todo-add', text });
        state.todo = r.todo || [];
        input.value = '';
        renderTodo();
        renderTop();
      } catch (err) {
        toast(err.message, { icon: 'failed' });
      }
    });
  }

  document.addEventListener('dragstart', (e) => {
    const item = e.target.closest?.('.todo-item');
    if (!item) return;
    state.dragTodo = item.dataset.todo;
    item.classList.add('dragging');
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'copy';
      e.dataTransfer.setData('text/plain', state.dragTodo);
    }
  });
  document.addEventListener('dragend', () => {
    state.dragTodo = null;
    for (const el of document.querySelectorAll('.todo-item.dragging')) el.classList.remove('dragging');
    world.setDropTarget(null);
  });

  const canvas = $('scene');
  const dragId = (e) => state.dragTodo || e.dataTransfer?.getData('text/plain') || '';
  canvas.addEventListener('dragover', (e) => {
    if (!dragId(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    world.setDropTarget(world.universeAt(e.clientX, e.clientY));
  });
  canvas.addEventListener('dragleave', () => world.setDropTarget(null));
  canvas.addEventListener('drop', (e) => {
    const id = dragId(e);
    if (!id) return;
    e.preventDefault();
    const repo = world.universeAt(e.clientX, e.clientY);
    if (repo) createTodoPlanet(id, repo);
    else { state.dragTodo = null; world.setDropTarget(null); }
  });
  document.addEventListener('dragover', (e) => {
    const lab = e.target.closest?.('[data-universe]');
    if (!lab || !dragId(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    world.setDropTarget(lab.dataset.universe);
  });
  document.addEventListener('drop', (e) => {
    const lab = e.target.closest?.('[data-universe]');
    if (!lab) return;
    const id = dragId(e);
    if (!id) return;
    e.preventDefault();
    createTodoPlanet(id, lab.dataset.universe);
  });

  document.addEventListener('dblclick', (e) => {
    const span = e.target.closest?.('[data-todo-text]');
    if (!span) return;
    const item = state.todo.find((x) => x.id === span.dataset.todoText);
    if (!item) return;
    state.todoEditing = item.id;
    const input = document.createElement('input');
    input.className = 'edit';
    input.value = item.text;
    span.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = async (save) => {
      if (done) return;
      done = true;
      state.todoEditing = null;
      if (save) {
        const v = input.value.trim();
        if (v && v !== item.text) {
          try {
            const r = await post('/api/action', { action: 'todo-update', id: item.id, text: v });
            state.todo = r.todo || state.todo;
          } catch (err) { toast(err.message, { icon: 'failed' }); }
        }
      }
      renderTodo();
      renderTop();
    };
    input.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') { ev.preventDefault(); finish(true); }
      else if (ev.key === 'Escape') { ev.preventDefault(); finish(false); }
    });
    input.addEventListener('blur', () => finish(true));
  });
}

/* ───────── details and screen ───────── */

async function fetchDetails(id) {
  if (!id || state.details[id] || state.detailsLoading[id]) return;
  state.detailsLoading[id] = true;
  try {
    const r = await api('/api/details?id=' + encodeURIComponent(id));
    state.details[id] = r.details;
    if (state.sel === id) renderFocus(true);
  } catch (e) {
    state.details[id] = { error: e.message };
    if (state.sel === id) renderFocus(true);
  } finally {
    delete state.detailsLoading[id];
  }
}

async function fetchScreen(target, role = '') {
  if (state.screenBusy[target]) return;
  state.screenBusy[target] = true;
  renderConsole();
  if (state.sel) renderFocus(true);
  try {
    const key = target === 'coordinator' ? 'coordinator' : target;
    const r = await api('/api/coordinator/screen?target=' + encodeURIComponent(key) + (role ? '&role=' + encodeURIComponent(role) : ''));
    state.screen[target] = { at: Date.now(), ...r };
  } catch (e) {
    state.screen[target] = { error: e.message };
  } finally {
    delete state.screenBusy[target];
    renderConsole();
    if (state.sel) renderFocus(true);
  }
}

function autoScreen() {
  if (!state.coordinator?.dialog) return;
  const s = state.screen.coordinator;
  if (s && Date.now() - (s.at || 0) < 2500) return;
  fetchScreen('coordinator');
}

function select(id, fly = false) {
  if (!task(id)) return;
  state.sel = id;
  state.archiveOpen = false;
  state.linkSel = null;
  state.panels.focus = true;
  world.select(id);
  if (fly) world.flyTo(id);
  renderFocus(true);
  renderQueue();
  renderNavigator();
  renderTop();
  fetchDetails(id);
}

/* ───────── notifications ───────── */

let audio = null;
function beep() {
  try {
    audio = audio || new (window.AudioContext || window.webkitAudioContext)();
    const o = audio.createOscillator();
    const g = audio.createGain();
    o.type = 'sine';
    o.frequency.value = 880;
    g.gain.value = 0.04;
    o.connect(g).connect(audio.destination);
    o.start();
    o.stop(audio.currentTime + 0.12);
  } catch { /* no sound device */ }
}

function notifyNew(first) {
  const keys = new Set(state.requests.map((r) => r.key));
  const fresh = state.requests.filter((r) => !state.reqSeen.has(r.key));
  state.reqSeen = keys;
  if (first || !state.notify || !fresh.length) return;
  beep();
  if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
    const r = fresh[0];
    try { new Notification('Herdr Planner: ' + r.title, { body: firstSentence(r.msg || r.text, 120), tag: r.key }); } catch {}
  }
}

/* ───────── panels ───────── */

function closePanels(except = null) {
  for (const k of ['focus', 'log', 'wait', 'nav']) if (k !== except) state.panels[k] = false;
}

function togglePanel(name) {
  const open = !state.panels[name];
  state.panels[name] = open;
  if (state.narrow && open) closePanels(name);
  renderTop();
  renderFocus();
  renderLog();
  renderQueue();
  renderNavigator();
  renderTodo();
  renderHelp();
}

/* ───────── keyboard ───────── */

function sortedPlanets() {
  return state.tasks
    .filter((t) => t.s !== 'done')
    .slice()
    .sort((a, b) => urgent(a.s) - urgent(b.s) || a.n - b.n);
}

function nextPlanet(delta) {
  const list = sortedPlanets();
  if (!list.length) return;
  const i = list.findIndex((t) => t.id === state.sel);
  const n = list[(i + delta + list.length) % list.length] || list[0];
  select(n.id, true);
}

function nextUniverse(delta) {
  const names = [...new Set(state.projects.map((p) => p.name))].sort();
  if (!names.length) return;
  const cur = world.current?.universe || task(state.sel)?.repo || '';
  const i = names.indexOf(cur);
  const n = names[(i + delta + names.length) % names.length];
  world.flyTo(n);
  renderTop();
}

function renderAll() {
  renderTop();
  renderFocus(true);
  renderConsole();
  renderLog();
  state.newLog.clear();
  renderQueue();
  renderNavigator();
  renderTodo();
  renderHelp();
}

/* ───────── clicks ───────── */

document.addEventListener('click', (e) => {
  if (!e.target.closest('.pop,[data-pop]')) {
    if (state.pop) { state.pop = null; renderTop(); }
  }
  const tag = e.target.closest('[data-tag]');
  if (tag) {
    e.stopPropagation();
    const t = byTag(tag.dataset.tag);
    if (t) select(t.id, true);
    return;
  }
  const variant = e.target.closest('[data-variant]');
  if (variant) {
    const o = state.variantOptions[Number(variant.dataset.variant)];
    if (o) {
      state.optimisticSent = o.value;
      post('/api/chat', { text: o.value }).catch((err) => { state.chatError = err.message; renderConsole(); });
      renderConsole();
      setTimeout(refresh, 900);
      setTimeout(refresh, 2500);
    }
    return;
  }
  const crumb = e.target.closest('[data-crumb]');
  if (crumb) {
    const [kind, ...rest] = crumb.dataset.crumb.split(':');
    if (kind === 'overview') { world.flyTo('overview'); state.linkSel = null; renderTop(); renderFocus(true); }
    else if (kind === 'universe') { world.flyTo(rest.join(':')); renderTop(); }
    return;
  }
  const navU = e.target.closest('[data-nav-universe]');
  if (navU) { world.flyTo(navU.dataset.navUniverse); renderTop(); return; }
  const uni = e.target.closest('[data-universe]');
  if (uni) { e.stopPropagation(); world.flyTo(uni.dataset.universe); renderTop(); return; }
  const req = e.target.closest('[data-request]');
  if (req) {
    const key = req.dataset.request;
    if (key === 'coordinator-dialog' || key === 'coordinator-options') { world.flyTo('god'); renderTop(); }
    else {
      const r = state.requests.find((x) => x.key === key);
      if (r && r.id && task(r.id)) select(r.id, true);
    }
    return;
  }
  const navT = e.target.closest('[data-sel]');
  if (navT) { if (navT.dataset.sel) select(navT.dataset.sel, true); return; }
  const b = e.target.closest('[data-act],[data-pop],[data-group],[data-filter],[data-stage],[data-chat]');
  if (!b) return;
  if (b.matches('[data-act]')) {
    e.stopPropagation();
    if (b.getAttribute('aria-disabled') === 'true' && !['files', 'reports', 'archive-view', 'refresh', 'screen'].includes(b.dataset.act)) {
      if (!state.board.ok) toast('Нет связи с доской: действие недоступно', { icon: 'failed' });
      return;
    }
    const extra = {};
    if (b.dataset.letter) extra.letter = b.dataset.letter;
    if (b.dataset.todoId) extra.todoId = b.dataset.todoId;
    if (b.dataset.target) extra.target = b.dataset.target;
    if (b.dataset.keys) extra.keys = b.dataset.keys;
    if (b.dataset.role) extra.role = b.dataset.role;
    if (b.dataset.from) extra.from = b.dataset.from;
    if (b.dataset.url) extra.url = b.dataset.url;
    if (b.dataset.act === 'notify') { act('notify'); return; }
    if (b.dataset.act === 'quiet') { act('quiet'); return; }
    if (b.dataset.act === 'link-mode') { act('link-mode'); return; }
    if (b.dataset.act === 'simple') { act('simple'); return; }
    if (b.dataset.act === 'help') { act('help'); return; }
    if (b.dataset.act === 'focus-close') { act('focus-close'); return; }
    if (b.dataset.act === 'screen') { act('screen', null, extra); return; }
    if (b.dataset.act === 'keys') { act('keys', null, extra); return; }
    if (b.dataset.act === 'undo-after') { saveAfter(extra.id || b.dataset.id, extra.refs || [], null); return; }
    act(b.dataset.act, b.dataset.id || null, extra);
    return;
  }
  if (b.matches('[data-pop]')) {
    e.stopPropagation();
    state.pop = state.pop === b.dataset.pop ? null : b.dataset.pop;
    renderTop();
    return;
  }
  if (b.matches('[data-group]')) return;
  if (b.matches('[data-filter]')) {
    const f = b.dataset.filter;
    state.filter = (state.filter && state.filter.length === 1 && state.filter[0] === f) ? null : [f];
    world.setFilter(state.filter);
    renderTop(); return;
  }
  if (b.matches('[data-stage]')) {
    const g = STAGES.find((x) => x.k === b.dataset.stage);
    const same = state.filter && state.filter.length === g.st.length && g.st.every((s) => state.filter.includes(s));
    state.filter = same ? null : g.st.slice();
    world.setFilter(state.filter);
    renderTop(); return;
  }
  if (b.matches('[data-chat]')) {
    if (b.dataset.chat === 'toggle') { state.expanded = !state.expanded; renderConsole(); }
    else if (b.dataset.chat === 'retry') { const ta = $('ask-in'); if (state.optimisticSent) ta.value = state.optimisticSent; $('ask').requestSubmit(); }
    return;
  }
});

document.addEventListener('input', (e) => {
  if (e.target.id === 'tell' && e.target.dataset.id) state.tellDraft[e.target.dataset.id] = e.target.value;
});

document.addEventListener('keydown', (e) => {
  const target = e.target;
  const typing = !!(target && target.matches && target.matches('textarea,input,select'));
  if (e.key === 'Enter' && target && target.id === 'tell') {
    const btn = document.querySelector('[data-act="tell-agent"]');
    if (btn) act('tell-agent', target.dataset.id);
    return;
  }
  if (typing) return;
  if (e.key === '/') { e.preventDefault(); $('ask-in').focus(); return; }
  if (/^[1-9]$/.test(e.key) && !e.metaKey && !e.ctrlKey) {
    const r = state.requests.filter((x) => x.type !== 'coordinator-options')[Number(e.key) - 1];
    if (r && r.id !== 'coordinator') { e.preventDefault(); select(r.id, true); }
    return;
  }
  if (e.key === 'Escape') {
    if (state.panels.help) { state.panels.help = false; renderHelp(); return; }
    if (state.linkMode) { setLinkMode(false); return; }
    state.pop = null;
    state.linkSel = null;
    for (const k of Object.keys(state.panels)) state.panels[k] = false;
    state.panels.focus = true;
    world.flyTo('overview');
    renderAll();
    return;
  }
  if (e.key === '0' || e.key === 'Home') { e.preventDefault(); world.flyTo('overview'); renderTop(); return; }
  if (e.key === 'Enter') { if (state.sel) world.flyTo(state.sel); return; }
  if (e.key === 'Tab' && target && target.closest && !target.closest('button,a')) { e.preventDefault(); nextPlanet(e.shiftKey ? -1 : 1); return; }
  if (e.key === '[') { nextUniverse(-1); return; }
  if (e.key === ']') { nextUniverse(1); return; }
  if (e.key === 'ArrowLeft') { e.preventDefault(); world.orbitBy(0.12, 0); return; }
  if (e.key === 'ArrowRight') { e.preventDefault(); world.orbitBy(-0.12, 0); return; }
  if (e.key === 'ArrowUp') { e.preventDefault(); world.orbitBy(0, -0.08); return; }
  if (e.key === 'ArrowDown') { e.preventDefault(); world.orbitBy(0, 0.08); return; }
  if (e.key === '+' || e.key === '=') { world.zoomBy(0.86); return; }
  if (e.key === '-' || e.key === '_') { world.zoomBy(1.16); return; }
  if (e.key === 'l' || e.key === 'L' || e.key === 'д' || e.key === 'Д') { setLinkMode(!state.linkMode); return; }
  if (e.key === 'f' || e.key === 'F' || e.key === 'а' || e.key === 'А') { togglePanel('focus'); return; }
  if (e.key === 'j' || e.key === 'J' || e.key === 'о' || e.key === 'О') { togglePanel('log'); return; }
  if (e.key === 'w' || e.key === 'W' || e.key === 'ц' || e.key === 'Ц') { togglePanel('wait'); return; }
  if (e.key === 'n' || e.key === 'N' || e.key === 'т' || e.key === 'Т') { togglePanel('nav'); return; }
  if (e.key === 'a' || e.key === 'A' || e.key === 'ф' || e.key === 'Ф') { state.archiveOpen = true; state.panels.focus = true; renderFocus(); renderTop(); return; }
  if (e.key === 't' || e.key === 'T' || e.key === 'е' || e.key === 'Е') { act('todo'); return; }
  if (e.key === 'q' || e.key === 'Q' || e.key === 'й' || e.key === 'Й') { toast('Лимиты подписок появятся в PLN-5'); return; }
  if (e.key === '?') { state.panels.help = !state.panels.help; renderHelp(); return; }
});

/* ───────── refresher, width, start ───────── */

async function refresh() {
  if (state.refreshing) return;
  state.refreshing = true;
  try {
    const s = await api('/api/state');
    if (state.optimisticSent && s.coordinator && s.coordinator.last_sent === state.optimisticSent) state.optimisticSent = '';
    if (s.coordinator && !s.coordinator.busy && s.coordinator.reply) state.chatError = '';
    applyState(s);
  } catch (e) {
    if (e.message !== 'unauthorized') {
      state.board = { ...state.board, ok: false, error: e.message };
      renderTop();
    }
  } finally {
    state.refreshing = false;
  }
}

function applyWidth() {
  state.narrow = innerWidth < 900;
  $('win').classList.toggle('narrow', state.narrow);
  if (state.narrow && !state._narrowInit) { state._narrowInit = true; state.panels.focus = false; }
  if (state.narrow) { world.detachChat(); world.detachTodo(); }
  else { world.attachChat(); world.attachTodo(); }
  world.showTodo(state.panels.todo);
  if (state.narrow) {
    const open = Object.entries(state.panels).filter(([k, v]) => v && k !== 'help');
    if (open.length > 1) closePanels(open[0][0]);
  }
  renderAll();
}

window.addEventListener('resize', applyWidth);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });

applySimple();
applyWidth();
setupChat();
setupTodo();
refresh();
setInterval(refresh, 4000);
