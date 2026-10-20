#!/usr/bin/env node
// A self-contained world for the planner checks: a herdr-projects stand-in
// that prints every status the board knows, details for each task, waits
// (`after`), attempts (`pick`), a Coordinator with options and a dialog, and a
// herdr stand-in. It touches no real board and logs every command under
// DIR/log/.
//
//   node scripts/fixture.mjs DIR
//
// Run the planner with:
//   HERDR_PROJECTS_BIN=DIR/bin/herdr-projects HERDR_SOCKET_PATH=DIR/herdr.sock
//   OMARCHY_THEME_DIR=DIR/theme XDG_STATE_HOME=DIR/state XDG_CONFIG_HOME=DIR/cfg
//   PATH=DIR/bin:$PATH herdr-planner --port N --data DIR/board

import { chmodSync, mkdirSync, realpathSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const dir = resolve(process.argv[2] || '');
if (!process.argv[2]) { console.error('usage: node scripts/fixture.mjs DIR'); process.exit(2); }
rmSync(dir, { recursive: true, force: true });
for (const d of ['board', 'bin', 'log', 'state', 'cfg', 'cache', 'theme', 'worktrees', 'reports']) mkdirSync(join(dir, d), { recursive: true });

const write = (path, text) => { mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, text); };
const keys = { alpha: 'ALP', beta: 'BET', gamma: 'GAM', delta: 'DEL', sigma: 'SIG' };
const rows = [];
const task = (repo, n, row) => rows.push({ id: `${repo}/${n}`, app: 'claude', ...row });
const passed = { findings: [{ id: 'F1', severity: 'minor', subject: 'Название кнопки можно короче', where: 'ui/button.rs:12', proof: 'ok', preexisting: false }], meets: true, gate: 'major', pass: true, text: 'Ревью пройдено: замечание раунда 1 исправлено, тесты зелёные.', checks: ['cargo test — 42 passed', 'ручная проверка: кнопка на месте'], by: 'claude/opus' };
const failedReview = { findings: [{ id: 'F1', severity: 'critical', subject: 'Кэш не сбрасывается при смене отчёта', where: 'cache.rs:40', proof: 'тест cache_reset падает', preexisting: false }, { id: 'F2', severity: 'major', subject: 'Нет теста на пустой отчёт', where: '', proof: '', preexisting: false }], meets: false, gate: 'major', pass: false, text: 'Нужно исправить кэш и добавить тест.', checks: ['cargo test cache_reset — failed'], by: 'claude/opus' };
const longPermission = 'Разреши, пожалуйста, удалить девять старых веток work/1–work/9 на машине с доской: все они уже влиты в main, но удаление необратимо. Ветку work/10 не трогай, на ней ещё идёт работа.';

