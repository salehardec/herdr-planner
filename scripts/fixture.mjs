#!/usr/bin/env node
// A self-contained world for the dashboard checks: a herdr-projects stand-in
// that prints every status the board knows, details for each task, a
// Coordinator that answers `coordinator tell`, and a theme. It touches no real
// board and logs every command under DIR/log/.
//
//   node scripts/fixture.mjs DIR
//
// Run the planner with:
//   HERDR_PROJECTS_BIN=DIR/bin/herdr-projects HERDR_SOCKET_PATH=DIR/herdr.sock
//   OMARCHY_THEME_DIR=DIR/theme XDG_STATE_HOME=DIR/state XDG_CONFIG_HOME=DIR/cfg
//   PATH=DIR/bin:$PATH herdr-planner --port N --data DIR/board

import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const dir = resolve(process.argv[2] || '');
if (!process.argv[2]) { console.error('usage: node scripts/fixture.mjs DIR'); process.exit(2); }
rmSync(dir, { recursive: true, force: true });
for (const d of ['board', 'bin', 'log', 'state', 'cfg', 'theme', 'worktrees', 'reports']) mkdirSync(join(dir, d), { recursive: true });

const write = (path, text) => { mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, text); };
const keys = { alpha: 'ALP', beta: 'BET', gamma: 'GAM', delta: 'DEL', sigma: 'SIG' };
const rows = [];
const task = (repo, n, row) => rows.push({ id: `${repo}/${n}`, app: 'claude', ...row });
const passed = { findings: [{ id: 'F1', severity: 'minor', subject: 'Название кнопки можно короче', where: 'ui/button.rs:12', proof: 'ok', preexisting: false }], meets: true, gate: 'major', pass: true, text: 'Ревью пройдено: замечание раунда 1 исправлено, тесты зелёные.', checks: ['cargo test — 42 passed', 'ручная проверка: кнопка на месте'], by: 'claude/opus' };
const failedReview = { findings: [{ id: 'F1', severity: 'critical', subject: 'Кэш не сбрасывается при смене отчёта', where: 'cache.rs:40', proof: 'тест cache_reset падает', preexisting: false }, { id: 'F2', severity: 'major', subject: 'Нет теста на пустой отчёт', where: '', proof: '', preexisting: false }], meets: false, gate: 'major', pass: false, text: 'Нужно исправить кэш и добавить тест.', checks: ['cargo test cache_reset — failed'], by: 'claude/opus' };

