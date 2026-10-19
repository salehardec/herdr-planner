'use strict';
/* Herdr Planner — the dashboard in the browser.
   Mockup: design/herdr-planner.html. Data: GET /api/state (the server polls the
   board every few seconds). Commands: POST /api/action and POST /api/chat. */

const $ = (id) => document.getElementById(id);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const NS = 'http://www.w3.org/2000/svg';
const mq = matchMedia('(prefers-reduced-motion: reduce)');
const QS = new URLSearchParams(location.search);
const calm = () => mq.matches || state.motion === 'off' || QS.has('calm');
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
const RAIL = ['очер.', 'план', 'работа', 'ревью', 'одобр.', 'готово'];
const RAILIDX = { queued: 0, plan: 1, working: 2, waiting: 2, failed: 2, review: 3, ready: 4, done: 5 };
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
};

const state = {
  tasks: [],
  archived: [],
  projects: [],
  summary: {},
  log: [],
  coordinator: {},
  theme: null,
  board: { ok: true, error: '', stale_at: 0 },
  auth: { required: false, user: '' },
  server: {},
  herdr: {},
  filter: null,
  collapsed: {},
  doneOpen: {},
  pop: null,
  sel: null,
  armed: null,
  armedAt: 0,
  side: {},
  retired: {},
  fresh: {},
  details: {},
  detailsLoading: {},
  archiveOpen: false,
  local: localClient(),
  prev: {},
  morph: {},
  exit: {},
  justChanged: [],
  firstLoad: true,
  colFirst: false,
  lastLog: 0,
  newLog: new Set(),
  optimisticSent: '',
  chatError: '',
  tellDraft: {},
  expanded: false,
  refreshing: false,
  touched: false,
  motion: 'auto',
};

const task = (id) => state.tasks.find((t) => t.id === id) || state.archived.find((t) => t.id === id);
const tagOf = (id) => task(id)?.tag || id;
const byTag = (tag) => [...state.tasks, ...state.archived].find((t) => t.tag === tag);
const urgent = (s) => ORDER.indexOf(s);
const timeOf = (ms) => {
  if (!ms) return '';
  const d = new Date(ms);
  return pad(d.getHours()) + ':' + pad(d.getMinutes());
};
const keysHTML = (text) => esc(text).replace(/\b([A-Z][A-Z0-9]{1,6}-\d+)\b/g, (m) => `<button class="k" data-tag="${m}" type="button">${m}</button>`);

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
    agent: t.app || '',
    model: '',
    age: '',
    line: t.msg || '',
    progress: Number.isFinite(t.progress) ? t.progress : null,
    eta: Number.isFinite(t.eta) ? t.eta : null,
    after: [],
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
  state.colFirst = wasFirst;
  state.tasks = (s.tasks || []).map(mapTask);
  state.archived = (s.archived || []).map(mapTask);
  state.projects = s.projects || [];
  state.summary = s.summary || {};
  state.board = s.board || state.board;
  state.auth = s.auth || state.auth;
  state.server = s.server || state.server;
  state.herdr = s.herdr || state.herdr;
  state.coordinator = s.coordinator || {};
  state.motion = s.motion || state.motion;

  for (const t of state.tasks) {
    if (prev.has(t.id) && prev.get(t.id) !== t.s) delete state.details[t.id];
    if (t.s === 'done') {
      if (wasFirst) {
        state.retired[t.id] = true;
      } else if (prev.has(t.id) && prev.get(t.id) !== 'done') {
        state.fresh[t.id] = now;
        setTimeout(() => {
          if (state.fresh[t.id]) {
            state.exit[t.id] = { kind: 'retire' };
            state.retired[t.id] = true;
            delete state.fresh[t.id];
            renderCols();
            renderTop();
            layoutFor(380);
          }
        }, 9000);
      } else if (!state.fresh[t.id] && !state.retired[t.id]) {
        state.retired[t.id] = true;
      }
    } else {
      delete state.retired[t.id];
      delete state.fresh[t.id];
    }
  }
  for (const id of Object.keys(state.fresh)) if (!state.tasks.some((t) => t.id === id)) delete state.fresh[id];

  const log = s.journal || [];
  state.newLog = new Set(log.filter((e) => e.n > state.lastLog).map((e) => e.n));
  state.lastLog = Math.max(state.lastLog, ...log.map((e) => e.n), 0);
  state.log = log;

  if (!state.sel || !task(state.sel)) {
    const cards = visibleTasks();
    state.sel = cards[0]?.id || state.tasks[0]?.id || null;
  }
  state.firstLoad = false;
  if (state.sel) fetchDetails(state.sel);
  renderAll();
}

function visibleTasks() {
  return state.tasks.filter((t) => t.s !== 'done' || state.fresh[t.id]);
}

function oldDone(repo) {
  return state.tasks.filter((t) => t.repo === repo && t.s === 'done' && !state.fresh[t.id]);
}

/* ───────── mascot ───────── */

const MASCOT = [
  '......tt......', '......XX......', '..XXXXXXXXXX..', '.XXXXXXXXXXXX.', '.XXEEXXXXEEXX.', '.XXEEXXXXEEXX.',
  '.XXXXXXXXXXXX.', '.XXXXXXXXXXXX.', '.XXXX....XXXX.', '..XX..XX..XX..',
];

function drawMascot() {
  let h = '<g>', ey = '';
  MASCOT.forEach((row, y) => [...row].forEach((c, x) => {
    if (c === '.') return;
    const r = `<rect x="${x * 10}" y="${y * 10 + 10}" width="10" height="10"`;
    if (c === 'X') h += r + ' class="b"/>';
    else if (c === 't') h += r + ' class="t"/>';
    else ey += r + ' class="e"/>';
  }));
  $('mascot').innerHTML = h + '</g><g class="eyes">' + ey + '</g>';
}

/* ───────── groups and cards ───────── */

function groupsModel() {
  const cards = visibleTasks();
  const by = new Map();
  for (const t of cards) {
    if (!by.has(t.repo)) by.set(t.repo, []);
    by.get(t.repo).push(t);
  }
  const gs = [...by.entries()].map(([name, list]) => {
    const p = state.projects.find((x) => x.name === name) || {};
    return {
      name,
      key: p.key || '',
      list: list.sort((a, b) => urgent(a.s) - urgent(b.s)),
    };
  });
  gs.sort((a, b) => Math.min(...a.list.map((t) => urgent(t.s))) - Math.min(...b.list.map((t) => urgent(t.s))));
  const cnt = { L: 0, R: 0 };
  gs.forEach((g) => {
    if (!state.side[g.name]) state.side[g.name] = cnt.L <= cnt.R ? 'L' : 'R';
    cnt[state.side[g.name]] += g.list.length + 1;
  });
  const L = gs.filter((g) => state.side[g.name] === 'L');
  const R = gs.filter((g) => state.side[g.name] === 'R');
  let n = 0;
  for (const g of [...L, ...R]) for (const t of g.list) t.num = ++n;
  return { L, R, all: gs, quiet: state.projects.filter((p) => p.quiet) };
}