task('alpha', 2, { state: 'blocked', kind: 'question', stage: 'working', title: 'Схема экспорта', msg: 'Какую схему брать: старую или новую? Старая совместима с клиентами, новая чище.' });
task('alpha', 3, { state: 'blocked', kind: 'permission', stage: 'plan', title: 'План миграции', msg: 'Plan ready' });
task('alpha', 4, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Тёмная тема', msg: 'Pass · 1 minor', pr: 'https://github.com/o/alpha/pull/12', review: passed, reviewRound: 1 });
task('alpha', 5, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Кэш отчётов', msg: 'Review failed · 1 critical, 1 major', review: failedReview, reviewRound: 2 });
task('alpha', 6, { app: 'omp', state: 'working', stage: 'working', title: 'Миграция базы', msg: 'Writing migration', progress: 40, eta: 12 });
task('alpha', 7, { app: 'pi', state: 'working', stage: 'reviewing', title: 'Документация', msg: 'Under review', progress: 55, reviewRound: 1 });
task('alpha', 8, { state: 'error', stage: 'working', title: 'Сбой сборки', msg: 'The build failed' });
task('alpha', 9, { state: 'working', stage: 'queued', title: 'Очередь за слотом', msg: 'Waiting for a free slot', queue: 1 });
task('alpha', 10, { state: 'working', stage: 'after', title: 'После девятой', msg: 'queued after ALP-9' });
task('alpha', 11, { state: 'idle', stage: 'todo', title: 'Заметка на потом', msg: '' });
task('alpha', 13, { state: 'done', stage: 'done', title: 'Старое дело', msg: 'Merged into main.' });
task('alpha', 15, { state: 'done', stage: 'done', title: 'Свежее дело', msg: 'Merged: https://github.com/o/alpha/pull/15', pr: 'https://github.com/o/alpha/pull/15' });
task('alpha', 16, { state: 'blocked', kind: 'permission', stage: 'working', title: 'Лишние ветки', msg: longPermission });
task('beta', 1, { state: 'done', stage: 'done', title: 'Единственное сделанное', msg: 'Merged into main.' });
task('gamma', 1, { app: 'pi', state: 'working', stage: 'working', title: 'Идёт работа', msg: 'Writing', progress: 30 });
task('gamma', 2, { state: 'done', stage: 'done', title: 'Архивная задача', msg: 'Merged into main.', archivedAt: 1792407710774 });
task('delta', 1, { app: 'codex', state: 'blocked', kind: 'permission', stage: 'approval', title: 'Проверки PR не прошли', msg: "The pull request's checks fail", pr: 'https://github.com/o/delta/pull/5', review: passed, reviewRound: 1 });
task('delta', 2, { state: 'blocked', stage: 'pick', title: 'Три варианта', msg: 'Pick one of 3' });
task('delta', 3, {
  state: 'blocked', stage: 'pick', title: 'Как кешировать', msg: 'Pick one of 3: память, диск или оба',
  attempts: [
    { letter: 'A', id: 'delta/3a', agent: 'omp/deepseek-v4-flash', standing: 'passed', stage: 'done', stat: '+120 −40', diff: 'https://example.invalid/delta/3a', report: 'http://127.0.0.1:34925/delta/3/a/report.html' },
    { letter: 'B', id: 'delta/3b', agent: 'claude/fable', standing: 'passed', stage: 'done', stat: '+90 −55', diff: 'https://example.invalid/delta/3b', report: 'http://127.0.0.1:34925/delta/3/b/report.html' },
    { letter: 'C', id: 'delta/3c', agent: 'pi/qwen', standing: 'failed', stage: 'failed', stat: '+200 −10', diff: 'https://example.invalid/delta/3c', report: '' },
  ],
});
task('sigma', 1, { state: 'blocked', stage: 'nosummary', title: 'Остановился без итога', msg: 'The agent stopped without a summary' });
task('sigma', 2, { app: 'omp', state: 'blocked', stage: 'reviewneeds', title: 'Ревьюеру нужен ты', msg: 'The reviewer needs you' });

for (const r of rows) {
  const [repo, n] = r.id.split('/');
  mkdirSync(join(dir, 'worktrees', `${repo}-${n}`), { recursive: true });
  mkdirSync(join(dir, 'reports', repo, n), { recursive: true });
}

// Dependencies the board already knows: one inside a universe, one between
// universes. `after` is not part of `list --json`; the planner asks per task.
write(join(dir, 'state', 'board.json'), JSON.stringify({
  keys,
  rows,
  after: { 'alpha/10': ['ALP-9'], 'gamma/1': ['ALP-4'] },
}, null, 1));
write(join(dir, 'state', 'coordinator.json'), JSON.stringify({
  status: 'idle · claude · claude-opus-5-5 · medium',
  reply: 'Жду задач. Доска спокойна.\n\nЧто делаем?\n1) Показать план на день\n2) Разобрать «Ждёт тебя»',
}));
// The Coordinator's Claude transcript (CLAUDE_CONFIG_DIR=DIR/claude): ten turns,
// the last one still unanswered, the way the page reads the chat history.
const chatDir = join(dir, 'board', 'coordinator', 'default', 'chat');
mkdirSync(chatDir, { recursive: true });
write(join(dir, 'board', 'coordinator', 'default', 'state.json'), JSON.stringify({ run: { kind: 'claude' }, session: 'fixture' }));
const user = (i, text) => JSON.stringify({ type: 'user', timestamp: new Date(1792389360000 + i * 60000).toISOString(), message: { role: 'user', content: text } });
const said = (i, text) => JSON.stringify({ type: 'assistant', timestamp: new Date(1792389361000 + i * 60000).toISOString(), message: { content: [{ type: 'text', text }] } });
const tool = (i) => JSON.stringify({ type: 'user', timestamp: new Date(1792389360500 + i * 60000).toISOString(), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 't', content: 'ok' }] } });
const lines = [];
for (let i = 1; i <= 10; i++) {
  lines.push(user(i, `Вопрос ${i}: как дела по ALP-${i}?`));
  lines.push(tool(i));
  if (i < 10) lines.push(said(i, `Ответ ${i}: ALP-${i} идёт по плану.`));
}
write(join(dir, 'claude', 'projects', realpathSync(chatDir).replace(/[^A-Za-z0-9]/g, '-'), 'fixture.jsonl'), lines.join('\n') + '\n');
mkdirSync(join(dir, 'state', 'herdr-planner'), { recursive: true });
write(join(dir, 'state', 'herdr-planner', 'todo.json'), JSON.stringify([
  { id: 't1', text: 'Починить кэш отчётов', at: 1792407710000 },
  { id: 't2', text: 'Добавить тёмную тему в настройки', at: 1792407710001 },
]));