task('alpha', 2, { state: 'blocked', kind: 'question', stage: 'working', title: 'Схема экспорта', msg: 'Какую схему брать: старую или новую?' });
task('alpha', 3, { state: 'blocked', kind: 'permission', stage: 'plan', title: 'План миграции', msg: 'Plan ready' });
task('alpha', 4, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Тёмная тема', msg: 'Pass · 1 minor', pr: 'https://github.com/o/alpha/pull/12', review: passed, reviewRound: 1 });
task('alpha', 5, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Кэш отчётов', msg: 'Review failed · 1 critical, 1 major', review: failedReview, reviewRound: 2 });
task('alpha', 6, { state: 'working', stage: 'working', title: 'Миграция базы', msg: 'Writing migration', progress: 40, eta: 12 });
task('alpha', 7, { state: 'working', stage: 'reviewing', title: 'Документация', msg: 'Under review', progress: 55, reviewRound: 1 });
task('alpha', 8, { state: 'error', stage: 'working', title: 'Сбой сборки', msg: 'The build failed' });
task('alpha', 9, { state: 'working', stage: 'queued', title: 'Очередь за слотом', msg: 'Waiting for a free slot', queue: 1 });
task('alpha', 10, { state: 'working', stage: 'after', title: 'После девятой', msg: 'queued after ALP-9' });
task('alpha', 11, { state: 'idle', stage: 'todo', title: 'Заметка на потом', msg: '' });
task('alpha', 13, { state: 'done', stage: 'done', title: 'Старое дело', msg: 'Merged into main.' });
task('alpha', 15, { state: 'done', stage: 'done', title: 'Свежее дело', msg: 'Merged: https://github.com/o/alpha/pull/15', pr: 'https://github.com/o/alpha/pull/15' });
task('beta', 1, { state: 'done', stage: 'done', title: 'Единственное сделанное', msg: 'Merged into main.' });
task('gamma', 1, { state: 'working', stage: 'working', title: 'Идёт работа', msg: 'Writing', progress: 30 });
task('gamma', 2, { state: 'done', stage: 'done', title: 'Архивная задача', msg: 'Merged into main.', archivedAt: 1792407710774 });
task('delta', 1, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Проверки PR не прошли', msg: "The pull request's checks fail", pr: 'https://github.com/o/delta/pull/5', review: passed, reviewRound: 1 });
task('delta', 2, { state: 'blocked', stage: 'pick', title: 'Три варианта', msg: 'Pick one of 3' });
task('sigma', 1, { state: 'blocked', stage: 'nosummary', title: 'Остановился без итога', msg: 'The agent stopped without a summary' });
task('sigma', 2, { state: 'blocked', stage: 'reviewneeds', title: 'Ревьюеру нужен ты', msg: 'The reviewer needs you' });

for (const r of rows) {
  const [repo, n] = r.id.split('/');
  mkdirSync(join(dir, 'worktrees', `${repo}-${n}`), { recursive: true });
  mkdirSync(join(dir, 'reports', repo, n), { recursive: true });
}

write(join(dir, 'state', 'board.json'), JSON.stringify({ keys, rows }, null, 1));
write(join(dir, 'state', 'coordinator.json'), JSON.stringify({ status: 'idle · claude · claude-opus-5-5 · medium', reply: 'Жду задач. Доска спокойна.' }));

const prelude = `const fs = require('node:fs'); const path = require('node:path');
const D = process.env.FIXTURE_DIR; const log = (f, o) => fs.appendFileSync(path.join(D, 'log', f), JSON.stringify(o) + '\\n');
const world = () => JSON.parse(fs.readFileSync(path.join(D, 'state', 'board.json'), 'utf8'));
const save = (w) => fs.writeFileSync(path.join(D, 'state', 'board.json'), JSON.stringify(w, null, 1));
const coord = () => JSON.parse(fs.readFileSync(path.join(D, 'state', 'coordinator.json'), 'utf8'));
const saveCoord = (c) => fs.writeFileSync(path.join(D, 'state', 'coordinator.json'), JSON.stringify(c));
const tag = (w, id) => { const i = id.lastIndexOf('/'); const repo = id.slice(0, i), n = id.slice(i + 1); return (w.keys[repo] || '#') + '-' + n; };
const find = (w, id) => w.rows.find((r) => r.id === id);
const plain = (w, r) => tag(w, r.id).padEnd(9) + ' ' + String(r.state).padEnd(8) + ' [' + r.app + ']  ' + r.title + (r.msg ? '  · ' + r.msg : '') + (r.archivedAt ? '  (archived)' : '');
`;

const bin = (name, body) => { const p = join(dir, 'bin', name); write(p, `#!/usr/bin/env node\n${prelude}\n${body}\n`); chmodSync(p, 0o755); };

bin('herdr-projects', `
const argv = process.argv.slice(2); const [cmd, ...rest] = argv;
if (cmd !== 'list' && cmd !== 'details') log('calls.jsonl', { argv });
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
  console.log('Coordinator: ' + c.status + '\\n\\n' + c.reply);
} else if (cmd === 'details') {
  const w = world(); const r = find(w, rest[0]);
  log('details.jsonl', { id: rest[0] });
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  const i = rest[0].lastIndexOf('/'); const repo = rest[0].slice(0, i), n = rest[0].slice(i + 1);
  console.log(JSON.stringify({
    task: { id: r.id, title: r.title, stage: r.stage, msg: r.msg || '', description: 'Описание задачи ' + r.title },
    failure: r.state === 'error' ? { stage: 'agent', title: 'Сборка упала', detail: 'npm test: 3 failed\\n<script>alert(1)</script>', next: 'Повторить: агент получит ошибку.' } : null,
    review: r.review || null,
    reviews: r.review ? Array.from({ length: r.reviewRound || 1 }, () => ({})) : [],
    agent: r.app,
    report: r.state === 'done' ? { url: 'http://127.0.0.1:34925/alpha/' + n + '/review.html' } : null,
    plan: r.stage === 'plan' ? { text: '1. Снять копию базы\\n2. Прогнать миграцию\\n3. Проверить откат' } : null,
    pr: r.pr || null,
    worktree: path.join(D, 'worktrees', repo + '-' + n),
    branch: 'work/' + n,
    after: r.stage === 'after' ? { waitsFor: ['ALP-9'], waiting: true, tasks: [] } : { waitsFor: [], waiting: false, tasks: [] },
    limits: { time: { limit: 'none', usedMs: 240000, used: '4m' }, tokens: { limit: 'none', used: 0, total: 0 } },
  }));
} else if (cmd === 'show') {
  const id = rest[1] || rest[0];
  console.log((tag(world(), id)) + ': every setting\\n* worker.agent              claude/claude-sonnet-5-5  this task\\n  worker.effort             high\\n');
} else if (['approve', 'review', 'retry', 'delegate', 'stop', 'tell', 'archive', 'restore'].includes(cmd)) {
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
  else if (cmd === 'delegate') Object.assign(r, { state: 'working', stage: 'working', msg: 'Just started', progress: 0 });
  else if (cmd === 'stop') Object.assign(r, { state: 'idle', stage: 'todo', msg: 'Stopped' });
  else if (cmd === 'archive') { r.archivedAt = Date.now(); r.msg = (r.msg || '') + ' (archived)'; }
  else if (cmd === 'restore') delete r.archivedAt;
  save(w);
  console.log(tag(w, r.id) + ': ' + cmd + ' ok');
} else { console.error('fixture herdr-projects: unsupported ' + argv.join(' ')); process.exit(2); }
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
write(join(dir, 'theme.name'), 'Fixture Night');
write(join(dir, 'herdr.sock'), '');
console.log(dir);