function primary(t) {
  if (t.s === 'ready') return { label: 'Одобрить', armed: 'Подтвердить одобрить', cmd: 'approve' };
  if (t.s === 'plan') return { label: 'Одобрить план', armed: 'Подтвердить одобрить план', cmd: 'approve' };
  if (t.s === 'failed') return { label: 'Повторить', cmd: 'retry' };
  if (t.s === 'waiting') return { label: 'Ответить', cmd: 'answer' };
  return null;
}

function railHTML(t, from) {
  const idx = RAILIDX[t.s] ?? 0;
  const pos = (s, prog) => {
    const i = RAILIDX[s] ?? 0;
    return (i + (s === 'working' && prog != null ? Math.min(0.9, (prog / 100) * 0.9) : 0)) / 5 * 100;
  };
  const to = pos(t.s, t.progress);
  const start = from != null ? from : to;
  let h = `<div class="rail" title="Этапы: очередь → план → работа → ревью → одобрение → готово" data-to="${to}" style="--l:${start}%;--w:${start}%"><i class="line"></i><i class="fill"></i>`;
  for (let i = 0; i < 6; i++) h += `<i class="nd ${i < idx ? 'past' : ''}" style="left:${i * 20}%"></i>`;
  h += `<span class="tok" aria-hidden="true">${gl(t.s, t.s === 'working' ? 'spin' : '')}</span>`;
  for (let i = 0; i < 6; i++) h += `<span class="rl ${i === idx ? 'cur' : ''}" style="left:${i * 20}%">${RAIL[i]}</span>`;
  return h + '</div>';
}

function badge(s) {
  return `<span class="badge">${gl(s, s === 'working' ? 'spin' : '')}${ST[s].label}</span>`;
}

function cardHTML(t, i, m) {
  const p = primary(t);
  const armed = !!p && state.armed === `${t.id}:${p.cmd}`;
  const dis = state.board.ok ? '' : ' aria-disabled="true"';
  let line = keysHTML(t.line || t.board_label || '');
  const meta = t.s === 'working' && t.progress != null
    ? `${t.progress}%${t.eta != null ? ' · ~' + t.eta + ' мин' : ''}`
    : (t.age || '');
  const btn = p ? `<button class="btn go ${armed ? 'arm' : ''}" data-act="${p.cmd}" data-id="${t.id}"${dis} type="button">${armed ? esc(p.armed) : p.label}</button>` : '';
  const files = t.s !== 'queued' ? `<button class="btn icon" data-act="files" data-id="${t.id}" aria-label="Открыть worktree ${t.id} в yazi" title="${state.local ? 'Открыть worktree в yazi' : 'Показать путь к worktree'}" type="button">${gl('folder')}</button>` : '';
  const arch = t.s === 'done' ? `<button class="btn icon" data-act="archive" data-id="${t.id}" aria-label="Убрать ${t.id} в архив" title="Убрать в архив" type="button">${gl('archive')}</button>` : '';
  return `<article class="card s-${t.s}" data-id="${t.id}" data-s="${t.s}" tabindex="0" role="button" aria-selected="${state.sel === t.id}" style="--d:${(i % 5) * 0.6}s"
      aria-label="${t.num}. ${esc(t.tag)}: ${esc(t.title)} — ${ST[t.s].label}">
    <i class="port"></i>
    <div class="c-top"><span class="num">${pad(t.num || 0)}</span><span class="key">${esc(t.tag)}</span><span class="agent"><b>${esc(t.agent)}</b></span><span class="sp"></span>${badge(t.s)}</div>
    <h3 class="c-title">${esc(t.title)}</h3>
    <div class="c-line"><span class="tx">${line}</span>${t.s === 'working' ? '<span class="eq" aria-hidden="true"><i></i><i></i><i></i><i></i></span>' : ''}<span class="age">${esc(meta)}</span></div>
    <div class="c-bot">${railHTML(t, m && m.from)}<span class="acts">${btn}${files}${arch}</span></div>
  </article>`;
}

function groupHTML(g) {
  const col = state.collapsed[g.name];
  const roll = {};
  for (const t of g.list) roll[t.s] = (roll[t.s] || 0) + 1;
  const old = oldDone(g.name);
  const rollH = ORDER.filter((s) => roll[s]).map((s) => `<span class="s-${s}">${gl(s)}${roll[s]}</span>`).join('');
  const doneInRepo = state.tasks.filter((t) => t.repo === g.name && t.s === 'done').length;
  const archBtn = `<button class="garch" data-archive-repo="${esc(g.name)}" title="Убрать все сделанные в архив (${doneInRepo})" aria-label="Убрать все сделанные в архив" type="button">${gl('archive')}</button>`;
  let h = `<section class="group" data-repo="${esc(g.name)}"><div class="ghead" data-group="${esc(g.name)}" role="button" tabindex="0" aria-expanded="${!col}"><i class="port"></i>${gl('chev', 'chev')}<span class="name">${esc(g.name)}</span><span class="pk">${esc(g.key)}</span><span class="sp"></span>${col ? `<span class="roll">${rollH}</span>` : ''}${doneInRepo ? archBtn : ''}</div>`;
  if (!col) {
    h += `<div class="cards">${g.list.map((t, i) => cardHTML(t, i, state.morph && state.morph[t.id])).join('')}</div>`;
    if (old.length) {
      const open = state.doneOpen[g.name];
      const rows = old.map((d) => `<div class="donerow"><span>${esc(d.tag)}</span><span>${esc(d.title)}</span><span>${esc(d.msg || '')}</span><button class="btn icon" data-act="archive" data-id="${d.id}" title="Убрать в архив" aria-label="Убрать ${esc(d.tag)} в архив" type="button">${gl('archive')}</button></div>`).join('');
      h += `<div class="donebar"><button data-done="${esc(g.name)}" aria-expanded="${!!open}" type="button">${gl('done')}Сделано ещё ${old.length}${open ? '' : ' · показать'}</button>${open ? `<div class="donerows">${rows}</div>` : ''}</div>`;
    }
  }
  return h + '</section>';
}

/* ───────── render: columns with FLIP ───────── */