const prelude = `const fs = require('node:fs'); const path = require('node:path');
const D = process.env.FIXTURE_DIR; const log = (f, o) => fs.appendFileSync(path.join(D, 'log', f), JSON.stringify(o) + '\\n');
const world = () => JSON.parse(fs.readFileSync(path.join(D, 'state', 'board.json'), 'utf8'));
const save = (w) => fs.writeFileSync(path.join(D, 'state', 'board.json'), JSON.stringify(w, null, 1));
const coord = () => JSON.parse(fs.readFileSync(path.join(D, 'state', 'coordinator.json'), 'utf8'));
const saveCoord = (c) => fs.writeFileSync(path.join(D, 'state', 'coordinator.json'), JSON.stringify(c));
const tag = (w, id) => { const i = id.lastIndexOf('/'); const repo = id.slice(0, i), n = id.slice(i + 1); return (w.keys[repo] || '#') + '-' + n; };
const say = (w, id) => w.rows.find((r) => r.id === id) ? tag(w, id) : id;
const find = (w, id) => w.rows.find((r) => r.id === id);
const byRef = (w, ref) => w.rows.find((r) => r.id === ref) || w.rows.find((r) => tag(w, r.id) === ref);
const resolve = (w, ref) => { const r = byRef(w, ref); if (r) return r.id; if (/^[A-Za-z0-9_.-]+:[^\\s]+$/.test(ref)) return ref; return null; };
const said = (w, id) => {
  const refs = (w.after || {})[id] || [];
  if (!refs.length) return id + ': waits for nothing';
  const items = refs.map((r) => { const row = byRef(w, r); return say(w, r) + (row && row.state === 'done' ? ' (done)' : ''); });
  return id + ': waits for ' + items.join(', ');
};
const plain = (w, r) => tag(w, r.id).padEnd(9) + ' ' + String(r.state).padEnd(8) + ' [' + r.app + ']  ' + r.title + (r.msg ? '  · ' + r.msg : '') + (r.archivedAt ? '  (archived)' : '');
`;

const bin = (name, body) => { const p = join(dir, 'bin', name); write(p, `#!/usr/bin/env node\n${prelude}\n${body}\n`); chmodSync(p, 0o755); };

bin('herdr-projects', `
const argv = process.argv.slice(2); const [cmd, ...rest] = argv;
if (cmd !== 'list' && cmd !== 'details' && cmd !== 'after') log('calls.jsonl', { argv });
if (cmd === 'list') {
  if (fs.existsSync(path.join(D, 'state', 'list-fails'))) { console.error('herdr-projects: the board is locked'); process.exit(1); }
  const w = world();
  const rows = argv.includes('--all') ? w.rows : w.rows.filter((r) => !r.archivedAt);
  log('lists.jsonl', { argv });
  if (argv.includes('--json')) console.log(JSON.stringify(rows));
  else console.log(rows.map((r) => plain(w, r)).join('\\n'));
} else if (cmd === 'coordinator' && rest[0] === 'tell') {
  let words = rest.slice(1);
  if (words[0] === '--') words = words.slice(1);
  const text = words.join(' ');
  const c = coord(); c.reply = 'Принял: «' + text + '». Это фикстура: ответ записан.'; saveCoord(c);
  log('coordinator.jsonl', { text });
  console.log('sent');
} else if (cmd === 'coordinator') {
  const c = coord();
  if (fs.existsSync(path.join(D, 'state', 'coordinator-dialog'))) {
    const dialog = fs.existsSync(path.join(D, 'state', 'dialog.txt')) ? fs.readFileSync(path.join(D, 'state', 'dialog.txt'), 'utf8') : 'May I continue?\\n❯ 1. Yes\\n  2. No';
    console.log('Coordinator: blocked · claude · claude-opus-5-5 · medium\\n\\n' + dialog);
  } else {
    console.log('Coordinator: ' + c.status + '\\n\\n' + c.reply);
  }
} else if (cmd === 'add' || cmd === 'new') {
  let repo = ''; const words = [];
  for (let i = 0; i < rest.length; i++) {
    if (rest[i] === '--repo') { repo = rest[i + 1] || ''; i++; }
    else if (rest[i] === '--desc' || rest[i] === '--description') { i++; }
    else words.push(rest[i]);
  }
  const title = words.join(' ');
  const w = world();
  if (!title) { console.error('add needs a title'); process.exit(1); }
  if (!repo || !w.keys[repo]) { console.error('No repo ' + repo); process.exit(1); }
  log('commands.jsonl', { argv });
  const nums = w.rows.filter((r) => r.id.startsWith(repo + '/')).map((r) => Number(r.id.split('/')[1]) || 0);
  const n = Math.max(0, ...nums) + 1;
  w.rows.push({ id: repo + '/' + n, app: 'claude', state: 'idle', stage: 'todo', title, msg: '' });
  save(w);
  fs.mkdirSync(path.join(D, 'worktrees', repo + '-' + n), { recursive: true });
  console.log(tag(w, repo + '/' + n) + ': added');
} else if (cmd === 'details') {
  const w = world(); const r = find(w, rest[0]);
  log('details.jsonl', { id: rest[0] });
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  const i = rest[0].lastIndexOf('/'); const repo = rest[0].slice(0, i), n = rest[0].slice(i + 1);
  const refs = (w.after || {})[r.id] || [];
  console.log(JSON.stringify({
    task: { id: r.id, title: r.title, stage: r.stage, msg: r.msg || '', description: 'Описание задачи ' + r.title + ': подробности, которые не влезают в строку доски.' },
    failure: r.state === 'error' ? { stage: 'agent', title: 'Сборка упала', detail: 'npm test: 3 failed\\n<script>alert(1)</script>', next: 'Повторить: агент получит ошибку.' } : null,
    review: r.review || null,
    reviews: r.review ? Array.from({ length: r.reviewRound || 1 }, () => ({})) : [],
    agent: r.app,
    agent_model: r.app + '/model-5',
    report: r.state === 'done' ? { url: 'http://127.0.0.1:34925/alpha/' + n + '/review.html' } : null,
    plan: r.stage === 'plan' ? { text: '1. Снять копию базы\\n2. Прогнать миграцию\\n3. Проверить откат', by: 'claude/opus', locked: false } : null,
    pr: r.pr || null,
    worktree: path.join(D, 'worktrees', repo + '-' + n),
    branch: 'work/' + n,
    after: { waitsFor: refs, waiting: r.stage === 'after', left: refs, waitsForYou: null, notReachable: null,
      tasks: refs.map((x) => { const row = byRef(w, x); return { id: row ? row.id : x, title: row ? row.title : x, repo: row ? row.id.split('/')[0] : '', done: !!(row && row.state === 'done'), stage: row ? row.stage : 'after' }; }) },
    attempts: r.attempts || null,
    comparison: r.attempts ? { summary: 'A и B прошли проверку, C упал' } : null,
    picked: r.picked || null,
    limits: { time: { limit: 'none', usedMs: 240000, used: '4m' }, tokens: { limit: 'none', used: 0, total: 0 } },
  }));
} else if (cmd === 'show') {
  const id = rest[1] || rest[0];
  console.log((tag(world(), id)) + ': every setting\\n* worker.agent              claude/claude-sonnet-5-5  this task\\n  worker.effort             high\\n');
} else if (cmd === 'after') {
  const w = world(); const id = resolve(w, rest[0] || '');
  if (!id || !find(w, id)) { console.error('No task ' + (rest[0] || '')); process.exit(1); }
  const clear = argv.includes('--clear');
  const refs = clear ? [] : rest.slice(1).join(' ').split(/[,\\s]+/).filter(Boolean);
  if (!clear && refs.length === 0) { console.log(said(w, id)); return; }
  const deps = refs.map((r) => resolve(w, r));
  if (deps.some((d) => !d)) { console.error('No task ' + refs[deps.findIndex((d) => !d)]); process.exit(1); }
  // The circle: does anything the new wait names reach this task?
  const graph = (w.after || {});
  const next = { ...graph, [id]: deps };
  const depsOf = (k) => (next[k] || []).map((x) => resolve(w, x)).filter(Boolean);
  const path = (from, seen) => {
    if (from === id) return [id];
    if (seen.has(from)) return null;
    seen.add(from);
    for (const x of depsOf(from)) { const p = path(x, seen); if (p) return [from, ...p]; }
    return null;
  };
  for (const d of deps) {
    const p = path(d, new Set());
    if (p) {
      console.error(say(w, id) + ' cannot wait for that: tasks would wait on each other (' + [id, ...p].map((x) => say(w, x)).join(' → ') + '). Nothing was changed.');
      process.exit(1);
    }
  }
  if (!w.after) w.after = {};
  if (deps.length) w.after[id] = deps; else delete w.after[id];
  save(w);
  log('commands.jsonl', { argv });
  console.log(said(w, id) + (deps.length === 0 ? ', so it starts now' : ''));
} else if (cmd === 'pick') {
  const w = world(); const r = find(w, rest[0]);
  log('commands.jsonl', { argv });
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  const letter = (rest[1] || '').toUpperCase();
  if (!(r.attempts || []).some((a) => a.letter === letter)) { console.error(tag(w, r.id) + ' has no attempt ' + letter + '. It has: A, B, C.'); process.exit(1); }
  r.picked = letter; r.state = 'working'; r.stage = 'working'; r.msg = 'Attempt ' + letter + ' picked';
  save(w);
  console.log(tag(w, r.id) + ': picked ' + letter);
} else if (['approve', 'review', 'retry', 'delegate', 'stop', 'tell', 'archive', 'restore', 'fix', 'finish'].includes(cmd)) {
  const w = world(); const r = find(w, rest[0]);
  log('commands.jsonl', { argv });
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  if (fs.existsSync(path.join(D, 'state', 'refuse'))) { console.error('herdr-projects: cannot ' + cmd + ': the merge has a conflict'); process.exit(1); }
  if (cmd === 'approve') {
    if (r.stage === 'plan') Object.assign(r, { state: 'working', kind: undefined, stage: 'working', msg: 'Building', progress: 1 });
    else Object.assign(r, { state: 'done', kind: undefined, stage: 'done', msg: 'Merged into main.' });
    delete r.review;
  } else if (cmd === 'review') Object.assign(r, { state: 'working', kind: undefined, stage: 'working', msg: 'Fixing what the review found', progress: 10 });
  else if (cmd === 'retry') Object.assign(r, { state: 'working', stage: 'working', msg: 'Trying again', progress: 5 });
  else if (cmd === 'fix') Object.assign(r, { state: 'working', stage: 'working', msg: 'Fixing the failure', progress: 5 });
  else if (cmd === 'finish') Object.assign(r, { state: 'done', stage: 'done', msg: 'Finished as is' });
  else if (cmd === 'delegate') Object.assign(r, { state: 'working', stage: 'working', msg: 'Just started', progress: 0 });
  else if (cmd === 'stop') Object.assign(r, { state: 'idle', stage: 'todo', msg: 'Stopped' });
  else if (cmd === 'archive') { r.archivedAt = Date.now(); r.msg = (r.msg || '') + ' (archived)'; }
  else if (cmd === 'restore') delete r.archivedAt;
  save(w);
  console.log(tag(w, r.id) + ': ' + cmd + ' ok');
} else { console.error('fixture herdr-projects: unsupported ' + argv.join(' ')); process.exit(2); }
`);