function renderCols() {
  const old = new Map($$('.col .card').map((e) => [e.dataset.id, {
    r: e.getBoundingClientRect(), s: e.dataset.s, el: e, repo: e.closest('.group')?.dataset.repo,
  }]));
  const first = state.colFirst && old.size === 0;
  const m = groupsModel();
  const stale = !state.board.ok;
  state.morph = {};
  const changed = [];
  for (const t of visibleTasks()) {
    const p = state.prev[t.id];
    if (p && p !== t.s) {
      state.morph[t.id] = { from: (RAILIDX[p] + (p === 'working' ? 0.5 : 0)) / 5 * 100 };
      changed.push(t.id);
    }
  }
  const empty = !visibleTasks().length;
  const emptyHTML = `<div class="empty-col"><b>Всё тихо.</b><span>На доске нет задач в работе, на ревью или в очереди. Скажи Координатору, что сделать, — задача появится здесь, и к ней протянется нить.</span></div>`;
  $('colL').innerHTML = (stale ? '<div class="stale">' : '<div>') + (empty ? emptyHTML : m.L.map(groupHTML).join('')) + '</div>';
  $('colR').innerHTML = (stale ? '<div class="stale">' : '<div>') + m.R.map(groupHTML).join('') + '</div>';
  const cards = $$('.col .card');
  cards.forEach((el, i) => {
    const id = el.dataset.id;
    const o = old.get(id);
    if (!calm()) {
      if (o) {
        const n = el.getBoundingClientRect();
        const dx = o.r.left - n.left, dy = o.r.top - n.top;
        if (Math.abs(dx) + Math.abs(dy) > 1) {
          el.style.transition = 'none';
          el.style.transform = `translate(${dx}px,${dy}px)`;
          requestAnimationFrame(() => requestAnimationFrame(() => {
            el.style.transition = 'transform .6s var(--m-ease)';
            el.style.transform = '';
          }));
          el.addEventListener('transitionend', () => { el.style.transition = ''; }, { once: true });
        }
      } else {
        el.classList.add('enter');
        el.style.setProperty('--e', (first ? i * 0.06 : 0) + 's');
        el.addEventListener('animationend', () => el.classList.remove('enter'), { once: true });
      }
      if (state.morph[id]) {
        el.classList.add('morph');
        el.addEventListener('animationend', () => el.classList.remove('morph'), { once: true });
      }
    }
    const rail = el.querySelector('.rail');
    if (rail) {
      const to = +rail.dataset.to;
      const set = () => { rail.style.setProperty('--l', to + '%'); rail.style.setProperty('--w', to + '%'); };
      if (calm() || (!state.morph[id] && !first)) set();
      else requestAnimationFrame(() => requestAnimationFrame(set));
    }
  });
  exitGhosts(old, new Set(cards.map((c) => c.dataset.id)));
  state.exit = {};
  state.prev = {};
  for (const t of visibleTasks()) state.prev[t.id] = t.s;
  state.justChanged = changed;
  $$('.col .card').forEach((el) => el.classList.toggle('dim', !!state.filter && !state.filter.includes(el.dataset.s)));
}

function exitGhosts(old, newIds) {
  if (calm()) return;
  const wr = $('win').getBoundingClientRect(), mr = $('main').getBoundingClientRect();
  $('ghosts').style.clipPath = `inset(${mr.top - wr.top}px 0 ${wr.bottom - mr.bottom}px 0)`;
  old.forEach((o, id) => {
    if (newIds.has(id)) return;
    const why = (state.exit[id] && state.exit[id].kind) || 'fade';
    const g = o.el.cloneNode(true);
    g.className = g.className.replace(/\b(enter|morph|dim)\b/g, '').trim() + ' ghost';
    g.removeAttribute('id');
    g.removeAttribute('tabindex');
    g.style.transition = 'none';
    g.style.transform = '';
    g.style.cssText += `;position:absolute;left:${o.r.left - wr.left}px;top:${o.r.top - wr.top}px;width:${o.r.width}px;height:${o.r.height}px;margin:0;transform-origin:0 0`;
    $('ghosts').appendChild(g);
    let tg = null;
    if (why === 'retire') {
      const b = document.querySelector(`.group[data-repo="${o.repo}"] .donebar button`);
      tg = b && b.getBoundingClientRect();
    } else if (why === 'collapse') {
      const h = document.querySelector(`.group[data-repo="${o.repo}"] .ghead`);
      tg = h && h.getBoundingClientRect();
    }
    if (tg && !calm()) {
      const dx = tg.left + Math.min(40, tg.width / 3) - (o.r.left + o.r.width / 2);
      const dy = tg.top + tg.height / 2 - (o.r.top + o.r.height / 2);
      g.animate([
        { transform: 'translate(0,0) scale(1)', opacity: 1, filter: 'blur(0)' },
        { transform: `translate(${dx}px,${dy}px) scale(.16)`, opacity: 0, filter: 'blur(2px)' },
      ], { duration: 620, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'forwards' }).onfinish = () => g.remove();
    } else {
      g.animate([
        { transform: 'scale(1)', opacity: 1, filter: 'blur(0)' },
        { transform: 'scale(.94)', opacity: 0, filter: 'blur(3px)' },
      ], { duration: 380, easing: 'ease', fill: 'forwards' }).onfinish = () => g.remove();
    }
  });
}

/* ───────── render: top ───────── */