bin('herdr', `
const [cmd, sub, ...rest] = process.argv.slice(2);
const namesOf = (w) => {
  const names = ['sw-coordinator'];
  for (const r of w.rows) {
    if (r.archivedAt) continue;
    const base = 'sw-' + r.id.split('/')[0] + '-' + r.id.split('/')[1];
    names.push(base);
    if (r.stage === 'reviewneeds') names.push(base + '-r');
  }
  return names;
};
if (cmd === 'agent' && sub === 'list') {
  const w = world();
  log('herdr.jsonl', { argv: process.argv.slice(2) });
  console.log(JSON.stringify({ id: 'cli:agent:list', result: { agents: namesOf(w).map((name) => ({ name })) } }));
  return;
}
if (cmd === 'agent' && sub === 'read') {
  log('herdr.jsonl', { argv: process.argv.slice(2) });
  const name = rest[0];
  if (!namesOf(world()).includes(name)) { console.error('no such agent: ' + name); process.exit(1); }
  console.log(fs.existsSync(path.join(D, 'state', 'dialog.txt')) ? fs.readFileSync(path.join(D, 'state', 'dialog.txt'), 'utf8') : '❯ May I continue?\\n  1. Yes\\n  2. No');
  return;
}
if (cmd === 'agent' && sub === 'send-keys') {
  if (!namesOf(world()).includes(rest[0])) { console.error('no such agent: ' + rest[0]); process.exit(1); }
  log('keys.jsonl', { target: rest[0], keys: rest.slice(1) });
  console.log('sent');
  return;
}
console.error('fixture herdr: unsupported ' + process.argv.slice(2).join(' '));
process.exit(2);
`);