function renderTop() {
  const s = state.summary;
  const quiet = state.projects.filter((p) => p.quiet);
  const total = s.tasks || 0;
  const stageCount = (st) => state.tasks.filter((t) => st.includes(t.s)).length;
  const sub = total
    ? `${s.active_projects || 0} ${plural(s.active_projects || 0, 'проект', 'проекта', 'проектов')} в работе. Ждут тебя: ${s.attention || 0}.`
    : 'Доска пуста. Скажи Координатору, что сделать, — задачи появятся здесь.';
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
    ? `<div class="pop"><b>Тихие проекты</b> — без активных задач.<div class="qlist">${quiet.length ? quiet.map((p) => `<span class="qp">${esc(p.name)} <em>${p.done} сделано</em></span>`).join('') : '<span class="qp">нет</span>'}</div></div>`
    : state.pop === 'herdr'
      ? `<div class="pop"><b>Планировщик работает отдельно от herdr.</b> Данные и действия идут напрямую в <code>herdr-projects</code>. Вкладки агента и пространства herdr доступны только в окне herdr${state.herdr.running ? ', которое сейчас запущено' : '; сейчас он не запущен'}. Файлы открываются в yazi${state.local ? '' : ' только на машине с доской'}.</div>`
      : '';
  $('top').innerHTML = `
    <div class="top-left">
      <div class="lbl">HERDR-PROJECTS · ДОСКА</div>
      <h1>Herdr Planner</h1>
      <p class="sub">${esc(sub)}</p>
      <div class="head-tools">
        <button class="tool" data-act="refresh" type="button">${gl('refresh')}Обновить</button>
        <button class="tool ${state.archiveOpen ? 'hot' : ''}" data-act="archive-view" type="button">${gl('archive')}Архив${s.archived ? ` · ${s.archived}` : ''}</button>
        ${s.done ? `<button class="tool" data-act="archive-done-all" type="button" title="Убрать все сделанные задачи в архив">${gl('archive')}Убрать сделанные</button>` : ''}
        ${state.auth.required ? `<span class="who">${esc(state.auth.user)}</span><button class="tool" data-act="logout" type="button">${gl('logout')}Выйти</button>` : ''}
      </div>
    </div>
    <div class="stats">
      ${stat('Проектов', s.active_projects || 0)}
      ${stat('Задач', total)}
      ${stat('Ждут тебя', s.attention || 0, 'hot')}
      ${quietStat}
      <div class="stat"><dt class="lbl">Связь</dt><dd class="small">${herdr}</dd></div>
    </div>
    <div class="pipe">${stages}</div>
    ${pop}`;
  $('banner').innerHTML = state.board.ok ? '' : `<div class="banner"><span>${gl('failed')}</span><span class="sp">Доска не отвечает — показаны данные от ${timeOf(state.board.stale_at) || '—'}; действия выключены.</span><button class="tool" data-act="refresh" type="button">${gl('refresh')}Повторить</button></div>`;
}

function plural(n, one, few, many) {
  const m10 = n % 10, m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/* ───────── render: chips, log, queue, focus ───────── */

function renderChips() {
  const counts = {};
  for (const t of state.tasks) counts[t.s] = (counts[t.s] || 0) + 1;
  $('chips').innerHTML = ORDER.filter((s) => counts[s]).map((s) => `<button class="chip s-${s}" data-filter="${s}" aria-pressed="${state.filter && state.filter.length === 1 && state.filter[0] === s}" title="${ST[s].title}" type="button">${gl(s)}${ST[s].label} ${counts[s]}</button>`).join('');
}

function renderLog() {
  const items = state.log.slice(0, 40);
  $('laneLog').innerHTML = `<div class="lbl"><span>01 / ЖУРНАЛ ДОСКИ</span><span>${state.board.ok ? 'живой' : 'нет связи'}</span></div><div class="body">${items.length ? items.map((e) => `<div class="logline ${state.newLog.has(e.n) ? 'new' : ''}"><span class="n">${String(e.n).padStart(3, '0')}</span><span>${timeOf(e.t)}</span><span class="kk s-${e.s}">${esc(tagOf(e.id))}</span><span class="tx">${keysHTML(e.text || '')}</span></div>`).join('') : '<div class="none">Пока тихо.</div>'}</div>`;
}

function renderQueue() {
  const q = state.tasks.filter((t) => ATTN.includes(t.s)).sort((a, b) => urgent(a.s) - urgent(b.s));
  $('laneQ').innerHTML = `<div class="lbl"><span>02 / ЖДЁТ ТЕБЯ</span><span>${q.length}</span></div><div class="body">${q.length ? q.map((t) => `<button class="q s-${t.s}" data-sel="${t.id}" type="button"><span>${gl(t.s)}</span><span class="kk">${esc(t.tag)}</span><span class="tx">${esc(t.title)}</span><span class="pj">${esc(t.repo)}</span></button>`).join('') : '<div class="none">Никто не ждёт. Хорошо.</div>'}</div>`;
}

function failureText(f) {
  if (!f) return '';
  if (typeof f === 'string') return f;
  return f.text || f.message || f.error || f.detail || f.summary || JSON.stringify(f);
}

function renderFocus(keepScroll = false) {
  const lane = $('laneF');
  const scroll = keepScroll ? lane.querySelector('.focus')?.scrollTop || 0 : 0;
  if (state.archiveOpen) {
    lane.innerHTML = `<div class="lbl"><span>03 / АРХИВ</span><span>${state.archived.length}</span></div><div class="focus"><div class="f-l"><h3>Архив доски</h3><p>Сделанные задачи, убранные с доски. Верни любую — она появится среди карточек.</p><div class="f-actions"><button class="tool" data-act="archive-close" type="button">${gl('close')}Закрыть архив</button></div></div><div class="f-r arc"><div class="arc-list">${state.archived.length ? state.archived.map((t) => `<div class="arc-row"><span class="k s-done">${esc(t.tag)}</span><div><div class="t">${esc(t.title)}</div><div class="m">${esc(t.msg || t.board_label)}</div></div><button class="btn" data-act="restore" data-id="${t.id}" type="button">Вернуть</button></div>`).join('') : '<div class="arc-empty">Архив пуст.</div>'}</div></div></div>`;
    if (keepScroll) lane.querySelector('.focus').scrollTop = scroll;
    return;
  }
  const t = state.sel ? task(state.sel) : null;
  if (!t) {
    lane.innerHTML = `<div class="lbl"><span>03 / ФОКУС</span><span></span></div><div class="focus"><div class="f-l"><h3>Выбери карточку</h3><p>Стрелки ← ↑ ↓ → ходят по карточкам, Enter открывает. Здесь появятся вердикт, проверки, ветка и действия.</p></div><div class="f-r"></div></div>`;
    return;
  }
  const d = state.details[t.id];
  const dis = state.board.ok ? '' : ' aria-disabled="true"';
  const review = d?.review && typeof d.review === 'object' ? d.review : (t.review || null);
  const checks = Array.isArray(review?.checks) ? review.checks : [];
  const verdict = review?.tally ? `Ревью: ${review.tally}` : (review?.text ? 'Ревью' : '');
  const after = d?.after?.waitsFor?.length ? d.after.waitsFor.map((x) => esc(x)).join(', ') : '';
  const limits = d?.limits?.time?.used ? `время ${d.limits.time.used}${d.limits.tokens?.used ? ` · токены ${d.limits.tokens.used}` : ''}` : '';
  const worktree = d?.worktree || '';
  const reports = d?.reports_dir || '';
  const actions = (t.actions || []).filter((a) => a !== 'tell' && a !== 'answer');
  const actionButtons = actions.map((a) => {
    const armed = state.armed === `${t.id}:${a}`;
    if (a === 'approve') {
      const label = t.s === 'plan' ? 'Одобрить план' : 'Одобрить';
      return `<button class="btn go ${armed ? 'arm' : ''}" data-act="approve" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить ' + label.toLowerCase() : label}</button>`;
    }
    if (a === 'anyway') return `<button class="btn danger ${armed ? 'arm' : ''}" data-act="anyway" data-id="${t.id}"${dis} type="button">${armed ? 'Подтвердить «всё равно»' : 'Всё равно влить'}</button>`;
    return `<button class="btn" data-act="${a}" data-id="${t.id}"${dis} type="button">${ACTION_LABEL[a] || a}</button>`;
  }).join('');
  const files = t.s !== 'queued' ? `<button class="btn" data-act="files" data-id="${t.id}" type="button">${gl('folder')}Worktree</button><button class="btn" data-act="reports" data-id="${t.id}" type="button">${gl('folder')}Отчёты и заметки</button>` : '';
  const remote = state.local ? '' : `<div class="remote">Открыть файлы можно только на машине с доской. Путь: <b>${esc(worktree || reports || 'ещё нет')}</b></div>`;
  const tell = `<div class="tell"><input id="tell" data-id="${t.id}" value="${esc(state.tellDraft[t.id] || '')}" placeholder="Написать агенту ${esc(t.tag)}…" aria-label="Написать агенту" autocomplete="off"${dis}><button class="btn" data-act="tell-agent" data-id="${t.id}"${dis} type="button">Отправить</button></div>`;
  const tab = `<div class="row"><button class="btn" data-act="tab" type="button" aria-disabled="true" title="Вкладка агента существует только в herdr">${gl('ext')}Вкладка агента</button><span class="hint">Только в herdr: планировщик работает отдельно.</span></div>`;
  const reviewBlock = review?.text
    ? `<div class="h">Вердикт</div><p class="review">${keysHTML(review.text)}</p>`
    : (t.review_failed ? `<div class="h">Ревью</div><p>Ревью не прошло — можно вернуть замечания агенту или влить «всё равно».</p>` : '');
  const checksBlock = checks.length ? `<div class="h">Проверки</div>${checks.map((c) => `<div class="check">${keysHTML(c)}</div>`).join('')}` : '';
  const failureBlock = d?.failure ? `<div class="h">Ошибка</div><p class="review">${keysHTML(failureText(d.failure))}</p>` : '';
  const next = d?.failure?.next || d?.review?.next || d?.next || '';
  const planText = d?.plan?.text || '';
  const branch = d?.branch ? `<div class="kv"><span>ветка</span><b>${esc(d.branch)}</b><span>worktree</span><b>${esc(worktree || '—')}</b>${after ? `<span>после</span><b>${after}</b>` : ''}${limits ? `<span>лимиты</span><b>${esc(limits)}</b>` : ''}</div>` : '';
  lane.innerHTML = `<div class="lbl"><span>03 / ФОКУС · ${esc(t.tag)}</span><span>${ST[t.s].label}</span></div>
    <div class="focus">
      <div class="f-l">
        <div class="f-hd"><span class="key">${esc(t.tag)}</span>${badge(t.s)}<span class="agent">${esc(t.agent)}</span></div>
        <h3>${esc(t.title)}</h3>
        <p>${keysHTML(t.line || t.board_label)}</p>
        ${branch}
        <div class="f-actions">${actionButtons}${files}</div>
        ${tell}
        ${remote}
        ${tab}
      </div>
      <div class="f-r">
        ${verdict ? `<div class="h">${esc(verdict)}</div>` : ''}
        ${reviewBlock}
        ${checksBlock}
        ${failureBlock}
        ${planText ? `<div class="h">План</div><p class="review">${keysHTML(planText)}</p>` : ''}
        ${next ? `<div class="h">Дальше</div><p>${keysHTML(next)}</p>` : ''}
        ${d ? '' : `<div class="hint">Загружаю детали задачи…</div>`}
        ${d?.report ? `<div class="row"><button class="btn" data-act="report" data-url="${esc(String(d.report.url || d.report))}" type="button">${gl('ext')}Отчёт</button></div>` : ''}
      </div>
    </div>`;
  if (keepScroll) lane.querySelector('.focus').scrollTop = scroll;
}

/* ───────── console ───────── */

function renderConsole() {
  const c = state.coordinator || {};
  const sent = state.optimisticSent || c.last_sent || '';
  const reply = c.reply || '';
  const err = state.chatError || c.error || '';
  let log = '';
  if (sent) {
    log += `<div class="you"><span>›</span><span>${esc(sent)}</span></div>`;
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
  $('clog').classList.toggle('clamp', !state.expanded);
  const busy = !!c.busy;
  $('cstate').textContent = err ? '● ошибка' : busy ? '● думает' : `● ${c.status || 'готов'}`;
  $('mstate').textContent = err ? 'ошибка' : busy ? 'думает' : 'готов';
  $('win').classList.toggle('thinking', busy);
  $('win').classList.toggle('err', !!err);
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
    state.touched = true;
    state.chatError = '';
    state.optimisticSent = v;
    ta.value = '';
    sync();
    renderConsole();
    burst();
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

/* ───────── actions ───────── */

function toast(msg) {
  const n = document.createElement('div');
  n.className = 'toast';
  n.innerHTML = gl('done') + esc(msg);
  $('toasts').appendChild(n);
  setTimeout(() => n.remove(), 3200);
}

function refreshSoon() {
  setTimeout(refresh, 500);
  setTimeout(refresh, 1800);
}

function sendAction(action, extra = {}) {
  return post('/api/action', { action, ...extra })
    .then((r) => { refreshSoon(); return r; })
    .catch((e) => {
      if (e.status === 403 && e.data && e.data.path) toast(e.data.error + ': ' + e.data.path);
      else toast(e.message);
      throw e;
    });
}

async function act(cmd, id) {
  state.touched = true;
  const t = id ? task(id) : (state.sel ? task(state.sel) : null);
  if (cmd === 'refresh') { await api('/api/refresh', { method: 'POST' }).catch(() => {}); await refresh(); toast('Доска прочитана'); return; }
  if (cmd === 'logout') { await post('/api/logout').catch(() => {}); location.href = '/login'; return; }
  if (cmd === 'archive-view') { state.archiveOpen = !state.archiveOpen; renderTop(); renderFocus(); return; }
  if (cmd === 'archive-close') { state.archiveOpen = false; renderTop(); renderFocus(); return; }
  if (cmd === 'archive-done-all') {
    try { const r = await sendAction('archive-done', { scope: 'all' }); toast(`В архив убрано: ${r.archived || 0}`); } catch (e) {}
    return;
  }
  if (cmd === 'answer') {
    if (t) select(t.id);
    setTimeout(() => { const i = $('tell'); if (i) i.focus(); }, 30);
    return;
  }
  if (cmd === 'tab') { toast('Вкладка агента есть только в herdr; планировщик работает отдельно'); return; }
  if (!t) return;
  if (!state.board.ok && !['files', 'reports'].includes(cmd)) {
    toast('Нет связи с доской: действие недоступно');
    return;
  }
  if (cmd === 'approve' || cmd === 'anyway') {
    const key = `${t.id}:${cmd}`;
    if (state.armed !== key) {
      state.armed = key;
      state.armedAt = Date.now();
      renderCols(); renderFocus(true); layoutFor(0);
      setTimeout(() => {
        if (state.armed === key) { state.armed = null; renderCols(); renderFocus(true); layoutFor(0); }
      }, 3500);
      return;
    }
    // A double-click must not confirm a merge: the second press has to come
    // after the button has visibly said what it will do.
    if (Date.now() - state.armedAt < 600) {
      toast('Нажмите ещё раз, чтобы подтвердить');
      return;
    }
    state.armed = null;
    state.armedAt = 0;
    renderCols(); renderFocus(true);
    try { await sendAction(cmd, { id: t.id, confirm: true }); toast(cmd === 'approve' ? `${t.tag}: одобрено` : `${t.tag}: влито «всё равно»`); } catch (e) {}
    return;
  }
  if (cmd === 'archive') {
    state.exit[t.id] = { kind: 'cancel' };
    try { await sendAction('archive', { id: t.id }); toast(`${t.tag} убрана в архив`); } catch (e) {}
    return;
  }
  if (cmd === 'restore') {
    try { await sendAction('restore', { id: t.id }); toast(`${t.tag} возвращена на доску`); } catch (e) {}
    return;
  }
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
    try { await sendAction('tell', { id: t.id, text }); i.value = ''; delete state.tellDraft[t.id]; toast(`Агенту ${t.tag} отправлено`); } catch (e) {}
    return;
  }
  if (cmd === 'report') {
    const r = state.details[t.id]?.report;
    const url = r && (r.url || r.link || r.href || r);
    if (url && /^https?:\/\//.test(String(url))) window.open(String(url), '_blank', 'noopener');
    return;
  }
  try {
    await sendAction(cmd, { id: t.id });
    toast(`${t.tag}: ${ACTION_LABEL[cmd] || cmd}`);
  } catch (e) {}
}

/* ───────── details ───────── */

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

function select(id) {
  if (!task(id)) return;
  state.sel = id;
  state.archiveOpen = false;
  $$('.col .card').forEach((c) => c.setAttribute('aria-selected', String(c.dataset.id === id)));
  renderQueue(); renderFocus(true); layoutThreads();
  fetchDetails(id);
  const c = document.querySelector(`.col .card[data-id="${id}"]`);
  if (c && !stacked()) {
    const col = c.closest('.col'), cr = col.getBoundingClientRect(), r = c.getBoundingClientRect();
    if (r.top < cr.top || r.bottom > cr.bottom) col.scrollTop += (r.top - cr.top) - 20;
  }
  signalSelect(id);
}

/* ───────── threads (from the design; positions are real DOM) ───────── */

const thr = new Map();
const el = (tag, attrs) => {
  const n = document.createElementNS(NS, tag);
  for (const k in attrs) n.setAttribute(k, attrs[k]);
  return n;
};
const mk = (o, name, cls, attrs = {}) => {
  if (o.p[name]) return o.p[name];
  const p = el('path', { class: cls, ...attrs });
  o.g.appendChild(p);
  o.p[name] = p;
  return p;
};
function ensureG(key, build) {
  let o = thr.get(key);
  if (o && o.g.isConnected) { o.seen = true; return o; }
  const g = el('g');
  $('g-th').appendChild(g);
  o = { g, p: {}, seen: true };
  thr.set(key, o);
  build(o);
  return o;
}

function layoutThreads() {
  const win = $('win');
  if (!win || win.classList.contains('stacked') || win.classList.contains('narrow') && win.getBoundingClientRect().width < 860) {
    $$('#g-th > g').forEach((g) => g.remove());
    thr.clear();
    return;
  }
  const m = $('main').getBoundingClientRect();
  const mascot = $('mascot').getBoundingClientRect();
  const core = $('core').getBoundingClientRect();
  const mx = mascot.left + mascot.width / 2 - m.left;
  const my = mascot.top + mascot.height / 2 - m.top;
  const spineX = core.left + core.width / 2 - m.left;
  const groups = $$('.col .group');
  const rightEdge = groups.length ? Math.min(...groups.map((g) => g.getBoundingClientRect().left)) : m.right;
  const leftEdge = groups.length ? Math.max(...groups.map((g) => g.getBoundingClientRect().right)) : m.left;
  void rightEdge; void leftEdge;
  thr.forEach((o) => { o.seen = false; });

  groups.forEach((gEl) => {
    const g = { name: gEl.dataset.repo, list: visibleTasks().filter((t) => t.repo === gEl.dataset.repo) };
    const gr = gEl.getBoundingClientRect();
    const head = gEl.querySelector('.ghead');
    if (!head) return;
    const hr = head.getBoundingClientRect();
    const colEl = gEl.closest('.col');
    const cr = colEl ? colEl.getBoundingClientRect() : gr;
    // A project scrolled out of its column has no visible header: do not draw
    // a strand from an off-screen position across the rest of the dashboard.
    if (hr.bottom <= cr.top + 1 || hr.top >= cr.bottom - 1) {
      const oldT = thr.get('t:' + g.name);
      if (oldT) { oldT.g.style.display = 'none'; oldT.seen = false; }
      g.list.forEach((t) => {
        const b = thr.get('b:' + t.id);
        if (b) { b.g.style.display = 'none'; b.seen = false; }
      });
      return;
    }
    const side = state.side[g.name] || 'L';
    const port = head.querySelector('.port');
    const pr = port ? port.getBoundingClientRect() : hr;
    const sx = (side === 'L' ? pr.right + 4 : pr.left - 4) - m.left;
    const sy = pr.top + pr.height / 2 - m.top;
    const spine = side === 'L' ? (pr.right + 18 - m.left) : (pr.left - 18 - m.left);
    const hy = hr.top + hr.height / 2 - m.top;
    const attn = g.list.filter((t) => ATTN.includes(t.s)).sort((a, b) => urgent(a.s) - urgent(b.s))[0];
    const dom = attn || g.list.find((t) => t.s === 'working') || g.list.find((t) => t.s === 'review') || g.list.find((t) => t.s === 'queued') || g.list[0];
    if (!dom) return;
    const o = ensureG('t:' + g.name, (o) => {
      [-1, 0, 1].forEach((i) => mk(o, 's' + i, 'strand' + (i === 0 ? ' c' : '')));
      mk(o, 'flow', 'flow');
      mk(o, 'comet', 'comet', { pathLength: 100 });
    });
    o.g.style.display = '';
    const dx = Math.abs(sx - spine);
    [-1, 0, 1].forEach((i) => {
      const c1 = [sx + (sx > spineX ? 1 : -1) * dx * 0.55, sy + i * 5 + i * 14];
      const c2 = [spineX + (sx > spineX ? 1 : -1) * dx * 0.35, hy + i * 3 - i * 10];
      o.p['s' + i].setAttribute('d', `M${sx} ${sy + i * 5} C${c1[0]} ${c1[1]} ${c2[0]} ${c2[1]} ${spineX} ${hy + i * 3}`);
    });
    o.p.flow.setAttribute('d', o.p.s0.getAttribute('d'));
    o.p.comet.setAttribute('d', o.p.s0.getAttribute('d'));
    o.g.setAttribute('class', `th s-${dom.s}${ATTN.includes(dom.s) ? ' rev' : ''}${state.collapsed[g.name] ? ' s-done' : ''}${state.filter && !g.list.some((t) => state.filter.includes(t.s)) ? ' dim' : ''}${g.list.some((t) => t.id === state.sel) ? ' sel' : ''}`);
    o.g.dataset.repo = g.name;

    const lastY = [];
    if (!state.collapsed[g.name]) {
      g.list.forEach((t) => {
        const pe = document.querySelector(`.col .card[data-id="${t.id}"] .port`);
        if (!pe) return;
        const pr2 = pe.getBoundingClientRect();
        if (pr2.bottom <= cr.top + 1 || pr2.top >= cr.bottom - 1) {
          const oldB = thr.get('b:' + t.id);
          if (oldB) { oldB.g.style.display = 'none'; oldB.seen = false; }
          return;
        }
        let py = pr2.top + pr2.height / 2 - m.top;
        const off = py < my || py > m.bottom - m.top;
        py = Math.max(cr.top - m.top + 2, Math.min(cr.bottom - m.top - 2, py));
        const px = (side === 'L' ? pr2.right - 2 : pr2.left + 2) - m.left;
        lastY.push(py);
        const b = ensureG('b:' + t.id, (b) => {
          mk(b, 'base', 'base');
          mk(b, 'flow', 'flow');
          mk(b, 'comet', 'comet', { pathLength: 100 });
        });
        b.g.style.display = '';
        const r = 8;
        const dd = `M${spine} ${hy} V${Math.max(hy, py - r)} Q${spine} ${py} ${spine + (side === 'L' ? -r : r)} ${py} H${px}`;
        ['base', 'flow', 'comet'].forEach((n) => b.p[n].setAttribute('d', dd));
        b.g.setAttribute('class', `th s-${t.s}${ATTN.includes(t.s) ? ' rev' : ''}${off ? ' off' : ''}${state.sel === t.id ? ' sel' : ''}${state.filter && !state.filter.includes(t.s) ? ' dim' : ''}`);
        b.g.dataset.id = t.id;
      });
    }
    let p = o.p.spine;
    if (!p) { p = el('path', { class: 'spine' }); o.g.appendChild(p); o.p.spine = p; }
    const end = lastY.length ? Math.max(...lastY) : hy;
    p.setAttribute('d', `M${spine} ${hy} V${end}`);
  });
  thr.forEach((o, k) => {
    if (!o.seen) {
      if (calm()) o.g.remove();
      else { o.g.classList.add('leaving'); const g = o.g; setTimeout(() => g.remove(), 500); }
      thr.delete(k);
    }
  });
}

let rafL = 0;
const reLayout = () => {
  if (rafL) return;
  rafL = requestAnimationFrame(() => { rafL = 0; layoutThreads(); });
};
function layoutFor(ms) {
  const t0 = performance.now();
  const tick = () => { layoutThreads(); if (performance.now() - t0 < ms) requestAnimationFrame(tick); };
  tick();
}

function pkt(d, color, rev, dur) {
  if (calm() || !d) return;
  const p = el('path', { d, class: 'pkt' });
  p.style.stroke = color;
  p.style.color = color;
  $('g-pk').appendChild(p);
  const L = p.getTotalLength();
  p.style.strokeDasharray = `28 ${L + 40}`;
  const a = p.animate(
    [{ strokeDashoffset: rev ? -L : 28 }, { strokeDashoffset: rev ? 28 : -L }],
    { duration: dur || Math.max(500, L * 2.2), easing: 'cubic-bezier(.45,0,.2,1)' }
  );
  a.onfinish = () => p.remove();
  return a.finished;
}

function signal(id, rev) {
  const t = task(id);
  if (!t) return;
  const b = thr.get('b:' + id), tr = thr.get('t:' + t.repo);
  if (!b || !tr || !b.p.base.getAttribute('d')) return;
  const col = getComputedStyle(b.p.base).stroke;
  if (rev) {
    const a = pkt(b.p.base.getAttribute('d'), col, true);
    setTimeout(() => {
      pkt(tr.p.s0.getAttribute('d'), col, true);
      setTimeout(pingCore, 900);
    }, Math.max(500, b.p.base.getTotalLength() * 2.2) - 60);
  } else {
    pkt(tr.p.s0.getAttribute('d'), col, false);
    setTimeout(() => pkt(b.p.base.getAttribute('d'), col, false), Math.max(500, tr.p.s0.getTotalLength() * 2.2) - 60);
  }
}

function burst() {
  thr.forEach((o, k) => {
    if (String(k).startsWith('t')) {
      const col = getComputedStyle(o.p.s0).stroke;
      pkt(o.p.s0.getAttribute('d'), col, false, 900);
    }
  });
}

function pingCore() {
  const m = $('mascot');
  m.classList.remove('ping');
  void m.offsetWidth;
  m.classList.add('ping');
  setTimeout(() => m.classList.remove('ping'), 600);
}

function signalSelect(id) {
  const b = thr.get('b:' + id);
  if (b && !calm()) {
    const col = getComputedStyle(b.p.base).stroke;
    const t = task(id);
    pkt(b.p.base.getAttribute('d'), col, !!(t && ATTN.includes(t.s)), 700);
  }
}

/* ───────── refresher ───────── */

async function refresh() {
  if (state.refreshing) return;
  state.refreshing = true;
  try {
    const s = await api('/api/state');
    if (state.optimisticSent && s.coordinator && s.coordinator.last_sent === state.optimisticSent) state.optimisticSent = '';
    if (s.coordinator && !s.coordinator.busy && s.coordinator.reply) state.chatError = '';
    applyState(s);
    if (state.justChanged.length) for (const id of state.justChanged) setTimeout(() => signal(id, true), 320);
    if (s.board && s.board.ok === false) {
      // A quiet refresh keeps the page usable; the banner explains the stale data.
    }
  } catch (e) {
    if (e.message !== 'unauthorized') {
      state.board = { ...state.board, ok: false, error: e.message };
      renderTop();
    }
  } finally {
    state.refreshing = false;
  }
}

/* ───────── render all and events ───────── */

function renderAll() {
  renderTop();
  renderCols();
  renderChips();
  renderConsole();
  renderLog();
  state.newLog.clear();
  renderQueue();
  renderFocus(true);
  layoutFor(calm() ? 0 : 700);
}

function stacked() {
  return $('win').classList.contains('stacked');
}

document.addEventListener('click', (e) => {
  if (!e.target.closest('.pop,[data-pop]')) {
    if (state.pop) { state.pop = null; renderTop(); }
  }
  state.touched = true;
  const archRepo = e.target.closest('[data-archive-repo]');
  if (archRepo) {
    e.stopPropagation();
    sendAction('archive-done', { scope: 'project', repo: archRepo.dataset.archiveRepo })
      .then((r) => toast(`В архив убрано: ${r.archived || 0}`)).catch(() => {});
    return;
  }
  const tag = e.target.closest('[data-tag]');
  if (tag) {
    e.stopPropagation();
    const t = byTag(tag.dataset.tag);
    if (t) select(t.id);
    return;
  }
  const b = e.target.closest('[data-act],[data-pop],[data-sel],[data-group],[data-filter],[data-stage],[data-done],[data-chat],.card');
  if (!b) return;
  if (b.matches('[data-act]')) {
    e.stopPropagation();
    if (b.getAttribute('aria-disabled') === 'true' && !['tab', 'files', 'reports', 'archive-view', 'refresh'].includes(b.dataset.act)) {
      if (!state.board.ok) toast('Нет связи с доской: действие недоступно');
      return;
    }
    act(b.dataset.act, b.dataset.id);
    return;
  }
  if (b.matches('[data-pop]')) {
    e.stopPropagation();
    state.pop = state.pop === b.dataset.pop ? null : b.dataset.pop;
    renderTop();
    return;
  }
  if (b.matches('[data-sel]')) { select(b.dataset.sel); return; }
  if (b.matches('[data-group]')) {
    e.stopPropagation();
    const g = b.dataset.group;
    state.collapsed[g] = !state.collapsed[g];
    if (state.collapsed[g]) {
      $$(`.group[data-repo="${g}"] .card`).forEach((c) => { state.exit[c.dataset.id] = { kind: 'collapse' }; });
    }
    renderCols(); layoutFor(500);
    return;
  }
  if (b.matches('[data-filter]')) {
    const f = b.dataset.filter;
    state.filter = (state.filter && state.filter.length === 1 && state.filter[0] === f) ? null : [f];
    renderTop(); renderChips(); applyFilter(); return;
  }
  if (b.matches('[data-stage]')) {
    const g = STAGES.find((x) => x.k === b.dataset.stage);
    const same = state.filter && state.filter.length === g.st.length && g.st.every((s) => state.filter.includes(s));
    state.filter = same ? null : g.st.slice();
    renderTop(); renderChips(); applyFilter(); return;
  }
  if (b.matches('[data-done]')) { const g = b.dataset.done; state.doneOpen[g] = !state.doneOpen[g]; renderCols(); layoutFor(300); return; }
  if (b.matches('[data-chat]')) {
    if (b.dataset.chat === 'toggle') { state.expanded = !state.expanded; renderConsole(); }
    else if (b.dataset.chat === 'retry') { const ta = $('ask-in'); if (state.optimisticSent) { ta.value = state.optimisticSent; } $('ask').requestSubmit(); }
    return;
  }
  if (b.matches('.card')) { select(b.dataset.id); return; }
});

function applyFilter() {
  $$('.col .card').forEach((el) => el.classList.toggle('dim', !!state.filter && !state.filter.includes(el.dataset.s)));
  layoutThreads();
}

document.addEventListener('input', (e) => {
  if (e.target.id === 'tell' && e.target.dataset.id) state.tellDraft[e.target.dataset.id] = e.target.value;
});

document.addEventListener('mouseover', (e) => {
  const c = e.target.closest('.card');
  thr.forEach((o) => o.g.classList.remove('hot'));
  if (c) { const o = thr.get('b:' + c.dataset.id); if (o) o.g.classList.add('hot'); }
});

document.addEventListener('keydown', (e) => {
  state.touched = true;
  const typing = e.target.matches('textarea,input');
  if (e.key === 'Enter' && e.target.id === 'tell') {
    const btn = document.querySelector('.f-actions [data-act="tell-agent"], [data-act="tell-agent"]');
    if (btn) act('tell-agent', e.target.dataset.id);
    return;
  }
  if (typing) return;
  if (e.key === 'Enter' && e.target.matches('.card')) { select(e.target.dataset.id); return; }
  if (e.key === '/') { e.preventDefault(); $('ask-in').focus(); return; }
  if (/^[1-9]$/.test(e.key)) {
    const c = $$('.col .card').find((x) => x.querySelector('.num').textContent === pad(+e.key));
    if (c) { select(c.dataset.id); c.focus(); }
    return;
  }
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    const cs = $$('.col .card');
    const i = cs.findIndex((c) => c.dataset.id === state.sel);
    const n = cs[Math.max(0, Math.min(cs.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
    if (n) { e.preventDefault(); select(n.dataset.id); n.focus(); }
    return;
  }
  if (e.key === 'Escape' && state.pop) { state.pop = null; renderTop(); }
});

function applyWidth() {
  const w = $('win').getBoundingClientRect().width || innerWidth;
  $('win').classList.toggle('stacked', w < 1120);
  $('win').classList.toggle('narrow', w < 860);
  const lbl = document.querySelector('.sendlbl');
  if (lbl) lbl.style.display = w < 520 ? 'none' : '';
  reLayout();
}
new ResizeObserver(applyWidth).observe($('win'));
window.addEventListener('resize', applyWidth);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });

/* ───────── start ───────── */

drawMascot();
setupChat();
$('win').classList.toggle('calm', calm());
refresh();
setInterval(refresh, 4000);