bin('xdg-terminal-exec', `log('terminal.jsonl', { argv: process.argv.slice(2) }); console.log('opened');`);

write(join(dir, 'theme', 'colors.toml'), `mode = "dark"
accent = "#dfa360"
selection = "#513b24"
muted = "#806f64"
background = "#1a120c"
dark_background = "#140e09"
darker_background = "#0d0906"
lighter_background = "#2a201a"
foreground = "#ecded6"
bright_foreground = "#fcf5f1"
hyprland_active_border = "rgba(dfa360ee) rgba(7ea9d5ee) 45deg"
red = "#e48682"
yellow = "#f6bb6f"
green = "#98bf7a"
cyan = "#5bcbcb"
blue = "#77b0e9"
magenta = "#da8fbf"
`);
// Omarchy's agent usage records: Claude has real subscription windows, omp
// and pi are API-key agents and must stay honestly empty. Pi's record is old
// enough to exercise the stale marking.
const usageAt = Date.now();
const inHours = (hours) => new Date(usageAt + hours * 3600 * 1000).toISOString();
const claudeLimits = [
  { label: 'Session (5-hour)', percent: 0.21, resetsAt: inHours(2) },
  // 30 hours guarantees another calendar day, so the page must add a date.
  { label: 'Weekly (7-day)', percent: 0.91, resetsAt: inHours(30) },
  { label: 'Fable Weekly', percent: 0.97, resetsAt: inHours(30), title: 'Fable Weekly' },
];
const usageDir = join(dir, 'state', 'omarchy', 'agents', 'usage');
write(join(usageDir, 'claude.json'), JSON.stringify({
  schemaVersion: 1, id: 'claude', name: 'Claude Code', updatedAt: new Date(usageAt).toISOString(),
  ready: true, hasLocalStats: true, tierLabel: 'Max 5x', usageStatusText: '', limits: claudeLimits,
  todayPrompts: 0, todaySessions: 0, todayTotalTokens: 0, todayTokensByModel: {}, recentDays: [],
  totalPrompts: 0, totalSessions: 0, activeDays: 0, activeDates: [], modelUsage: {},
}));
for (const [id, name] of [['omp', 'OMP'], ['pi', 'Pi']]) {
  write(join(usageDir, `${id}.json`), JSON.stringify({
    id, name, ready: true, usageStatusText: 'API keys · no subscription',
    totalPrompts: 0, totalSessions: 0, activeDays: 0, todayPrompts: 0, todaySessions: 0,
    todayTotalTokens: 0, todayTokensByModel: {}, recentDays: [], modelUsage: {},
    hasLocalStats: true, hasPromptStats: true,
  }));
}
write(join(dir, 'cache', 'omarchy', 'agent-usage', 'claude-limits.json'), JSON.stringify({
  fetchedAtMs: usageAt,
  limits: claudeLimits,
}));
const old = new Date(usageAt - 2 * 3600 * 1000);
utimesSync(join(usageDir, 'pi.json'), old, old);

write(join(dir, 'theme.name'), 'Fixture Night');
write(join(dir, 'herdr.sock'), '');
console.log(dir);
