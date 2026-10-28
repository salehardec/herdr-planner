#!/usr/bin/env node
// A self-contained world for the planner checks, with no real board behind it.
//
//   node scripts/fixture.mjs DIR [--scale 50x500]
//
// DIR gets:
//   bin/        stand-ins on PATH: herdr-projects, herdr, claude (translation), gh, glab,
//               xdg-terminal-exec, nvidia-smi (answers from load/nvidia.csv).
//               Each is a node script built from the functions below (nvidia-smi is a shell script).
//   state/      the stand-ins' own world: board.json (rows, waits, meta), coordinator.json,
//               todo.json (the planner's), the Omarchy usage records.
//   board/      what the planner reads as the board's files: repos.json, repo-keys.json,
//               tasks/<repo>/<n>/task.json and closing.md, coordinator/default/…
//   projects/   the repositories' folders in two groups (AI, herdr); alpha, beta are git clones
//               of origins/ (alpha's clone is two commits behind its origin).
//   claude/, omp/, codex/   the sub-agents' session files (CLAUDE_CONFIG_DIR, omp sessions, CODEX_HOME).
//   log/        one JSON line per command a stand-in got (calls, commands, claude, …).
// Every name, id and env var is listed in scripts/fixture-notes.md.
//
// Run the planner on it (run-checks.sh does this):
//   HERDR_PROJECTS_BIN=DIR/bin/herdr-projects HERDR_SOCKET_PATH=DIR/herdr.sock
//   OMARCHY_THEME_DIR=DIR/theme XDG_STATE_HOME=DIR/state XDG_CONFIG_HOME=DIR/cfg
//   CLAUDE_CONFIG_DIR=DIR/claude CODEX_HOME=DIR/codex PATH=DIR/bin:$PATH
//   herdr-planner --port N --data DIR/board
// The time-dependent parts (created, mtimes, rate limits) are relative to now; the rest is fixed.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';

const MIN = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;
const NOW = Date.now();
// Where a task created `ago` ago falls on the calendar must not depend on the time of day the fixture is built: «today»
// (under 12 h) stays after this midnight, «yesterday» (24–48 h) lies in the day before, the older ones are days back.
const MIDNIGHT = new Date(NOW).setHours(0, 0, 0, 0);
const createdAgo = (ago) => {
  // whole milliseconds: the planner reads `created` as an integer
  if (ago >= 12 * HOUR && ago < 2 * DAY) return MIDNIGHT - (ago - DAY);
  if (ago < 12 * HOUR) return Math.round(NOW - ago * Math.min(1, (0.9 * (NOW - MIDNIGHT)) / (7 * HOUR)));
  return NOW - ago;
};
const iso = (ms) => new Date(ms).toISOString();

// ---------------------------------------------------------------- arguments

const args = process.argv.slice(2);
const pos = [];
let scaleSpec = null;
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--scale') scaleSpec = args[++i] ?? '';
  else pos.push(args[i]);
}
if (!pos[0]) { console.error('usage: node scripts/fixture.mjs DIR [--scale 50x500]'); process.exit(2); }
let scale = null;
if (scaleSpec !== null) {
  const m = /^(\d+)x(\d+)$/.exec(scaleSpec);
  if (!m || +m[1] < 1 || +m[2] < 1) { console.error('--scale wants UNIVERSESxPLANETS, for example 50x500'); process.exit(2); }
  scale = { universes: +m[1], planets: +m[2] };
}

const root = path.resolve(pos[0]);
fs.rmSync(root, { recursive: true, force: true });
fs.mkdirSync(root, { recursive: true });
const dir = fs.realpathSync(root);
for (const d of ['board', 'bin', 'log', 'state', 'state/herdr-planner', 'cfg', 'cache', 'theme', 'worktrees', 'reports', 'projects', 'origins', 'claude', 'codex', 'omp']) {
  fs.mkdirSync(path.join(dir, d), { recursive: true });
}

const write = (p, text) => { fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, text); };
const jl = (items) => items.map((x) => JSON.stringify(x)).join('\n') + '\n';

// ---------------------------------------------------------------- shared helpers
// `common` is also copied into every stand-in (Function.toString), so it must not
// use anything from this file but its arguments.

function common(DATA, mods) {
  const { fs, path, crypto } = mods;
  const D = DATA.dir;
  const log = (file, o) => fs.appendFileSync(path.join(D, 'log', file), JSON.stringify(o) + '\n');
  const readJson = (p, fallback) => { try { return JSON.parse(fs.readFileSync(p, 'utf8')); } catch (e) { return fallback; } };
  const world = () => readJson(path.join(D, 'state', 'board.json'), null);
  const save = (w) => fs.writeFileSync(path.join(D, 'state', 'board.json'), JSON.stringify(w, null, 1));
  const coord = () => readJson(path.join(D, 'state', 'coordinator.json'), { status: 'idle', reply: '' });
  const saveCoord = (c) => fs.writeFileSync(path.join(D, 'state', 'coordinator.json'), JSON.stringify(c));
  const tag = (w, id) => { const i = id.lastIndexOf('/'); const repo = id.slice(0, i), n = id.slice(i + 1); return (w.keys[repo] || '#') + '-' + n; };
  // A task by id (`alpha/10`) or by its tag (`ALP-10`, as the board's own commands take it).
  const find = (w, ref) => w.rows.find((r) => r.id === ref) || w.rows.find((r) => tag(w, r.id) === ref);
  const say = (w, id) => (w.rows.find((r) => r.id === id) ? tag(w, id) : id);
  // A ref the board would accept: a task here, or `label:TAG` of another machine.
  const resolve = (w, ref) => { const r = find(w, ref); if (r) return r.id; if (/^[A-Za-z0-9_.-]+:[^\s]+$/.test(ref)) return ref; return null; };
  const plain = (w, r) => tag(w, r.id).padEnd(9) + ' ' + String(r.state).padEnd(8) + ' [' + r.app + ']  ' + r.title + (r.msg ? '  · ' + r.msg : '') + (r.archivedAt ? '  (archived)' : '');
  const said = (w, id) => {
    const refs = (w.after || {})[id] || [];
    if (!refs.length) return id + ': waits for nothing';
    const items = refs.map((r) => { const row = find(w, r); return say(w, r) + (row && row.state === 'done' ? ' (done)' : ''); });
    return id + ': waits for ' + items.join(', ');
  };
  const sleepMs = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
  const uuid = (s) => {
    const h = crypto.createHash('md5').update(String(s)).digest('hex');
    return [h.slice(0, 8), h.slice(8, 12), '4' + h.slice(13, 16), '8' + h.slice(17, 20), h.slice(20, 32)].join('-');
  };
  const nextN = (w, repo) => Math.max(0, ...w.rows.filter((r) => r.id.startsWith(repo + '/')).map((r) => Number(r.id.split('/')[1]) || 0)) + 1;
  // The board's repos.json (name -> folder) and repo-keys.json (`had`: key -> name).
  const publishRepos = (w) => {
    fs.writeFileSync(path.join(D, 'board', 'repos.json'), JSON.stringify(w.folders, null, 1));
    const had = {};
    for (const [name, key] of Object.entries(w.keys)) had[key] = name;
    // An older key of alpha, kept in `had`: containers labelled AL9-4 belong to alpha/4 too.
    had.AL9 = 'alpha';
    fs.writeFileSync(path.join(D, 'board', 'repo-keys.json'), JSON.stringify({ upgraded: true, had }, null, 1));
  };
  // The task's own record: board/tasks/<repo>/<n>/task.json, as the board keeps it.
  const publishTask = (w, row) => {
    const [repo, n] = row.id.split('/');
    const m = (w.meta || {})[row.id] || {};
    const dirT = path.join(D, 'board', 'tasks', repo, n);
    fs.mkdirSync(dirT, { recursive: true });
    const after = ((w.after || {})[row.id] || []).map((r) => resolve(w, r) || r);
    fs.writeFileSync(path.join(dirT, 'task.json'), JSON.stringify({
      id: row.id, repo, n: Number(n), root: (w.folders || {})[repo] || '', title: row.title,
      description: m.description || '', owner: 'me', app: row.app, stage: row.stage, state: row.state,
      created: m.created || 0, at: m.at || m.created || 0, settings: m.settings || {}, model: m.model ?? null,
      agent: row.app, reviewerOn: m.reviewerOn ?? null,
      session: m.session || '', worktree: m.worktree || '', branch: m.branch || '',
      after, afterDone: m.afterDone || [], exitedAt: m.exitedAt ?? null,
      ...(m.gone ? { last: { status: null, gone: true } } : {}),
    }, null, 2));
  };
  // The key the board suggests: the last word's first letter and the next consonants (`newrepo` → NWR).
  const suggestKey = (name, taken) => {
    const words = name.split(/[-_.]/).filter(Boolean);
    const last = words[words.length - 1] || name;
    const cons = last.toLowerCase().replace(/[aeiouy]/g, '');
    let key = (last[0] + cons.slice(1)).slice(0, 3).toUpperCase();
    if (taken.includes(key)) key = name.replace(/[^A-Za-z0-9]/g, '').slice(0, 3).toUpperCase();
    return key;
  };
  // `add TITLE --repo /abs/path`: a git folder not yet on the board is registered (name = folder name, key from it).
  const registerFolder = (w, p) => {
    const real = fs.realpathSync(p);
    if (!fs.existsSync(path.join(real, '.git'))) throw new Error('herdr-projects: ' + real + ' is not a git folder');
    const same = Object.keys(w.folders).find((n) => w.folders[n] === real);
    if (same) return same;
    const name = path.basename(real);
    if (w.keys[name]) throw new Error('herdr-projects: a repo named ' + name + ' is on the board already');
    w.keys[name] = suggestKey(name, Object.values(w.keys));
    w.folders[name] = real;
    w.settings[name] = {};
    publishRepos(w);
    return name;
  };
  // The board's settings as its own commands show them: `show [--repo R] --json`, `set NAME VALUE [--repo R]`,
  // `unset NAME [--repo R]`. The file keeps one level per name: '' is the home board, a repo its own. `cfg` has the
  // models and effort levels per program, the built-in values and the repos' keys.
  const boardSettings = (file, cfg) => {
    const load = () => readJson(file, {});
    const kindOf = (name) => (/^(coordinator|board|watcher)\./.test(name) ? 'board' : 'work');
    const effective = (store, repo, name) => ((repo && store[repo] && store[repo][name] !== undefined) ? store[repo][name] : (store[''] || {})[name]);
    // A level is the home board (''), one repository, or one task (`@task:repo/n`); instructions add up, so the
    // inherited levels are listed after the own ones.
    const levelOf = (repo, task) => (task ? '@task:' + task : (repo || ''));
    // The board's real level files, which the planner reads for a scope's own instructions (its `show` joins levels):
    // settings/config.json for you and the repositories, board/tasks/<repo>/<n>/task.json for a task.
    const settingsFile = path.join(D, 'settings', 'config.json');
    const mirror = () => {
      const store = load();
      const out = {};
      for (const [level, vals] of Object.entries(store)) {
        if (level.startsWith('@task:')) continue;
        const target = level ? ((out.repos ??= {})[level] ??= {}) : out;
        for (const [name, value] of Object.entries(vals)) {
          const parts = name.split('.');
          let node = target;
          for (const p of parts.slice(0, -1)) node = node[p] ??= {};
          node[parts.at(-1)] = value;
        }
      }
      fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
      fs.writeFileSync(settingsFile, JSON.stringify(out, null, 1));
      for (const [level, vals] of Object.entries(store)) {
        if (!level.startsWith('@task:')) continue;
        const [repo, n] = level.slice(6).split('/');
        const tf = path.join(D, 'board', 'tasks', repo, n, 'task.json');
        if (!fs.existsSync(tf)) continue;
        const rec = readJson(tf, { settings: {} });
        rec.settings = rec.settings || {};
        delete rec.settings['worker.instructions'];
        delete rec.settings['reviewer.instructions'];
        for (const [name, value] of Object.entries(vals)) rec.settings[name] = value;
        fs.writeFileSync(tf, JSON.stringify(rec, null, 1));
      }
    };
    const show = (repo, task) => {
      const store = load();
      const home = store[''] || {};
      const own = store[levelOf(repo, task)] || {};
      const list = [];
      const where = task ? 'this task' : repo ? 'repo' : 'you';
      const place = task ? 'task' : repo ? 'repo' : 'you';
      if (repo && !task && cfg.keys[repo]) list.push({ name: 'repo.key', kind: 'board', value: cfg.keys[repo], from: 'repo', place: 'repo', own: true, note: '' });
      // Instructions add up, broadest first: the real `show` gives the joined value and still says `own: true` when
      // the scope itself has words. The planner must not take that joined value as the scope's own text (PLN-45).
      const INSTR = new Set(['worker.instructions', 'reviewer.instructions']);
      const levels = task ? ['', repo, levelOf(repo, task)] : repo ? ['', repo] : [''];
      const label = (l) => (task && l === levelOf(repo, task) ? 'this task' : l === repo && repo ? 'repo' : 'you');
      for (const name of INSTR) {
        const parts = [];
        for (const l of levels) {
          const v = (store[l] || {})[name];
          if (v === undefined) continue;
          if (v === 'none') { parts.length = 0; continue; }
          parts.push({ l, v });
        }
        if (!parts.length && own[name] === undefined) continue;
        const value = parts.length ? parts.map((p) => p.v).join('\n\n') : 'none';
        const ownHere = own[name] !== undefined;
        const near = parts[parts.length - 1];
        list.push({
          name, kind: kindOf(name), value, own: ownHere,
          place: ownHere ? place : (near?.l ? (near.l === repo ? 'repo' : 'task') : 'you'),
          from: parts.map((p) => label(p.l)).join(' + ') || where,
          note: '',
        });
      }
      for (const [name, value] of Object.entries(own)) {
        if (INSTR.has(name)) continue;
        list.push({ name, kind: kindOf(name), value, from: where, place, own: true, note: '' });
      }
      for (const level of task ? [repo, ''] : repo ? [''] : []) {
        for (const [name, value] of Object.entries(store[level] || {})) {
          if (INSTR.has(name) || list.some((x) => x.name === name)) continue;
          list.push({ name, kind: kindOf(name), value, from: level ? 'repo' : 'you', place: level ? 'repo' : 'you', own: false, note: '' });
        }
      }
      for (const [name, value] of Object.entries(cfg.builtin)) {
        if (list.some((x) => x.name === name)) continue;
        list.push({ name, kind: kindOf(name), value, from: 'built in', place: 'builtin', own: false, note: name === 'reviewer.fast' ? 'does nothing: claude has no fast mode' : '' });
      }
      if (task) return { settings: list };
      return repo ? { repo: { name: repo, key: cfg.keys[repo] || '', folder: '' }, settings: list } : { settings: list };
    };
    const refuse = (store, name, value, repo) => {
      if (!/^[a-z][A-Za-z0-9.]*$/.test(name)) return 'no such setting: ' + name;
      if (/\.agent$/.test(name)) {
        const program = String(value).split('/')[0];
        const names = cfg.models[program];
        if (!names) return name + ': ' + value + ' is not a program the board knows; herdr-projects models lists the ones it does';
        if (!names.includes(value)) return name + ': ' + value + ' is not a model ' + program + ' takes. Run: herdr-projects models ' + program;
      }
      if (/\.effort$/.test(name)) {
        const agent = String(effective(store, repo, name.replace(/\.effort$/, '.agent')) || '');
        const program = agent.split('/')[0];
        const levels = cfg.efforts[program];
        if (levels && !levels.includes(value)) return name + ': ' + value + ' is not an effort ' + program + ' takes: ' + levels.join(', ');
      }
      return null;
    };
    const set = (name, value, repo, task) => {
      const store = load();
      const problem = refuse(store, name, value, repo);
      if (problem) return { error: problem };
      const level = levelOf(repo, task);
      store[level] = store[level] || {};
      store[level][name] = value;
      fs.writeFileSync(file, JSON.stringify(store, null, 1));
      mirror();
      return { ok: true };
    };
    const unset = (name, repo, task) => {
      const store = load();
      const level = levelOf(repo, task);
      if (store[level]) delete store[level][name];
      fs.writeFileSync(file, JSON.stringify(store, null, 1));
      mirror();
      return { ok: true };
    };
    return { show, set, unset };
  };
  return { boardSettings, D, fs, path, log, readJson, world, save, coord, saveCoord, tag, find, say, resolve, plain, said, sleepMs, uuid, nextN, publishRepos, publishTask, suggestKey, registerFolder };
}

// ---------------------------------------------------------------- the world

const passed = { findings: [{ id: 'F1', severity: 'minor', subject: 'Название кнопки можно короче', where: 'ui/button.rs:12', proof: 'ok', preexisting: false }], meets: true, gate: 'major', pass: true, text: 'Ревью пройдено: замечание раунда 1 исправлено, тесты зелёные.', checks: ['cargo test — 42 passed', 'ручная проверка: кнопка на месте'], by: 'claude/opus' };
const failedReview = { findings: [{ id: 'F1', severity: 'critical', subject: 'Кэш не сбрасывается при смене отчёта', where: 'cache.rs:40', proof: 'тест cache_reset падает', preexisting: false }, { id: 'F2', severity: 'major', subject: 'Нет теста на пустой отчёт', where: '', proof: '', preexisting: false }], meets: false, gate: 'major', pass: false, text: 'Нужно исправить кэш и добавить тест.', checks: ['cargo test cache_reset — failed'], by: 'claude/opus' };
const longPermission = 'Разреши, пожалуйста, удалить девять старых веток work/1–work/9 на машине с доской: все они уже влиты в main, но удаление необратимо. Ветку work/10 не трогай, на ней ещё идёт работа.';

const rows = [];
const task = (repo, n, row) => rows.push({ id: `${repo}/${n}`, app: 'claude', ...row });

task('alpha', 2, { state: 'blocked', kind: 'question', stage: 'working', title: 'Схема экспорта', msg: 'Какую схему брать: старую или новую? Старая совместима с клиентами, новая чище.' });
task('alpha', 3, { state: 'blocked', kind: 'permission', stage: 'plan', title: 'План миграции', msg: 'Plan ready' });
task('alpha', 4, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Тёмная тема', msg: 'Pass · 1 minor', pr: 'https://github.com/o/alpha/pull/12', review: passed, reviewRound: 1 });
task('alpha', 5, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Кэш отчётов', msg: 'Review failed · 1 critical, 1 major', review: failedReview, reviewRound: 2 });
task('alpha', 6, { app: 'omp', state: 'working', stage: 'working', title: 'Миграция базы', msg: 'Writing migration', progress: 40, eta: 12 });
task('alpha', 7, { app: 'pi', state: 'working', stage: 'reviewing', title: 'Документация', msg: 'Under review', progress: 55, reviewRound: 1 });
task('alpha', 8, { state: 'error', stage: 'working', title: 'Сбой сборки', msg: 'The build failed', exitedAt: NOW - 5 * MIN });
task('alpha', 9, { state: 'working', stage: 'queued', title: 'Очередь за слотом', msg: 'Waiting for a free slot', queue: 1 });
task('alpha', 10, { state: 'working', stage: 'after', title: 'После девятой', msg: 'queued after ALP-9' });
task('alpha', 11, { state: 'idle', stage: 'todo', title: 'Заметка на потом', msg: '' });
task('alpha', 13, { state: 'done', stage: 'done', title: 'Старое дело', msg: 'Merged into main.' });
task('alpha', 15, { state: 'done', stage: 'done', title: 'Свежее дело', msg: 'Merged: https://github.com/o/alpha/pull/15', pr: 'https://github.com/o/alpha/pull/15' });
task('alpha', 16, { state: 'blocked', kind: 'permission', stage: 'working', title: 'Лишние ветки', msg: longPermission });
// A question the board has no line for: an empty message reaches the page as the board's own «Needs you» (PLN-61).
task('alpha', 20, { state: 'blocked', kind: 'question', stage: 'working', title: 'Вопрос без текста', msg: 'Needs you' });
// The PLN-60 case itself: an agent stopped on a permission dialog. The board knows it as a question, or as nothing at all.
task('alpha', 21, { state: 'blocked', kind: 'question', stage: 'working', title: 'Диалог разрешения', msg: 'Needs you' });
task('alpha', 22, { state: 'blocked', stage: 'working', title: 'Диалог без вида', msg: 'Needs you' });
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
// An archived task of a quiet repo: `docs` has no live work at all.
task('docs', 1, { state: 'done', stage: 'done', title: 'Архивный разбор', msg: 'Merged into main.', archivedAt: NOW - 9 * DAY });

const KEYS = { alpha: 'ALP', beta: 'BET', gamma: 'GAM', delta: 'DEL', sigma: 'SIG', omega: 'OMG', docs: 'DOC' };
// Two groups of folders: the galaxy shows an arm per group.
const GROUP = { alpha: 'AI', beta: 'AI', gamma: 'AI', omega: 'AI', delta: 'herdr', sigma: 'herdr', docs: 'herdr' };
const folders = {};
for (const [repo, g] of Object.entries(GROUP)) folders[repo] = path.join(dir, 'projects', g, repo);

// Per task: when it was made, its text, its worker when it is not the default for its app.
const META = {
  'alpha/2': { ago: 25 * MIN, desc: 'Схема экспорта отчётов: старая совместима с клиентами, новая чище, но ломает два старых эндпоинта. Решить, какую брать, и проверить на тестовом отчёте.' },
  'alpha/3': { ago: 2 * HOUR, desc: 'Plan the migration of the reports table to the new schema: snapshot, dry run, rollback steps. Keep the old endpoints working during the switch.' },
  'alpha/4': { ago: 3 * HOUR, desc: 'Тёмная тема для страницы отчётов: цвета из палитры Omarchy, контраст не ниже AA.' },
  'alpha/5': { ago: 50 * MIN, desc: 'Cache of report aggregates: invalidate on dataset change. The review found a stale-cache bug, see the findings.' },
  'alpha/6': { ago: 5 * HOUR, desc: 'Миграция базы под новую схему отчётов: таблицы, индексы и откат. Работать в своей ветке, main не трогать.', worker: 'omp/deepseek/deepseek-v4-flash', model: 'deepseek-v4-flash' },
  'alpha/7': { ago: 26 * HOUR, desc: 'Document the export API for the new schema, examples included.', worker: 'pi/default', model: null, reviewer: 'pi/kimi-coding/k3.1-pro' },
  'alpha/8': { ago: 3 * DAY, desc: 'Сборка падает на шаге линковки после обновления зависимостей. Найти причину и починить.' },
  'alpha/9': { ago: 2 * HOUR, desc: 'Run the nightly import of tenant reports; the worker waits for a free slot.' },
  'alpha/10': { ago: 30 * HOUR, desc: 'Follow-up of the nightly import: fix retries once ALP-9 is merged.', afterDone: [] },
  'alpha/11': { ago: 9 * DAY, desc: 'Заметка на потом: проверить, нужен ли HTTP-кеш для экспорта.' },
  'alpha/13': { ago: 9 * DAY, desc: 'Старое дело: убрать устаревший флаг совместимости из конфига.', noExit: true },
  'alpha/15': { ago: 4 * HOUR, desc: 'Export button now shows progress; the job reports percent through the existing event channel.' },
  'alpha/16': { ago: 12 * MIN, desc: 'Find and remove the old branches work/1–work/9; they are all merged into main, work/10 stays.' },
  'alpha/20': { ago: 8 * MIN, desc: 'Агент ждёт ответа, но доска не отдала ни строки вопроса: текст бывает только на экране агента.' },
  'alpha/21': { ago: 6 * MIN, desc: 'Агент остановился на диалоге разрешения; доска знает только «Needs you» и вид «вопрос».' },
  'alpha/22': { ago: 5 * MIN, desc: 'Агент остановился на диалоге разрешения; доска не назвала даже вида — только «Needs you».' },
  'beta/1': { ago: 9 * DAY, desc: 'Initial beta bootstrap: CI config and the first smoke test.' },
  'gamma/1': { ago: 27 * HOUR, desc: 'Writing the pricing notes for the gamma service; the color tokens come from alpha/4.', afterDone: [{ id: 'gamma/2', title: 'Архивная задача' }] },
  'gamma/2': { ago: 3 * DAY, desc: 'Archived: old gamma CLI cleanup.' },
  'delta/1': { ago: 6 * HOUR, desc: 'Open a PR for the delta cache rewrite; the checks fail on the lint step. Codex agent.', worker: 'codex/gpt-5-codex', model: 'gpt-5-codex' },
  'delta/2': { ago: 30 * HOUR, desc: 'Выбрать один из трёх вариантов интерфейса импорта.' },
  'delta/3': { ago: 3 * DAY, desc: 'Как кешировать: в памяти, на диске или оба. Три попытки, выбрать одну.' },
  'sigma/1': { ago: 7 * HOUR, desc: 'Перенести конфиг сборки на новый формат; агент остановился без итога.', noSettings: true },
  'sigma/2': { ago: 26 * HOUR, desc: 'Ревьюеру нужен человек: спорный вопрос про лицензию зависимости.', worker: 'omp/deepseek/deepseek-v4-flash', model: 'deepseek-v4-flash', reviewerOn: 'codex/gpt-6-luna' },
  'docs/1': { ago: 9 * DAY, desc: 'Archived documentation pass.' },
};
const WORKER_BY_APP = { claude: 'claude/claude-sonnet-5-5', omp: 'omp/deepseek/deepseek-v4-flash', pi: 'pi/default', codex: 'codex/gpt-5-codex' };
const MODEL_BY_APP = { claude: 'claude-sonnet-5-5', omp: 'deepseek-v4-flash', pi: null, codex: 'gpt-5-codex' };
const CLOSING = {
  'sigma/2': 'Итог SIG-2: лицензия зависимости совместима, оставляем. Решение: зависимость берём только из реестра, не из форка.\n',
  'delta/1': 'Итог DEL-1: переписали кэш delta. Проверки линта падают на шаге форматирования — это известно и не относится к задаче.\n',
  'alpha/4': [
    'Итог ALP-4: тёмная тема для страницы отчётов готова.',
    '- Цвета берутся из палитры Omarchy, контраст не ниже AA.',
    '- Решили не хранить тему в базе: хватает localStorage и системной настройки.',
    '- Грабли: переключатель темы мигал при загрузке, пока класс не ставился до первой отрисовки.',
  ].join('\n') + '\n',
  'alpha/5': [
    'Итог ALP-5: кэш агрегатов переписан.',
    '- Сброс кэша при смене набора данных добавлен.',
    '- Тест `cache_reset` пока падает: нужен пустой отчёт.',
    '- Ревью просило добавить тест на пустой отчёт — не успел.',
  ].join('\n') + '\n',
  'alpha/15': [
    'Summary of ALP-15: the export button now shows progress.',
    '- Added a progress bar to the export dialog (ui/export.rs).',
    '- The job reports percent through the existing event channel; no new endpoint.',
    '- Cancelling an export removes the partial file.',
    '- Tests: cargo test export — 14 passed; a 2 GB report exported in 41 s by hand.',
    '- PR 15 merged into main after one review round (a naming note, fixed).',
    '- Left for later: resuming an export after a restart (not in scope).',
  ].join('\n') + '\n',
  'alpha/13': [
    'Итог ALP-13: флаг совместимости `legacy_export` удалён из конфига.',
    '- Убраны три ветки в экспорте, которые читали этот флаг.',
    '- Конфиг по умолчанию теперь без него; старые файлы читаются как раньше.',
    '- Тесты зелёные, работа влита в main.',
  ].join('\n') + '\n',
  // More than the old 16 KiB endpoint cap, with a table and nested lists: the window must show all of it.
  'beta/1': [
    'Done: CI config added, the smoke test passes on main. Nothing left open.',
    '',
    '| метод | путь | назначение |',
    '|:---|:---:|---:|',
    '| POST | /api/export | создать выгрузку |',
    '| GET | /api/export/7 | прочитать выгрузку |',
    '',
    '- Верхний пункт',
    '  - Вложенный пункт',
    '    - Ещё глубже',
    '- Второй пункт',
    '',
    ...Array.from({ length: 220 }, (_, i) => `- Строка длинного итога ${i + 1}: ${'подробность '.repeat(6)}`),
    '',
    'КОНЕЦ-ИТОГА',
  ].join('\n') + '\n',
};

// The Coordinator: a Claude transcript of ten turns, the last one unanswered.
const chatDir = path.join(dir, 'board', 'coordinator', 'default', 'chat');
fs.mkdirSync(chatDir, { recursive: true });
const chatFolder = fs.realpathSync(chatDir).replace(/[^A-Za-z0-9]/g, '-');

// ---------------------------------------------------------------- git: origins and clones

const git = (cwd, gitArgs, date) => {
  const env = { ...process.env };
  if (date) env.GIT_AUTHOR_DATE = env.GIT_COMMITTER_DATE = date;
  return execFileSync('git', ['-c', 'commit.gpgsign=false', '-c', 'init.defaultBranch=main', '-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid', ...gitArgs], { cwd, env, stdio: ['ignore', 'pipe', 'pipe'] }).toString().trim();
};
const commit = (cwd, file, text, msg, date) => {
  fs.writeFileSync(path.join(cwd, file), text);
  git(cwd, ['add', '-A']);
  git(cwd, ['commit', '-q', '-m', msg], date);
};
const origins = path.join(dir, 'origins');
const seedRoot = path.join(dir, '.seed');
fs.mkdirSync(seedRoot, { recursive: true });
const D1 = '2026-10-10T10:00:00Z', D2 = '2026-10-10T10:05:00Z', D3 = '2026-10-10T10:10:00Z';

// alpha: the origin has three commits; the clone was made after the first, then fetched:
// its main is two commits behind origin/main, and FETCH_HEAD is there.
const alphaBare = path.join(origins, 'alpha.git');
git(origins, ['init', '-q', '--bare', alphaBare]);
git(alphaBare, ['symbolic-ref', 'HEAD', 'refs/heads/main']);
const seedA = path.join(seedRoot, 'alpha');
fs.mkdirSync(seedA);
git(seedA, ['init', '-q']);
commit(seedA, 'README.md', '# alpha\n', 'Первый коммит', D1);
git(seedA, ['push', '-q', alphaBare, 'HEAD:refs/heads/main']);
const alphaDir = path.join(dir, 'projects', 'AI', 'alpha');
git(dir, ['clone', '-q', alphaBare, alphaDir]);
commit(seedA, 'notes.txt', 'вторая правка\n', 'Вторая правка', D2);
commit(seedA, 'notes.txt', 'третья правка\n', 'Третья правка', D3);
git(seedA, ['push', '-q', alphaBare, 'HEAD:refs/heads/main']);
git(alphaDir, ['fetch', '-q', 'origin']);
const alphaHead = git(alphaDir, ['rev-parse', 'refs/heads/main']);
const alphaMid = git(alphaDir, ['rev-parse', 'refs/remotes/origin/main~1']);
const alphaOriginSha = git(alphaDir, ['rev-parse', 'refs/remotes/origin/main']);
const alphaTip = Number(git(alphaDir, ['log', '-1', '--format=%ct', 'refs/remotes/origin/main']));

// beta: level with its origin.
const betaBare = path.join(origins, 'beta.git');
git(origins, ['init', '-q', '--bare', betaBare]);
git(betaBare, ['symbolic-ref', 'HEAD', 'refs/heads/main']);
const seedB = path.join(seedRoot, 'beta');
fs.mkdirSync(seedB);
git(seedB, ['init', '-q']);
commit(seedB, 'README.md', '# beta\n', 'Первый коммит беты', D1);
git(seedB, ['push', '-q', betaBare, 'HEAD:refs/heads/main']);
git(dir, ['clone', '-q', betaBare, path.join(dir, 'projects', 'AI', 'beta')]);

// newrepo: a folder ready to be added on the page (one commit, a GitHub origin the stand-in gh knows as WRITE).
const newrepo = path.join(dir, 'projects', 'herdr', 'newrepo');
fs.mkdirSync(newrepo, { recursive: true });
git(newrepo, ['init', '-q']);
commit(newrepo, 'main.rs', 'fn main() {}\n', 'Начало newrepo', D1);
git(newrepo, ['remote', 'add', 'origin', 'https://github.com/acme/mine.git']);
fs.rmSync(seedRoot, { recursive: true, force: true });

// The plain folders (no git): gamma, omega, delta, sigma, docs.
for (const [repo, f] of Object.entries(folders)) {
  if (repo === 'alpha' || repo === 'beta') continue;
  fs.mkdirSync(f, { recursive: true });
  write(path.join(f, 'NOTES.txt'), `${repo}: папка без git (фикстура).\n`);
}

// docs: a git folder with no origin (its universe memory is added and committed from the page).
{
  const docsDir = folders.docs;
  git(docsDir, ['init', '-q']);
  git(docsDir, ['config', 'user.name', 'fixture']);
  git(docsDir, ['config', 'user.email', 'fixture@example.invalid']);
  commit(docsDir, 'NOTES.txt', 'docs: папка (фикстура).\n', 'Начало docs', D1);
}

// ---------------------------------------------------------------- the rows' records

const C0 = common({ dir }, { fs, path, crypto });
const W = {};     // id -> the record's fields (meta)
const ompNow = (ms) => iso(ms).replace(/[:.]/g, '-');
const createdOf = (id) => createdAgo(META[id]?.ago ?? 2 * DAY);
for (const r of rows) {
  const [repo, nStr] = r.id.split('/');
  const n = Number(nStr);
  const m = META[r.id] || {};
  const created = createdOf(r.id);
  const worker = m.worker ?? WORKER_BY_APP[r.app];
  W[r.id] = {
    created,
    at: r.state === 'done' || r.archivedAt ? Math.min(NOW, created + 40 * MIN) : created,
    description: m.desc ?? `Задача ${r.id}: ${r.title}.`,
    settings: { ...(m.noSettings ? {} : (worker ? { 'worker.agent': worker } : {})), ...(m.reviewer ? { 'reviewer.agent': m.reviewer } : {}) },
    reviewerOn: m.reviewerOn ?? null,
    model: m.model !== undefined ? m.model : MODEL_BY_APP[r.app] ?? null,
    session: C0.uuid('session ' + r.id),
    worktree: path.join(dir, 'worktrees', `${repo}-${n}`),
    branch: `work/${n}`,
    // noExit: the shape of most done tasks on a real board: no exitedAt, the board only saw the pane gone (last.gone).
    exitedAt: m.noExit ? null : (r.exitedAt ?? (r.state === 'done' || r.archivedAt ? Math.min(NOW, created + 40 * MIN) : null)),
    gone: !!m.noExit,
    afterDone: m.afterDone || [],
  };
}

// The worktrees exist before anything resolves them (Claude names its folders after realpaths).
for (const r of rows) fs.mkdirSync(W[r.id].worktree, { recursive: true });
// A Claude session has a folder even when it has no sub-agents: then the list is empty, with no «нет папки сессии» note.
for (const r of rows.filter((x) => x.app === 'claude')) {
  const folder = fs.realpathSync(W[r.id].worktree).replace(/[^A-Za-z0-9]/g, '-');
  fs.mkdirSync(path.join(dir, 'claude', 'projects', folder, W[r.id].session), { recursive: true });
}

// The board's waits: one inside a universe, one between universes (kept as before).
const after = { 'alpha/10': ['ALP-9'], 'gamma/1': ['ALP-4'] };

// ---------------------------------------------------------------- sub-agent files

const wt = (repo, n) => path.join(dir, 'worktrees', `${repo}-${n}`);
const userRec = (at, text) => ({ type: 'user', timestamp: iso(at), message: { role: 'user', content: text } });
const toolUse = (at, id, name, input) => ({ type: 'assistant', timestamp: iso(at), message: { role: 'assistant', content: [{ type: 'tool_use', id, name, input }], stop_reason: 'tool_use' } });
const toolRes = (at, id, text) => ({ type: 'user', timestamp: iso(at), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: text }] } });
const textRec = (at, text, stop) => ({ type: 'assistant', timestamp: iso(at), message: { role: 'assistant', content: [{ type: 'text', text }], stop_reason: stop ?? null } });

// Claude: <claude>/projects/<dashed realpath of the worktree>/<session>/subagents/agent-<id>.jsonl (+ .meta.json).
function claudeSub(cwd, session, id, { records, meta, mtime }) {
  const folder = fs.realpathSync(cwd).replace(/[^A-Za-z0-9]/g, '-');
  const sub = path.join(dir, 'claude', 'projects', folder, session, 'subagents');
  fs.mkdirSync(sub, { recursive: true });
  const file = path.join(sub, `agent-${id}.jsonl`);
  fs.writeFileSync(file, jl(records));
  fs.writeFileSync(path.join(sub, `agent-${id}.meta.json`), JSON.stringify(meta));
  if (mtime) fs.utimesSync(file, mtime / 1000, mtime / 1000);
}
const ALP2 = W['alpha/2'].session;
// alpha/2 (Waiting, claude): one running, one quiet (15 min), one done.
claudeSub(wt('alpha', 2), ALP2, 'a1f0c3e9d7b24e10', {
  records: [
    userRec(NOW - 14 * MIN, 'Найди все места, где бэкенд отдаёт отчёты: хендлеры, сериализация, кеш.'),
    toolUse(NOW - 13 * MIN, 'toolu_01', 'Grep', { pattern: 'export', path: 'src' }),
    toolRes(NOW - 13 * MIN + 4000, 'toolu_01', 'src/api/export.rs:14: pub async fn export(...)'),
    toolUse(NOW - 90 * 1000, 'toolu_02', 'Read', { file_path: 'src/api/export.rs' }),
  ],
  meta: { agentType: 'Explore', description: 'Explore Rust backend', finished: false },
  mtime: NOW - 30 * 1000,
});
claudeSub(wt('alpha', 2), ALP2, 'b2e8d5a4c9f31b07', {
  records: [
    userRec(NOW - 70 * MIN, 'Проверь, какие клиенты читают старую схему.'),
    textRec(NOW - 16 * MIN, 'Жду ответа по клиенту mobile-2: он в отпуске до понедельника.', null),
  ],
  meta: { agentType: 'general-purpose', description: 'Проверить клиентов старой схемы', finished: false },
  mtime: NOW - 15 * MIN,
});
claudeSub(wt('alpha', 2), ALP2, 'c3d9e6b5daf42c18', {
  records: [
    userRec(NOW - 40 * MIN, 'Сравни старую и новую схему по полям.'),
    toolUse(NOW - 39 * MIN, 'toolu_11', 'Read', { file_path: 'docs/schema-v1.md' }),
    toolRes(NOW - 39 * MIN + 3000, 'toolu_11', '# schema v1: owner_id, period, amount, …'),
    textRec(NOW - 38 * MIN, 'Старая схема: 14 полей, 2 таблицы.\nНовая: 11 полей, одна таблица.\nДля клиентов v1 нужны owner_id и period, в новой их нет.\nРекомендация: оставить старую схему для v1, новую отдавать с /v2.', 'end_turn'),
  ],
  meta: { agentType: 'Explore', description: 'Сравнить схемы', finished: true },
  mtime: NOW - 5 * MIN,
});
// alpha/16 (claude): one running sub-agent.
claudeSub(wt('alpha', 16), W['alpha/16'].session, 'd4aa7f6c0b53d229', {
  records: [
    userRec(NOW - 11 * MIN, 'Найди ветки work/1–work/9 и проверь, что они влиты в main.'),
    toolUse(NOW - 10 * MIN, 'toolu_21', 'Bash', { command: 'git branch --merged main' }),
    toolRes(NOW - 10 * MIN + 2000, 'toolu_21', '  work/1\n  work/2\n  work/3'),
    toolUse(NOW - MIN, 'toolu_22', 'Bash', { command: 'git log -1 --format=%ci work/9' }),
  ],
  meta: { agentType: 'general-purpose', description: 'Найти старые ветки', finished: false },
  mtime: NOW - 20 * 1000,
});

// omp: the parent session file (task.json `session`), its children `<parent without .jsonl>/<Name>.jsonl` (+ `.md`).
function ompParent(file, cwd, ms, id, text, toolPath) {
  write(file, jl([
    { type: 'session', version: 3, id, timestamp: iso(ms), cwd: fs.realpathSync(cwd) },
    { type: 'message', timestamp: iso(ms + 1000), message: { role: 'user', content: [{ type: 'text', text }] } },
    { type: 'message', timestamp: iso(ms + 2000), message: { role: 'assistant', content: [{ type: 'toolCall', id: 'call_1', name: 'read', arguments: { path: toolPath } }] } },
  ]));
}
const ompFile = (cwd, ms, id) => path.join(dir, 'omp', 'agent', 'sessions', fs.realpathSync(cwd).replace(/[^A-Za-z0-9]/g, '-'), `${ompNow(ms)}_${id}.jsonl`);
function ompChild(parent, name, { records, result, mtime }) {
  const folder = parent.replace(/\.jsonl$/, '');
  fs.mkdirSync(folder, { recursive: true });
  const file = path.join(folder, `${name}.jsonl`);
  fs.writeFileSync(file, jl(records));
  if (result) fs.writeFileSync(path.join(folder, `${name}.md`), result);
  if (mtime) fs.utimesSync(file, mtime / 1000, mtime / 1000);
}
const omp6Id = C0.uuid('omp alpha/6');
const omp6 = ompFile(wt('alpha', 6), createdOf('alpha/6'), omp6Id);
ompParent(omp6, wt('alpha', 6), createdOf('alpha/6'), omp6Id, 'Перенеси таблицы отчётов в новую схему, индексы не потеряй.', 'db/schema.sql');
const t0 = NOW - 20 * MIN;
const parentPath = omp6;
ompChild(parentPath, 'Migration-Scan', {
  records: [
    { type: 'session', version: 3, id: C0.uuid('scan'), timestamp: iso(t0), cwd: fs.realpathSync(wt('alpha', 6)), parentSession: parentPath },
    { type: 'message', timestamp: iso(t0 + 1000), message: { role: 'user', content: [{ type: 'text', text: 'Найди все таблицы с датами в старом формате.' }] } },
    { type: 'message', timestamp: iso(t0 + 2000), message: { role: 'assistant', content: [{ type: 'toolCall', id: 'call_2', name: 'read', arguments: { path: 'db/schema.sql' } }] } },
    { type: 'message', timestamp: iso(t0 + 3000), message: { role: 'assistant', content: [{ type: 'text', text: 'Нашёл 4 таблицы: reports, report_days, exports, export_jobs. Даты хранятся в трёх форматах.' }] } },
    { type: 'custom', timestamp: iso(t0 + 4000), customType: 'session_exit', data: { code: 0 } },
  ],
  result: '# Итог\n\nТаблицы: reports, report_days, exports, export_jobs.\nФорматы дат: ISO, unix, DD.MM.YYYY.\n',
  mtime: t0 + 4000,
});
ompChild(parentPath, 'Migration-Index', {
  records: [
    { type: 'session', version: 3, id: C0.uuid('index'), timestamp: iso(NOW - 2 * MIN), cwd: fs.realpathSync(wt('alpha', 6)), parentSession: parentPath },
    { type: 'message', timestamp: iso(NOW - 2 * MIN + 1000), message: { role: 'user', content: [{ type: 'text', text: 'Построй план индексов для export_jobs.' }] } },
    { type: 'message', timestamp: iso(NOW - 90 * 1000), message: { role: 'assistant', content: [{ type: 'toolCall', id: 'call_3', name: 'grep', arguments: { pattern: 'export_jobs', path: 'db' } }] } },
  ],
  mtime: NOW - 30 * 1000,
});
// sigma/2 (omp): a parent with no children yet.
const omp11Id = C0.uuid('omp sigma/2');
const omp11 = ompFile(wt('sigma', 2), createdOf('sigma/2'), omp11Id);
ompParent(omp11, wt('sigma', 2), createdOf('sigma/2'), omp11Id, 'Проверь лицензию зависимости и скажи, можно ли её брать.', 'LICENSE');

// codex: delta/1 has a parent rollout and one child (its parent_thread_id names the parent).
const CX_PARENT = C0.uuid('codex delta/1');
const CX_CHILD = C0.uuid('codex child delta/1');
const cxDir = path.join(dir, 'codex', 'sessions', '2026', '10', '20');
fs.mkdirSync(cxDir, { recursive: true });
const cxParent = path.join(cxDir, `rollout-2026-10-20T06-00-00-${CX_PARENT}.jsonl`);
write(cxParent, jl([
  { timestamp: iso(createdOf('delta/1')), type: 'session_meta', payload: { id: CX_PARENT, timestamp: iso(createdOf('delta/1')), cwd: wt('delta', 1), originator: 'codex_cli' } },
  { timestamp: iso(createdOf('delta/1') + 1000), type: 'event_msg', payload: { type: 'task_started' } },
  { timestamp: iso(createdOf('delta/1') + 2000), type: 'event_msg', payload: { type: 'user_message', message: 'Открой PR с новым кешем; проверки падают на lint.' } },
  { timestamp: iso(createdOf('delta/1') + 3000), type: 'event_msg', payload: { type: 'SubAgentActivity', kind: 'started', agent_thread_id: CX_CHILD } },
]));
write(path.join(cxDir, `rollout-2026-10-20T09-40-00-${CX_CHILD}.jsonl`), jl([
  { timestamp: iso(NOW - 4 * MIN), type: 'session_meta', payload: { id: CX_CHILD, parent_thread_id: CX_PARENT, agent_role: 'explorer', timestamp: iso(NOW - 4 * MIN) } },
  { timestamp: iso(NOW - 4 * MIN + 1000), type: 'event_msg', payload: { type: 'task_started' } },
  { timestamp: iso(NOW - 3 * MIN), type: 'response_item', payload: { type: 'function_call', name: 'shell', arguments: JSON.stringify({ command: ['rg', '-n', 'lint', '.github'] }) } },
]));

// The session ids of the rows that have files: omp and codex keep the file's path.
W['alpha/6'].session = omp6;
W['sigma/2'].session = omp11;
W['delta/1'].session = cxParent;

// ---------------------------------------------------------------- the Coordinator

// Transcript: user turns and tool results, assistant turns with model and usage. The last
// assistant turn's usage is 150000 tokens, 75 % of a 200k window.
const usage = (i) => (i === 9
  ? { input_tokens: 3000, cache_read_input_tokens: 140000, cache_creation_input_tokens: 7000 }
  : { input_tokens: 2000, cache_read_input_tokens: 30000 + i * 1000, cache_creation_input_tokens: 500 });
const cuser = (i, text) => JSON.stringify({ type: 'user', timestamp: new Date(1792389360000 + i * 60000).toISOString(), message: { role: 'user', content: text } });
const csaid = (i, text) => JSON.stringify({ type: 'assistant', timestamp: new Date(1792389361000 + i * 60000).toISOString(), message: { role: 'assistant', model: 'claude-opus-5-5', content: [{ type: 'text', text }], usage: usage(i) } });
const ctool = (i) => JSON.stringify({ type: 'user', timestamp: new Date(1792389360500 + i * 60000).toISOString(), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 't', content: 'ok' }] } });
// A tool call of the running turn: the page shows its description as what the Coordinator is doing.
const ccall = (i, description, command) => JSON.stringify({ type: 'assistant', timestamp: new Date(1792389360300 + i * 60000).toISOString(), message: { role: 'assistant', model: 'claude-opus-5-5', stop_reason: 'tool_use', content: [{ type: 'tool_use', id: `tu${i}`, name: 'Bash', input: { command, description } }] } });
// A message sent with a picture, the way Claude Code writes it (2.1.3xx): a text block that ends at the planner's header
// (the path lines are dropped) and an image block with the bytes inside; the planner serves it from the transcript.
const PNG_1PX = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==';
const cimage = (i, text) => JSON.stringify({ type: 'user', timestamp: new Date(1792389360000 + i * 60000).toISOString(), message: { role: 'user', content: [{ type: 'text', text: `${text}\nКартинки (файлы на машине с доской):` }, { type: 'image', source: { type: 'base64', media_type: 'image/png', data: PNG_1PX } }] } });
// The turns the page checks look at: 6 has an image, 7 is a board notice (its title quotes something of its own), 8 is Markdown,
// 9 ends with a board event, 10 is running.
const QUESTIONS = {
  7: '[Уведомление доски] alpha/7 «Документация «API экспорта»: примеры»: ждёт тебя (needs you). Посмотри board details alpha/7 и коротко доложи итог.',
};
const ANSWERS = {
  8: 'Ответ 8: **ALP-8** идёт по плану.\n\n## Что сделано\n\n- проверил `cargo test`\n  - сборка прошла\n- открыл PR\n\n```sh\nherdr-projects list --json\n```\n\n| Шаг | Статус |\n|---|---|\n| сборка | ok |\n| ревью | ждёт |\n\nПодробнее: https://example.com/alp-8',
  9: 'Ответ 9: ALP-9 идёт по плану.\n\nStarted an agent on ALP-9',
};
const transcript = [];
for (let i = 1; i <= 10; i++) {
  transcript.push(i === 6 ? cimage(i, 'Вопрос 6: как дела по ALP-6? Вот снимок.') : cuser(i, QUESTIONS[i] ?? `Вопрос ${i}: как дела по ALP-${i}?`));
  if (i === 10) transcript.push(ccall(i, 'Читаю board details ALP-10', 'herdr-projects details alpha/10'));
  transcript.push(ctool(i));
  if (i < 10) transcript.push(csaid(i, ANSWERS[i] ?? `Ответ ${i}: ALP-${i} идёт по плану.`));
}
// The last assistant turn is the one with the big usage (i = 9 above).
write(path.join(dir, 'claude', 'projects', chatFolder, 'fixture.jsonl'), transcript.join('\n') + '\n');
write(path.join(dir, 'board', 'coordinator', 'default', 'state.json'), JSON.stringify({ run: { kind: 'claude' }, session: 'fixture' }));
// What the Coordinator's program knows as commands (the `/` list): a skill of the person's own folder,
// a command there, and a command of the Coordinator's own folder. Claude Code's built-ins come with the planner.
write(path.join(dir, 'claude', 'skills', 'fixture-skill', 'SKILL.md'), '---\nname: fixture-skill\ndescription: Навык фикстуры: проверяет подсказки ввода\n---\n\n# Fixture skill\n');
write(path.join(dir, 'claude', 'commands', 'fixture-cmd.md'), '---\ndescription: Команда фикстуры\n---\n\nDo the fixture thing.\n');
write(path.join(chatDir, '.claude', 'commands', 'here-cmd.md'), '---\ndescription: Команда папки Координатора\n---\n\nHere.\n');
// A skill that is a link, as most of `~/.claude/skills` are on this machine: it must be offered too.
write(path.join(dir, 'agents', 'ui-ux-pro-max', 'SKILL.md'), '---\nname: ui-ux-pro-max\ndescription: Навык-ссылка фикстуры\n---\n');
fs.mkdirSync(path.join(dir, 'claude', 'skills'), { recursive: true });
fs.symlinkSync(path.join(dir, 'agents', 'ui-ux-pro-max'), path.join(dir, 'claude', 'skills', 'ui-ux-pro-max'));
// A name longer than the list is wide: it must wrap, not run past the edge and hide its words.
write(
  path.join(dir, 'claude', 'skills', 'superpowers-executing-plans-with-subagent-driven-development-review', 'SKILL.md'),
  '---\nname: superpowers-executing-plans-with-subagent-driven-development-review\ndescription: Навык с длинным именем\n---\n',
);

// Memory (Markdown files; the planner indexes them). The Galaxy's own: two entries and no MEMORY.md (the planner makes it).
const memEntry = (id, title, desc, type, body, extra = '') => `---\nname: ${id}\ntitle: ${title}\ndescription: ${desc}\ntype: ${type}\nadded: 2026-10-19\nsource: chat\n${extra}---\n\n${body}\n`;
const memGalaxy = path.join(dir, 'memory', 'galaxy');
write(path.join(memGalaxy, 'gitlab-bez-pr.md'), memEntry('gitlab-bez-pr', 'GitLab — без PR', 'В репо на GitLab слияние идёт без pull request', 'rule', 'Доска открывает PR только на GitHub. Для репозиториев GitLab ставим `merge.pr false` и `merge.checks none`.'));
write(path.join(memGalaxy, 'deshevaya-model.md'), memEntry('deshevaya-model', 'Дешёвая модель', '«Дешёвая модель» — omp/deepseek/deepseek-v4-flash', 'preference', 'Когда человек просит дешёвую модель, ставим задаче `--worker.agent omp/deepseek/deepseek-v4-flash`; ревьюера не меняем.'));
// The board's instruction files of the Coordinator (the planner adds its block after the board's region).
// The machine Coordinator and a project Coordinator have their own instruction files; the human's notes sit above the board's marker.
const boardRules = "<!-- herdr-projects: the Coordinator's standing rules. The board writes what is between this line and the closing one, at every start: your own notes go outside them. -->\n";
write(path.join(dir, 'board', 'coordinator', 'default', 'machine', 'chat', 'AGENTS.md'), boardRules + "You are the machine Coordinator.\n<!-- /herdr-projects -->\n");
write(path.join(dir, 'board', 'coordinator', 'default', 'projects', 'alpha', 'rules.md'), 'Правила проектного Координатора alpha.\n');
write(path.join(chatDir, 'AGENTS.md'), "Мои заметки выше маркера доски.\n\n<!-- herdr-projects: the Coordinator's standing rules. The board writes what is between this line and the closing one, at every start: your own notes go outside them. -->\nYou are the Coordinator of the human's work board in herdr (herdr-projects).\n<!-- /herdr-projects -->\n");
write(path.join(chatDir, 'CLAUDE.md'), "<!-- herdr-projects: the Coordinator's standing rules. The board writes what is between this line and the closing one, at every start: your own notes go outside them. -->\n@AGENTS.md\n<!-- /herdr-projects -->\n");
// Claude Code's own memory of that project folder (read only for the planner).
write(path.join(dir, 'claude', 'projects', chatFolder, 'memory', 'user-answers-short.md'), '---\nname: user-answers-short\ndescription: Человек просит отвечать коротко, с результата\nmetadata:\n  type: feedback\n---\n\nОтвечать коротко и начинать с результата. Подробности — только если спросят.\n');
write(path.join(dir, 'claude', 'projects', chatFolder, 'memory', 'MEMORY.md'), '- [user-answers-short](user-answers-short.md) — коротко\n');
// A universe's memory: alpha has one entry that is not in git; docs is a git folder without memory yet.
write(path.join(alphaDir, 'docs', 'memory', 'merge-squash.md'), memEntry('merge-squash', 'Слияние — squash', 'В alpha задачи сливаются squash, история ветки не нужна', 'decision', 'Решили сливать squash: ветки задач короткие, а лента main остаётся читаемой.'));
// The Coordinator's own sub-agent: one, under the transcript's folder. Its state follows the
// Coordinator: `done` while it is idle; `state/coordinator-busy` makes the Coordinator «working».
claudeSub(chatDir, 'fixture', 'f6cc9b8e2d75f44b', {
  records: [
    userRec(NOW - 6 * MIN, 'Проверь очередь ALP-9 и ALP-10 и скажи, что мешает.'),
    toolUse(NOW - 5 * MIN, 'toolu_31', 'Bash', { command: 'herdr-projects list --json' }),
    toolRes(NOW - 5 * MIN + 1000, 'toolu_31', '[…]'),
  ],
  meta: { agentType: 'general-purpose', description: 'Проверить очередь ALP', finished: false },
  mtime: NOW - 20 * 1000,
});
fs.writeFileSync(path.join(dir, 'state', 'coordinator.json'), JSON.stringify({
  status: 'idle · claude · claude-opus-5-5 · medium',
  reply: 'Жду задач. Доска спокойна.\n\nЧто делаем?\n1) Показать план на день\n2) Разобрать «Ждёт тебя»',
}));
fs.writeFileSync(path.join(dir, 'state', 'herdr-planner', 'todo.json'), JSON.stringify([
  { id: 't1', text: 'Починить кэш отчётов', at: 1792407710000 },
  { id: 't2', text: 'Добавить тёмную тему в настройки', at: 1792407710001 },
]));

// ---------------------------------------------------------------- the big world (--scale)

const GROUPS = ['AI', 'herdr', 'work', 'lab'];
const PLANET = [
  { state: 'working', stage: 'working', msg: 'Writing', progress: 35 },
  { state: 'working', stage: 'queued', msg: 'Waiting for a free slot', queue: 2 },
  { state: 'blocked', kind: 'question', stage: 'working', msg: 'Which format: CSV or JSON?' },
  { state: 'blocked', kind: 'permission', stage: 'approval', msg: 'Pass · 1 minor', review: passed, reviewRound: 1 },
  { state: 'blocked', stage: 'plan', msg: 'Plan ready' },
  { state: 'error', stage: 'working', msg: 'The build failed' },
  { state: 'idle', stage: 'todo', msg: '' },
  { state: 'done', stage: 'done', msg: 'Merged into main.' },
  { state: 'working', stage: 'reviewing', msg: 'Under review', progress: 60, reviewRound: 1 },
  { state: 'blocked', stage: 'pick', msg: 'Pick one of 3' },
];

// ---------------------------------------------------------------- the stand-ins

const models = {
  claude: ['claude/claude-sonnet-5-5', 'claude/claude-opus-5-5', 'claude/claude-haiku-5-5'],
  omp: ['omp/deepseek/deepseek-v4-flash', 'omp/deepseek/deepseek-v4-pro', 'omp/openai/gpt-5'],
  codex: ['codex/gpt-5-codex', 'codex/o4-mini'],
  pi: ['pi/default', 'pi/qwen-coder-3'],
};
const modelNote = 'Имена моделей программа отдаёт сама: доска спрашивает её, что принимает. Здесь — то, что видно на этой машине; аккаунт может открыть не все.';

const ORIGIN_URL = alphaBare;
// The board's own settings for the stand-ins: the effort levels, the built-in values, what the board starts with and
// what its doctor says. The PATH gets codex and omp stand-ins below.
const BOARD = {
  efforts: {
    claude: ['low', 'medium', 'high', 'xhigh', 'max'],
    codex: ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'],
    omp: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'auto'],
  },
  builtin: {
    'worker.fast': false, 'worker.access': 'full', 'worker.fallback': 'none', 'worker.instructions': 'none', 'coordinator.instructions': 'none',
    'reviewer.fast': true, 'reviewer.access': 'full', 'reviewer.fallback': 'none',
    'coordinator.fast': false, 'research.only': false, 'workspace.from': 'main', 'attempts.by': 1, 'attempts.pick': 'you',
    'review.when': 'repos', 'review.mustFix': 'major', 'review.rounds': 3, 'merge.by': 'you', 'merge.method': 'squash',
    'limit.time': 'none', 'limit.tokens': 'none', 'board.machines': 'auto', 'board.theme': 'herdr',
  },
  seed: {
    home: {
      'coordinator.agent': 'claude/claude-opus-5-5', 'coordinator.effort': 'medium', 'plan.agent': 'claude/claude-opus-5-5',
      'reviewer.agent': 'claude/claude-opus-5-5', 'reviewer.effort': 'high', 'worker.agent': 'claude/claude-sonnet-5-5',
      'worker.effort': 'high', 'watcher.agent': 'claude/claude-haiku-5-5', 'board.agents': 6,
      'coordinator.instructions': 'Мои правила, поверх общих:\n1. Настройки меняй только с моего явного согласия.\n2. Если непонятно, к какой задаче это относится, — спроси.',
    },
    alpha: { 'worker.effort': 'max' },
  },
  doctor: {
    home: [
      '✓ herdr-projects 1.2.4, downloaded: the fixture plugin',
      '✓ claude 2.1.303, signed in (worker.agent, reviewer.agent, coordinator.agent and 5 more)',
      '✗ codex 0.9.2, not signed in: codex login',
      '✓ omp 1.5.0, API keys: no sign-in needed',
      '1 problem.',
    ],
  },
  doctorExit: { home: 1 },
};

const utc = (ms) => {
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())} +0000 UTC`;
};

const DATA = {
  dir,
  board: BOARD,
  gh: { 'acme/mine': 'WRITE', 'acme/theirs': 'READ' },
  models,
  modelNote,
  // Machines the board knows (`herdr-projects machines`, `herdr machine list`): the planner works with its own board only and
  // must not show them, nor fail on them.
  machines: {
    here: { label: 'this', host: 'fixture', version: '1.2.4' },
    machines: [
      { label: 'vm-a', target: 'deploy@vm-a', ok: true, version: '1.2.4', tasks: 4 },
      { label: 'vm-b', target: 'deploy@vm-b', ok: false, why: { kind: 'herdr-projects', line: 'herdr-projects is not found there', detail: [], said: '', code: 127 } },
    ],
    off: false,
  },
  savedMachines: [
    { label: 'vm-a', ssh_target: 'deploy@vm-a', remote_session: 'work' },
    { label: 'vm-b', ssh_target: 'deploy@vm-b' },
  ],
  // The shipyard: a documentation address (192.0.2.0/24 is never a real host); the ssh stand-in answers for it.
  runnerR: {
    label: 'vm-r',
    target: 'me@192.0.2.29',
    host: '192.0.2.29',
    ports: '192.0.2.29',
    containers: [
      { id: 'a'.repeat(64), name: 'pg-ALP-3', image: 'postgres:16', state: 'running', status: 'Up 1 hour', created: utc(NOW - 60 * 60 * 1000), labels: 'herdr.task=ALP-3,com.docker.compose.project=work-3', ports: '0.0.0.0:15432->5432/tcp', networks: 'work-3_default', mounts: 'work-3_pgdata:/var/lib/postgresql/data', size: '28MB', cpu: '0.40%', mem: '96MiB / 1GiB', pids: '7', nano: 2000000000, memLimit: 1073741824 },
      { id: '4'.repeat(64), name: 'pg-AL9-4', image: 'postgres:16', state: 'running', status: 'Up 3 hours', created: utc(NOW - 3 * 60 * 60 * 1000), labels: 'herdr.task=AL9-4,com.docker.compose.project=work-4', ports: '0.0.0.0:15433->5432/tcp', networks: 'work-4_default', mounts: '', size: '31MB', cpu: '0.10%', mem: '72MiB / 1GiB', pids: '6' },
      { id: 'b'.repeat(64), name: 'web-ALP-15', image: 'nginx:1.27', state: 'exited', status: 'Exited (0) 2 hours ago', created: utc(NOW - 2 * 24 * 60 * 60 * 1000), labels: 'herdr.task=ALP-15', ports: '', networks: 'bridge', mounts: '', size: '12MB', cpu: '0.00%', mem: '0B / 0B', pids: '0', nano: 1000000000, memLimit: 536870912 },
      { id: 'c'.repeat(64), name: 'x-ALP-99', image: 'alpine:3', state: 'exited', status: 'Exited (0) 5 days ago', created: utc(NOW - 5 * 24 * 60 * 60 * 1000), labels: 'herdr.task=ALP-99', ports: '', networks: 'bridge', mounts: '', size: '4MB', cpu: '0.00%', mem: '0B / 0B', pids: '0' },
      { id: 'd'.repeat(64), name: 'neighbor-db-1', image: 'postgres:15', state: 'running', status: 'Up 3 days', created: utc(NOW - 3 * 24 * 60 * 60 * 1000), labels: 'com.docker.compose.project=neighbor', ports: '5432/tcp', networks: 'neighbor_default', mounts: 'neighbor_data:/var/lib/postgresql/data', size: '1.2GB', cpu: '1.20%', mem: '210MiB / 512MiB', pids: '9', nano: 2000000000, memLimit: 536870912 },
      { id: 'e'.repeat(64), name: 'buildx_buildkit_x0', image: 'moby/buildkit:latest', state: 'running', status: 'Up 2 months', created: utc(NOW - 60 * 24 * 60 * 60 * 1000), labels: '', ports: '', networks: 'host', mounts: '', size: '0B', cpu: '0.10%', mem: '34MiB / 0B', pids: '4' },
      // The board's build cache (scripts/yard.sh): a container of no task, an image and a volume, all with
      // herdr.shared=alpha. The planner shows them apart from the tasks and from other people's containers.
      { id: '6'.repeat(64), name: 'yard-cache-1', image: 'herdr-planner-yard:0123456789ab', state: 'running', status: 'Up 1 hour', created: utc(NOW - 60 * 60 * 1000), labels: 'herdr.shared=alpha', ports: '', networks: 'bridge', mounts: 'hp-yard-cache:/cache', size: '0B', cpu: '0.00%', mem: '12MiB / 12GiB', pids: '1', nano: 8000000000, memLimit: 12884901888 },
    ],
    networks: [
      { id: 'f'.repeat(64), name: 'work-3_default', driver: 'bridge' },
      { id: '1'.repeat(64), name: 'neighbor_default', driver: 'bridge' },
      { id: '2'.repeat(64), name: 'bridge', driver: 'bridge' },
    ],
    volumes: [
      { name: 'work-3_pgdata', driver: 'local', size: '28MB', labels: 'com.docker.compose.project=work-3' },
      { name: 'neighbor_data', driver: 'local', size: '1.1GB', labels: 'com.docker.compose.project=neighbor' },
      // Unused volumes the disk card counts: a stray one of somebody else (unticked), the state of a stopped builder
      // (its buildx rm takes it with it), a builder state nobody owns any more, and the state of the running builder.
      { name: 'chroma_data', driver: 'local', size: '3GB', labels: 'com.docker.compose.project=chroma', links: 0 },
      { name: 'buildx_buildkit_old0_state', driver: 'local', size: '3.1GB', labels: '', links: 0 },
      { name: 'buildx_buildkit_gone0_state', driver: 'local', size: '8GB', labels: '', links: 0 },
      { name: 'buildx_buildkit_x0_state', driver: 'local', size: '1GB', labels: '', links: 1 },
      // The build cache volume of the board itself: herdr.shared=alpha, never offered anywhere for removal.
      { name: 'hp-yard-cache', driver: 'local', size: '1.2GB', labels: 'herdr.shared=alpha', links: 1 },
    ],
    // The disk card: `docker system df` totals, the images with their container counts, the buildx builders.
    images: [
      { repo: 'postgres', tag: '16', containers: 2, size: '294MB' },
      { repo: 'nginx', tag: '1.27', containers: 1, size: '190MB' },
      { repo: 'old/app', tag: '1', containers: 0, size: '1.2GB' },
      { repo: 'old/tool', tag: '2', containers: 0, size: '800MB' },
    ],
    // The images of the board's build caches: `docker image ls --filter label=herdr.shared`, then the labels.
    sharedImages: [
      { id: '9'.repeat(64), tags: ['herdr-planner-yard:0123456789ab'], size: 2500000000, labels: 'herdr.shared=alpha' },
    ],
    imagesReclaimable: '2.0GB (60%)',
    cacheReclaimable: '2.4GB',
    builders: [
      { name: 'old', driver: 'docker-container', node: 'old0', status: 'inactive' },
      { name: 'x', driver: 'docker-container', node: 'x0', status: 'running' },
    ],
  },
};

function herdrProjectsMain(C, DATA) {
  const { fs, path, log, D, world, save, coord, saveCoord, tag, find, resolve, said, plain, sleepMs, nextN, publishRepos, publishTask, registerFolder } = C;
  const argv = process.argv.slice(2);
  const [cmd, ...rest] = argv;
  const join = (...p) => path.join(D, ...p);
  const fail = (msg, code = 1) => { console.error(msg); process.exit(code); };
  const flag = (name) => { const i = rest.indexOf(name); return i >= 0 ? rest[i + 1] : undefined; };
  if (cmd !== 'list' && cmd !== 'details' && cmd !== 'after') log('calls.jsonl', { argv });
  if (cmd === 'list') {
    if (fs.existsSync(join('state', 'list-fails'))) fail('herdr-projects: the board is locked');
    const w = world();
    const rows = argv.includes('--all') ? w.rows : w.rows.filter((r) => !r.archivedAt);
    log('lists.jsonl', { argv });
    if (argv.includes('--json')) console.log(JSON.stringify(rows));
    else console.log(rows.map((r) => plain(w, r)).join('\n'));
    return;
  }

  if (cmd === 'machines') {
    log('machines.jsonl', { argv });
    if (argv.includes('--json')) console.log(JSON.stringify(DATA.machines));
    else console.log(DATA.machines.machines.map((m) => m.label + '  ' + m.target + '  ' + (m.ok ? 'ok' : 'не отвечает: ' + m.why.line)).join('\n'));
    return;
  }

  if (cmd === 'models') {
    const prog = rest[0];
    log('models.jsonl', { argv });
    const names = DATA.models[prog];
    if (!names) fail('models: unknown program ' + prog);
    sleepMs(2000);
    console.log(DATA.modelNote + '\n');
    for (const name of names) console.log(name);
    if (DATA.board.efforts[prog]) console.log('\nEffort (worker.effort and the like): ' + DATA.board.efforts[prog].join(', ') + ' (from `' + prog + ' --help`).');
    return;
  }

  if (cmd === 'coordinator' && rest[0] === 'tell') {
    let words = rest.slice(1);
    if (words[0] === '--') words = words.slice(1);
    const text = words.join(' ');
    const c = coord();
    c.reply = 'Принял: «' + text + '». Это фикстура: ответ записан.';
    saveCoord(c);
    log('coordinator.jsonl', { text, argv });
    console.log('sent');
    return;
  }
  if (cmd === 'coordinator' && rest[0] === 'new') {
    log('commands.jsonl', { argv });
    console.log('ok: a new Coordinator is starting');
    return;
  }
  if (cmd === 'coordinator') {
    const c = coord();
    if (fs.existsSync(join('state', 'coordinator-dialog'))) {
      const dialog = fs.existsSync(join('state', 'dialog.txt')) ? fs.readFileSync(join('state', 'dialog.txt'), 'utf8') : 'May I continue?\n❯ 1. Yes\n  2. No';
      console.log('Coordinator: blocked · claude · claude-opus-5-5 · medium\n\n' + dialog);
    } else if (fs.existsSync(join('state', 'coordinator-busy'))) {
      console.log('Coordinator: working · claude · claude-opus-5-5 · medium\n\n' + c.reply);
    } else {
      console.log('Coordinator: ' + c.status + '\n\n' + c.reply);
    }
    return;
  }

  if (cmd === 'add' || cmd === 'new') {
    const VALUE = ['--repo', '--desc', '--description', '--app', '--model', '--worker.agent'];
    const words = [];
    const opts = {};
    for (let i = 0; i < rest.length; i++) {
      if (VALUE.includes(rest[i])) { opts[rest[i]] = rest[i + 1]; i++; }
      else if (!rest[i].startsWith('--')) words.push(rest[i]);
    }
    const title = words.join(' ');
    if (!title) fail('add needs a title');
    const w = world();
    let repo = opts['--repo'] || '';
    if (repo.startsWith('/')) {
      try { repo = registerFolder(w, repo); } catch (e) { fail(e.message); }
    }
    if (!repo || !w.keys[repo]) fail('No repo ' + repo);
    log('commands.jsonl', { argv });
    const n = nextN(w, repo);
    const id = repo + '/' + n;
    const row = { id, app: opts['--app'] || 'claude', state: 'idle', stage: 'todo', title, msg: '' };
    w.rows.push(row);
    const worktree = path.join(D, 'worktrees', repo + '-' + n);
    w.meta[id] = {
      created: Date.now(), description: opts['--desc'] || opts['--description'] || '', model: opts['--model'] || null,
      settings: opts['--worker.agent'] ? { 'worker.agent': opts['--worker.agent'] } : {}, session: C.uuid(id), worktree, branch: 'work/' + n, afterDone: [],
    };
    fs.mkdirSync(worktree, { recursive: true });
    save(w);
    publishTask(w, row);
    console.log(id + ': added');
    return;
  }

  if (cmd === 'doctor') {
    console.log(DATA.board.doctor.home.join('\n'));
    process.exit(DATA.board.doctorExit.home);
  }

  if (cmd === 'show' && argv.includes('--json')) {
    const repo = flag('--repo') || '';
    const task = rest.find((x) => /^[^/]+\/\d+$/.test(x)) || '';
    const w = world();
    if (repo && !w.keys[repo]) fail('No repo ' + repo);
    const ops = C.boardSettings(join('state', 'board-settings.json'), { models: DATA.models, efforts: DATA.board.efforts, builtin: DATA.board.builtin, keys: w.keys });
    console.log(JSON.stringify(ops.show(repo, task), null, 1));
    return;
  }

  if (cmd === 'set' || cmd === 'unset') {
    const repo = flag('--repo') || '';
    const task = flag('--task') || '';
    const w = world();
    if (repo && !w.keys[repo]) fail('No repo ' + repo);
    const name = rest[0];
    const value = cmd === 'set' ? rest[1] : undefined;
    if (!name || (cmd === 'set' && value === undefined)) fail(cmd + ' needs a setting' + (cmd === 'set' ? ' and a value' : ''));
    log('commands.jsonl', { argv });
    if (cmd === 'set' && name === 'repo.key') {
      if (!repo) fail('repo.key belongs to a universe: add --repo NAME');
      if (Object.keys(w.keys).some((k) => k !== repo && w.keys[k] === value)) fail('the key ' + value + ' is taken');
      w.keys[repo] = value;
      save(w);
      publishRepos(w);
      console.log(repo + ': ' + name + ' = ' + value);
      return;
    }
    const ops = C.boardSettings(join('state', 'board-settings.json'), { models: DATA.models, efforts: DATA.board.efforts, builtin: DATA.board.builtin, keys: w.keys });
    const res = cmd === 'set' ? ops.set(name, value, repo, task) : ops.unset(name, repo, task);
    if (res.error) fail(res.error);
    console.log((task || repo || 'home') + ': ' + name + (cmd === 'set' ? ' = ' + value : ' unset'));
    return;
  }

  if (cmd === 'show') {
    const id = rest[1] || rest[0];
    console.log(tag(world(), id) + ': every setting\n* worker.agent              claude/claude-sonnet-5-5  this task\n  worker.effort             high\n');
    return;
  }

  if (cmd === 'details') {
    const w = world(); const r = find(w, rest[0]);
    log('details.jsonl', { id: rest[0] });
    if (!r) fail('No task ' + rest[0]);
    const i = r.id.lastIndexOf('/'); const repo = r.id.slice(0, i), n = r.id.slice(i + 1);
    const refs = (w.after || {})[r.id] || [];
    console.log(JSON.stringify({
      task: { id: r.id, title: r.title, stage: r.stage, msg: r.msg || '', description: 'Описание задачи ' + r.title + ': подробности, которые не влезают в строку доски.' },
      failure: r.state === 'error' ? { stage: 'agent', title: 'Сборка упала', detail: 'npm test: 3 failed\n<script>alert(1)</script>', next: 'Повторить: агент получит ошибку.' } : null,
      review: r.review || null,
      reviews: r.review ? Array.from({ length: r.reviewRound || 1 }, () => ({})) : [],
      agent: r.app,
      agent_model: r.app + '/model-5',
      report: r.state === 'done' ? { url: 'http://127.0.0.1:34925/alpha/' + n + '/review.html' } : null,
      plan: r.stage === 'plan' ? { text: '1. Снять копию базы\n2. Прогнать миграцию\n3. Проверить откат', by: 'claude/opus', locked: false } : null,
      pr: r.pr || null,
      worktree: path.join(D, 'worktrees', repo + '-' + n),
      branch: 'work/' + n,
      after: {
        waitsFor: refs, waiting: r.stage === 'after', left: refs, waitsForYou: null, notReachable: null,
        tasks: refs.map((x) => { const row = find(w, x); return { id: row ? row.id : x, title: row ? row.title : x, repo: row ? row.id.split('/')[0] : '', done: !!(row && row.state === 'done'), stage: row ? row.stage : 'after' }; }),
      },
      attempts: r.attempts || null,
      comparison: r.attempts ? { summary: 'A и B прошли проверку, C упал' } : null,
      picked: r.picked || null,
      limits: { time: { limit: 'none', usedMs: 240000, used: '4m' }, tokens: { limit: 'none', used: 0, total: 0 } },
    }));
    return;
  }

  if (cmd === 'after') {
    const w = world(); const id = resolve(w, rest[0] || '');
    if (!id || !find(w, id)) fail('No task ' + (rest[0] || ''));
    const clear = argv.includes('--clear');
    const refs = clear ? [] : rest.slice(1).join(' ').split(/[,\s]+/).filter(Boolean);
    if (!clear && refs.length === 0) { console.log(said(w, id)); return; }
    const deps = refs.map((r) => resolve(w, r));
    if (deps.some((d) => !d)) fail('No task ' + refs[deps.findIndex((d) => !d)]);
    // The circle: does anything the new wait names reach this task?
    const graph = w.after || {};
    const depsOf = (k) => (k === id ? deps : (graph[k] || [])).map((x) => resolve(w, x)).filter(Boolean);
    const chainFrom = (from, seen) => {
      if (from === id) return [id];
      if (seen.has(from)) return null;
      seen.add(from);
      for (const x of depsOf(from)) { const p = chainFrom(x, seen); if (p) return [from, ...p]; }
      return null;
    };
    for (const d of deps) {
      const p = chainFrom(d, new Set());
      if (p) fail(tag(w, id) + ' cannot wait for that: tasks would wait on each other (' + [id, ...p].map((x) => say0(w, x)).join(' → ') + '). Nothing was changed.', 1);
    }
    if (!w.after) w.after = {};
    if (deps.length) w.after[id] = deps; else delete w.after[id];
    save(w);
    publishTask(w, find(w, id));
    log('commands.jsonl', { argv });
    console.log(said(w, id) + (deps.length === 0 ? ', so it starts now' : ''));
    return;
  }
  function say0(w, x) { return w.rows.find((r) => r.id === x) ? tag(w, x) : x; }

  if (cmd === 'pick') {
    const w = world(); const r = find(w, rest[0]);
    log('commands.jsonl', { argv });
    if (!r) fail('No task ' + rest[0]);
    const letter = (rest[1] || '').toUpperCase();
    if (!(r.attempts || []).some((a) => a.letter === letter)) fail(tag(w, r.id) + ' has no attempt ' + letter + '. It has: A, B, C.');
    r.picked = letter; r.state = 'working'; r.stage = 'working'; r.msg = 'Attempt ' + letter + ' picked';
    save(w);
    console.log(tag(w, r.id) + ': picked ' + letter);
    return;
  }

  if (['approve', 'review', 'retry', 'delegate', 'stop', 'tell', 'archive', 'restore', 'fix', 'finish'].includes(cmd)) {
    const w = world(); const r = find(w, rest[0]);
    log('commands.jsonl', { argv });
    if (!r) fail('No task ' + rest[0]);
    if (fs.existsSync(join('state', 'refuse'))) fail('herdr-projects: cannot ' + cmd + ': the merge has a conflict');
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
    return;
  }

  fail('fixture herdr-projects: unsupported ' + argv.join(' '), 2);
}

function herdrMain(C, DATA) {
  const { fs, path, log, D, world } = C;
  const [cmd, sub, ...rest] = process.argv.slice(2);
  const join = (...p) => path.join(D, ...p);
  const namesOf = (w) => {
    const names = ['sw-coordinator'];
    for (const r of w.rows) {
      if (r.archivedAt) continue;
      const base = 'sw-' + r.id.split('/')[0] + '-' + r.id.split('/')[1];
      names.push(base);
      // The reviewer's terminal is there while it reviews or needs you; `state/no-reviewer` says it has gone.
      if ((r.stage === 'reviewneeds' || r.stage === 'reviewing') && !fs.existsSync(join('state', 'no-reviewer'))) names.push(base + '-r');
    }
    return names;
  };
  if (cmd === '--version') { console.log('herdr 0.9.4'); return; }
  if (cmd === 'machine' && sub === 'list') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    console.log(JSON.stringify(DATA.savedMachines));
    return;
  }
  if (cmd === 'agent' && sub === 'list') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    console.log(JSON.stringify({ id: 'cli:agent:list', result: { agents: namesOf(world()).map((name) => ({ name })) } }));
    return;
  }
  if (cmd === 'agent' && sub === 'read') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    const name = rest[0];
    if (!namesOf(world()).includes(name)) { console.error('no such agent: ' + name); process.exit(1); }
    const dialog = fs.existsSync(join('state', 'dialog.txt')) ? fs.readFileSync(join('state', 'dialog.txt'), 'utf8') : '❯ May I continue?\n  1. Yes\n  2. No';
    // A terminal screen, as `--format ansi` gives it: the prompt bright, the choices yellow.
    if (rest.includes('ansi')) {
      console.log(dialog.split('\n').map((line, i) => `\x1b[${i === 0 ? '1;36' : '33'}m${line}\x1b[0m`).join('\n'));
      return;
    }
    console.log(dialog);
    return;
  }
  if (cmd === 'agent' && sub === 'send-keys') {
    if (!namesOf(world()).includes(rest[0])) { console.error('no such agent: ' + rest[0]); process.exit(1); }
    log('keys.jsonl', { target: rest[0], keys: rest.slice(1) });
    // A program draws its next screen after a choice: `state/after-enter.txt` becomes the screen when Enter is sent.
    if (rest.slice(1).includes('enter') && fs.existsSync(join('state', 'after-enter.txt'))) {
      fs.writeFileSync(join('state', 'dialog.txt'), fs.readFileSync(join('state', 'after-enter.txt'), 'utf8'));
      fs.unlinkSync(join('state', 'after-enter.txt'));
    }
    console.log('sent');
    return;
  }
  if (cmd === 'agent' && sub === 'get') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    const name = rest[0];
    if (!namesOf(world()).includes(name)) { console.error('no such agent: ' + name); process.exit(1); }
    console.log(JSON.stringify({ id: 'cli:agent:get', result: { agent: { name, pane_id: 'w1:p-' + name }, type: 'agent_info' } }));
    return;
  }
  // The terminal brought to the front: the agent has to be there.
  if (cmd === 'agent' && sub === 'focus') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    const name = rest[0];
    if (!namesOf(world()).includes(name)) { console.error(JSON.stringify({ error: { code: 'agent_not_found', message: 'agent target ' + name + ' not found' } })); process.exit(1); }
    log('focus.jsonl', { target: name });
    console.log(JSON.stringify({ id: 'cli:agent:focus', result: { type: 'ok' } }));
    return;
  }
  // Literal text into a pane (the own answer of a selection menu): the pane is named `w1:p-<agent>`.
  if (cmd === 'pane' && sub === 'send-text') {
    log('herdr.jsonl', { argv: process.argv.slice(2) });
    log('text.jsonl', { pane: rest[0], text: rest.slice(1).join(' ') });
    console.log('sent');
    return;
  }
  console.error('fixture herdr: unsupported ' + process.argv.slice(2).join(' '));
  process.exit(2);
}

// The translator: reads the text between <<< and >>> on stdin; answers «ПЕРЕВОД: » and the first 60 characters.
function claudeMain(C, DATA) {
  const { log } = C;
  const argv = process.argv.slice(2);
  let input = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (d) => { input += d; });
  process.stdin.on('end', () => {
    const from = input.indexOf('<<<');
    const to = input.lastIndexOf('>>>');
    const text = from >= 0 && to > from ? input.slice(from + 3, to).trim() : input.trim();
    const chars = Array.from(text);
    const at = argv.indexOf('--tools');
    const mi = argv.indexOf('--model');
    log('claude.jsonl', { argv, toolsEmpty: at >= 0 && argv[at + 1] === '', model: mi >= 0 ? argv[mi + 1] : null, chars: chars.length });
    const si = argv.indexOf('--system-prompt');
    if (si >= 0 && String(argv[si + 1]).includes('долговременную память')) {
      const tag = (/Задача: (\S+)/.exec(text) || [])[1] || 'TASK';
      log('claude.jsonl', { suggest: tag });
      process.stdout.write('- decision: ' + tag + ' — тема не хранится в базе: хватает localStorage и системной настройки.\n- pitfall: класс темы ставить до первой отрисовки, иначе страница мигает.\n');
      return;
    }
    process.stdout.write('ПЕРЕВОД: ' + chars.slice(0, 60).join('') + (chars.length > 60 ? '…' : '') + '\n');
  });
}

function ghMain(C, DATA) {
  const { log } = C;
  const a = process.argv.slice(2);
  log('gh.jsonl', { argv: a });
  if (a[0] === 'auth' && a[1] === 'status') {
    console.log('github.com\n  ✓ Logged in to github.com account fixture-user (keyring)');
    return;
  }
  if (a[0] === 'api' && a[1] === 'user') {
    console.log('fixture-user');
    return;
  }
  if (a[0] === 'repo' && a[1] === 'view') {
    const repo = a[2] || '';
    const perm = DATA.gh[repo];
    if (!perm) {
      console.error("GraphQL: Could not resolve to a Repository with the name '" + repo + "'. (repository)");
      process.exit(1);
    }
    console.log(JSON.stringify({ viewerPermission: perm, isFork: false, parent: null, defaultBranchRef: { name: 'main' }, nameWithOwner: repo, isPrivate: false }));
    return;
  }
  console.error('gh fixture: unsupported ' + a.join(' '));
  process.exit(2);
}

// gitlab.example.com is not logged in here: the hint of the planner is what this exercises.
function glabMain(C, DATA) {
  const { log } = C;
  const a = process.argv.slice(2);
  log('glab.jsonl', { argv: a });
  const host = a[a.indexOf('--hostname') + 1] || '';
  if (a[0] === 'auth' && a[1] === 'status') {
    if (host === 'gitlab.example.com') {
      console.error('You are not logged into any GitLab hosts. To log in to ' + host + ', run: glab auth login --hostname ' + host);
      process.exit(1);
    }
    console.log('✓ Logged in to ' + host + ' as fixture-user');
    return;
  }
  if (a[0] === 'api') {
    console.error('401 Unauthorized');
    process.exit(1);
  }
  console.error('glab fixture: unsupported ' + a.join(' '));
  process.exit(2);
}

// The shipyard's stand-in: one fixed probe, docker version / system df, and exact-id removals. The removed ids are
// remembered in state/runner.json so the next probe leaves them out; every call is logged to ssh.jsonl.
function sshMain(C, DATA) {
  const { fs, path, log } = C;
  const args = process.argv.slice(2);
  if (args[0] === '-G') {
    const t = args[1] || '';
    const host = t.includes('@') ? t.split('@')[1] : t;
    console.log(`host ${host}`);
    console.log(`hostname ${host.split(':')[0]}`);
    console.log(`user ${t.includes('@') ? t.split('@')[0] : ''}`);
    return;
  }
  const target = args.length >= 2 ? args[args.length - 2] : '';
  const remote = args.length ? args[args.length - 1] : '';
  const unwrapped = /^exec "\$\{SHELL:-\/bin\/sh\}" -lc '([\s\S]*)'$/.exec(remote);
  const inner = unwrapped ? unwrapped[1].split("'\\''").join("'") : remote;
  const host = target.includes('@') ? target.split('@')[1] : target;
  const R = DATA.runnerR;
  if (host !== R.host && host !== R.label) {
    console.error('ssh: Could not resolve hostname ' + host + ': Name or service not known');
    process.exit(255);
  }
  const stateFile = path.join(C.D, 'state', 'runner.json');
  const st = C.readJson(stateFile, { ids: [], networks: [], volumes: [] });
  st.ids = st.ids || []; st.networks = st.networks || []; st.volumes = st.volumes || []; st.builders = st.builders || [];
  // What a check adds to the yard on top of DATA.runnerR (containers, networks, volumes), written into this file.
  const extra = st.extra || {};
  const allContainers = R.containers.concat(extra.containers || []);
  const allNetworks = R.networks.concat(extra.networks || []);
  const allVolumes = R.volumes.concat(extra.volumes || []);
  const saveSt = () => fs.writeFileSync(stateFile, JSON.stringify(st));
  log('ssh.jsonl', { argv: args, target, inner: inner.slice(0, 400), runner: R.label });
  if (inner === 'true') return;
  if (inner.startsWith('docker version')) { console.log('28.3.0'); return; }
  if (inner.startsWith('docker image ls -q --filter label=herdr.shared')) {
    (R.sharedImages || []).forEach((i) => console.log(i.id)); return;
  }
  if (inner.startsWith('docker image inspect --format')) {
    (R.sharedImages || []).filter((i) => inner.includes(i.id)).forEach((i) => console.log(JSON.stringify({
      Id: i.id, RepoTags: i.tags, Size: i.size,
      Config: { Labels: Object.fromEntries((i.labels || '').split(',').filter(Boolean).map((p) => p.split('='))) },
    })));
    return;
  }
  if (inner.startsWith('docker system df')) {
    console.log(JSON.stringify({ Type: 'Images', TotalCount: '152', Active: '12', Size: '159.7GB', Reclaimable: '153.1GB (96%)' }));
    console.log(JSON.stringify({ Type: 'Containers', TotalCount: '6', Active: '4', Size: '1.2GB', Reclaimable: '12MB (1%)' }));
    return;
  }
  if (inner.startsWith('docker rm -v ')) {
    const ids = inner.slice('docker rm -v '.length).trim().split(/\s+/).filter(Boolean);
    ids.forEach((id) => st.ids.push(id)); saveSt(); ids.forEach((id) => console.log(id)); return;
  }
  if (inner === 'docker image prune -a -f') {
    st.imagesPruned = true; saveSt();
    console.log('Deleted Images:\nuntagged: old/app:1\n\nTotal reclaimed space: 2GB'); return;
  }
  if (inner === 'docker builder prune -a -f') {
    st.cachePruned = true; saveSt();
    console.log('Total:\t2.4GB'); return;
  }
  if (inner.startsWith('docker buildx rm ')) {
    const name = inner.slice('docker buildx rm '.length).trim();
    const b = (R.builders || []).find((x) => x.name === name);
    if (!b) { console.error('ERROR: no builder ' + name + ' found'); process.exit(1); }
    if (b.status === 'running') { console.error('ERROR: builder ' + name + ' is running (the check never removes a running builder)'); process.exit(1); }
    st.builders.push(name); st.volumes.push('buildx_buildkit_' + b.node + '_state'); saveSt(); console.log(name + ' removed'); return;
  }
  // One container: the log tail (a line more at every call, so the page's refresh shows), stop and restart. Stopped
  // ids and restarted ones are remembered in the state file; the probe shows them.
  const logsRe = /^docker logs --tail (\d+) ([0-9a-f]{64}) 2>&1$/.exec(inner);
  if (logsRe) {
    const c = allContainers.find((x) => x.id === logsRe[2] && !st.ids.includes(x.id));
    if (!c) { console.log('Error response from daemon: No such container: ' + logsRe[2]); process.exit(1); }
    st.logCalls = (st.logCalls || 0) + 1; saveSt();
    const total = 40 + st.logCalls;
    const lines = [];
    // The journal of a yard command (PLN-56): a header before the output, a line with the exit code and the time after it.
    for (let i = 1; i <= total; i++) {
      if (i === 1) lines.push('[yard] \u25b6 2026-01-01 10:00:00 \u00b7 yard.sh check \u00b7 run 20260101-100000-1');
      lines.push((i % 17 === 0 ? 'ERROR ' : i % 5 === 0 ? 'WARN  ' : 'INFO  ') + c.name + ' line ' + i + (i % 9 === 0 ? ' \u001b[31mred\u001b[0m' : ''));
      if (i === 12) lines.push('[yard] \u25a0 2026-01-01 10:00:09 \u00b7 yard.sh check \u00b7 exit 0 \u00b7 9s');
      if (i === 24) lines.push('[yard] \u25a0 2026-01-01 10:01:09 \u00b7 yard.sh test \u00b7 exit 101 \u00b7 1m 9s');
    }
    console.log(lines.slice(-Number(logsRe[1])).join('\n'));
    return;
  }
  const stopRe = /^docker (stop|restart) ([0-9a-f]{64})$/.exec(inner);
  if (stopRe) {
    const c = allContainers.find((x) => x.id === stopRe[2] && !st.ids.includes(x.id));
    if (!c) { console.error('Error response from daemon: No such container: ' + stopRe[2]); process.exit(1); }
    st.stopped = (st.stopped || []).filter((x) => x !== c.id);
    st.started = (st.started || []).filter((x) => x !== c.id);
    (stopRe[1] === 'stop' ? st.stopped : st.started).push(c.id);
    saveSt(); console.log(c.id); return;
  }
  if (inner.startsWith('docker rm -f -v ')) {
    const ids = inner.slice('docker rm -f -v '.length).trim().split(/\s+/).filter(Boolean);
    ids.forEach((id) => st.ids.push(id)); saveSt(); ids.forEach((id) => console.log(id)); return;
  }
  if (inner.startsWith('docker network rm ')) {
    const ids = inner.slice('docker network rm '.length).trim().split(/\s+/).filter(Boolean);
    ids.forEach((id) => st.networks.push(id)); saveSt(); ids.forEach((id) => console.log(id)); return;
  }
  if (inner.startsWith('docker volume rm ')) {
    const names = inner.slice('docker volume rm '.length).trim().split(/\s+/).filter(Boolean);
    names.forEach((n) => st.volumes.push(n)); saveSt(); names.forEach((n) => console.log(n)); return;
  }
  if (inner.includes('herdr-planner-runner 1')) {
    const gone = new Set(st.ids);
    const goneNet = new Set(st.networks);
    const goneVol = new Set(st.volumes);
    const out = [];
    const row = (...f) => out.push(f.join('\t'));
    out.push('herdr-planner-runner 1');
    row('now', String(Math.floor(Date.now() / 1000)));
    row('load', '1.20 0.80 0.50 1/100 2');
    row('cpus', '8');
    row('arch', 'x86_64');
    row('host', 'vm-r');
    row('os', 'Ubuntu 24.04 LTS');
    row('mem', 'MemTotal:       16777216 kB');
    row('mem', 'MemAvailable:   12582912 kB');
    row('mem', 'SwapTotal:       2097152 kB');
    row('mem', 'SwapFree:        2097152 kB');
    row('disk', '/var/lib/docker\t' + (st.disk ? st.disk.size + ' ' + st.disk.avail : '500000000000 300000000000'));
    row('docker', '28.3.0 x86_64');
    row('compose', 'v2.17.3');
    row('info', '8|17179869184|6|4|40|Ubuntu 24.04 LTS|runc,nvidia,');
    row('gpu', '0, NVIDIA L4-24Q, 12, 1337, 24576, 45');
    row('gpuproc', '4242,1337\tdocker-' + 'a'.repeat(64));
    out.push('== ps');
    const stopped = new Set(st.stopped || []);
    const started = new Set(st.started || []);
    const stateOf = (c) => (stopped.has(c.id) ? 'exited' : started.has(c.id) ? 'running' : c.state);
    const statusOf = (c) => (stopped.has(c.id) ? 'Exited (0) 1 second ago' : started.has(c.id) ? 'Up 1 second' : c.status);
    allContainers.filter((c) => !gone.has(c.id)).forEach((c) => out.push(JSON.stringify({
      ID: c.id, Names: c.name, Image: c.image, State: stateOf(c), Status: statusOf(c), CreatedAt: c.created,
      Labels: c.labels, Ports: c.ports, Networks: c.networks, Mounts: c.mounts, Size: c.size,
    })));
    out.push('== stats');
    allContainers.filter((c) => !gone.has(c.id)).forEach((c) => out.push(JSON.stringify({
      ID: c.id.slice(0, 12), CPUPerc: c.cpu, MemUsage: c.mem, PIDs: c.pids, NetIO: '1kB / 2kB', BlockIO: '0B / 0B',
    })));
    out.push('== lim');
    allContainers.filter((c) => !gone.has(c.id)).forEach((c) => out.push([c.id, c.nano || 0, c.memLimit || 0, 0, 0].join('|')));
    out.push('== net');
    allNetworks.filter((n) => !goneNet.has(n.id) && !goneNet.has(n.name)).forEach((n) => out.push(JSON.stringify({ ID: n.id, Name: n.name, Driver: n.driver, Labels: n.labels || '' })));
    out.push('== vol');
    allVolumes.filter((v) => !goneVol.has(v.name)).forEach((v) => out.push(JSON.stringify({ Name: v.name, Driver: v.driver, Labels: v.labels || '' })));
    out.push('== img');
    (R.sharedImages || []).forEach((i) => out.push(JSON.stringify({
      Id: i.id, RepoTags: i.tags, Size: i.size,
      Config: { Labels: Object.fromEntries((i.labels || '').split(',').filter(Boolean).map((p) => p.split('='))) },
    })));
    process.stdout.write(out.join('\n') + '\n');
    return;
  }
  if (inner.includes('herdr-planner-disk 1')) {
    const gone = new Set(st.ids);
    const goneVol = new Set(st.volumes);
    const cs = allContainers.filter((c) => !gone.has(c.id));
    const vols = allVolumes.filter((v) => !goneVol.has(v.name));
    const links = (v) => (v.links !== undefined ? v.links : cs.filter((c) => (c.mounts || '').split(',').some((m) => m.split(':')[0] === v.name)).length);
    const imgs = (R.images || []).filter((i) => !(st.imagesPruned && i.containers === 0));
    const builders = (R.builders || []).filter((b) => !st.builders.includes(b.name));
    const out = ['herdr-planner-disk 1', '== df'];
    out.push(JSON.stringify({ Type: 'Images', TotalCount: String(imgs.length), Active: String(imgs.filter((i) => i.containers > 0).length), Size: '3GB', Reclaimable: st.imagesPruned ? '0B' : R.imagesReclaimable }));
    out.push(JSON.stringify({ Type: 'Build Cache', TotalCount: st.cachePruned ? '0' : '7', Active: '0', Size: st.cachePruned ? '0B' : '2.4GB', Reclaimable: st.cachePruned ? '0B' : R.cacheReclaimable }));
    out.push('== dfv');
    out.push(JSON.stringify({
      Images: imgs.map((i) => ({ Repository: i.repo, Tag: i.tag, Containers: String(i.containers), Size: i.size, ID: 'sha256:' + i.repo.length })),
      Containers: cs.map((c) => ({ ID: c.id, Names: c.name, Image: c.image, State: c.state, Status: c.status, CreatedAt: c.created, Labels: c.labels, Mounts: c.mounts, Networks: c.networks, Size: c.size })),
      Volumes: vols.map((v) => ({ Name: v.name, Driver: v.driver, Links: String(links(v)), Size: v.size || '0B', Labels: v.labels || '' })),
      BuildCache: [],
    }));
    out.push('', '== buildx');
    out.push('NAME/NODE DRIVER/ENDPOINT STATUS  BUILDKIT PLATFORMS', 'default * docker', '  default default         running v0.23.1  linux/amd64');
    builders.forEach((b) => { out.push(b.name + ' ' + b.driver, '  ' + b.node + ' unix:///var/run/docker.sock ' + b.status + (b.status === 'running' ? ' v0.12.0 linux/amd64' : '')); });
    process.stdout.write(out.join('\n') + '\n');
    return;
  }
  console.error('ssh fixture: unsupported ' + inner.slice(0, 120));
  process.exit(2);
}

function xdgMain(C) {
  const argv = process.argv.slice(2);
  C.log('terminal.jsonl', { argv });
  console.log('opened');
}

function writeBin(name, fn) {
  const src = '#!/usr/bin/env node\n' +
    `// Generated by scripts/fixture.mjs: the stand-in for ${name}. Do not edit; rerun the generator.\n` +
    `const DATA = ${JSON.stringify(DATA)};\n` +
    `const COMMON = ${common.toString()};\n` +
    "const C = COMMON(DATA, { fs: require('node:fs'), path: require('node:path'), crypto: require('node:crypto') });\n" +
    `(${fn.toString()})(C, DATA);\n`;
  const p = path.join(dir, 'bin', name);
  fs.writeFileSync(p, src);
  fs.chmodSync(p, 0o755);
}
writeBin('herdr-projects', herdrProjectsMain);
writeBin('herdr', herdrMain);
writeBin('claude', claudeMain);
writeBin('gh', ghMain);
writeBin('glab', glabMain);
writeBin('ssh', sshMain);
writeBin('xdg-terminal-exec', xdgMain);

// ---------------------------------------------------------------- the world's state and files

const w = {
  keys: { ...KEYS },
  folders: {},
  rows,
  after,
  settings: {},
  meta: {},
};
for (const repo of Object.keys(KEYS)) w.folders[repo] = folders[repo];
Object.assign(w.meta, W);

function addScale(w) {
  const perUniverse = Math.max(1, Math.round(scale.planets / scale.universes));
  let seed = 20261020;
  const rnd = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648; };
  for (let u = 1; u <= scale.universes; u++) {
    const name = 's' + String(u).padStart(2, '0');
    const key = 'S' + String(u).padStart(2, '0');
    const folder = path.join(dir, 'projects', GROUPS[u % GROUPS.length], name);
    fs.mkdirSync(folder, { recursive: true });
    w.keys[name] = key;
    w.folders[name] = folder;
    const quiet = u % 10 < 3; // 30 % of the universes have finished work only
    for (let p = 1; p <= perUniverse; p++) {
      const tpl = quiet ? PLANET[7] : PLANET[(u + p) % PLANET.length];
      const row = { id: `${name}/${p}`, app: p % 7 === 0 ? 'omp' : 'claude', ...tpl, title: `Планета ${name}-${p}` };
      const ago = Math.floor(rnd() * 9 * DAY);
      const created = NOW - Math.max(2 * MIN, ago);
      w.rows.push(row);
      w.meta[row.id] = {
        created,
        at: row.state === 'done' ? created + MIN : created,
        description: `Планета ${row.id}: ${row.title}. Сгенерирована для проверки на большом объёме.`,
        settings: { 'worker.agent': 'claude/claude-sonnet-5-5' },
        model: 'claude-sonnet-5-5',
        session: C0.uuid('scale ' + row.id),
        worktree: path.join(dir, 'worktrees', `${name}-${p}`),
        branch: `work/${p}`,
        exitedAt: row.state === 'done' ? created + MIN : null,
        afterDone: [],
      };
      fs.mkdirSync(w.meta[row.id].worktree, { recursive: true });
    }
  }
}
if (scale) addScale(w);

// Everything the planner reads from the board's own folder.
C0.publishRepos(w);
for (const row of w.rows) {
  C0.publishTask(w, row);
  fs.mkdirSync(path.join(dir, 'reports', row.id.split('/')[0], row.id.split('/')[1]), { recursive: true });
}
for (const [id, text] of Object.entries(CLOSING)) {
  const [repo, n] = id.split('/');
  write(path.join(dir, 'board', 'tasks', repo, n, 'closing.md'), text);
}
// The stand-ins' world (the rows, the waits, the meta) lives under state/.
fs.writeFileSync(path.join(dir, 'state', 'board.json'), JSON.stringify(w, null, 1));
write(path.join(dir, 'theme', 'colors.toml'), `mode = "dark"
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
// Omarchy's agent usage records: Claude has real subscription windows, omp and pi are API-key
// agents and must stay honestly empty. Pi's record is old enough to exercise the stale marking.
const usageAt = Date.now();
const inHours = (hours) => new Date(usageAt + hours * 3600 * 1000).toISOString();
const claudeLimits = [
  { label: 'Session (5-hour)', percent: 0.21, resetsAt: inHours(2) },
  // 30 hours guarantees another calendar day, so the page must add a date.
  { label: 'Weekly (7-day)', percent: 0.91, resetsAt: inHours(30) },
  { label: 'Fable Weekly', percent: 0.97, resetsAt: inHours(30), title: 'Fable Weekly' },
];
const usageDir = path.join(dir, 'state', 'omarchy', 'agents', 'usage');
write(path.join(usageDir, 'claude.json'), JSON.stringify({
  schemaVersion: 1, id: 'claude', name: 'Claude Code', updatedAt: new Date(usageAt).toISOString(),
  ready: true, hasLocalStats: true, tierLabel: 'Max 5x', usageStatusText: '', limits: claudeLimits,
  todayPrompts: 0, todaySessions: 0, todayTotalTokens: 0, todayTokensByModel: {}, recentDays: [],
  totalPrompts: 0, totalSessions: 0, activeDays: 0, activeDates: [], modelUsage: {},
}));
for (const [id, name] of [['omp', 'OMP'], ['pi', 'Pi']]) {
  write(path.join(usageDir, `${id}.json`), JSON.stringify({
    id, name, ready: true, usageStatusText: 'API keys · no subscription',
    totalPrompts: 0, totalSessions: 0, activeDays: 0, todayPrompts: 0, todaySessions: 0,
    todayTotalTokens: 0, todayTokensByModel: {}, recentDays: [], modelUsage: {},
    hasLocalStats: true, hasPromptStats: true,
  }));
}
write(path.join(dir, 'cache', 'omarchy', 'agent-usage', 'claude-limits.json'), JSON.stringify({ fetchedAtMs: usageAt, limits: claudeLimits }));
const old = new Date(usageAt - 2 * 3600 * 1000);
fs.utimesSync(path.join(usageDir, 'pi.json'), old, old);
// The board's own settings (show, set and unset read and write this file): the board with its repo alpha.
write(path.join(dir, 'state', 'board-settings.json'), JSON.stringify({ '': BOARD.seed.home, alpha: BOARD.seed.alpha }, null, 1));
// The same levels in the board's real shape (settings/config.json), as the planner reads a scope's own instructions.
write(path.join(dir, 'settings', 'config.json'), '{}');
// The planner's own list of shipyards, at the path it reads: <data>/../settings/runners.json.
write(path.join(dir, 'settings', 'runners.json'), JSON.stringify({
  version: 1, updated_at_ms: NOW, default: 'vm-r',
  runners: [{
    id: 'vm-r', target: DATA.runnerR.target, ports_host: DATA.runnerR.ports,
    tags: ['docker28', 'gpu', 'x86_64'],
    protected: ['buildx_*', 'neighbor'],
    note: '8 ядер, L4 24 ГБ (фикстура)', added_at_ms: NOW, capacity: null,
  }],
  assign: {},
}, null, 1));
write(path.join(dir, 'state', 'runner.json'), JSON.stringify({ ids: [], networks: [], volumes: [] }));
// The crew's programs on PATH (the planner looks for them there): stand-ins for codex and omp (claude is above).
for (const name of ['codex', 'omp']) {
  const p = path.join(dir, 'bin', name);
  fs.writeFileSync(p, '#!/bin/sh\necho "' + name + ' stand-in"\n');
  fs.chmodSync(p, 0o755);
}
// The video driver: `nvidia-smi` answers from load/nvidia.csv (the lines it would print) and, with no such file, fails the way a machine
// without a card does. The planner's own reading of the machine can be replaced by load/load.json (HERDR_PLANNER_LOAD_FILE).
fs.mkdirSync(path.join(dir, 'load'), { recursive: true });
fs.writeFileSync(path.join(dir, 'bin', 'nvidia-smi'), '#!/bin/sh\nf="$(dirname "$0")/../load/nvidia.csv"\n[ -f "$f" ] && exec cat "$f"\necho "NVIDIA-SMI has failed because it could not communicate with the NVIDIA driver." >&2\nexit 9\n');
fs.chmodSync(path.join(dir, 'bin', 'nvidia-smi'), 0o755);
// The planner's own look at this machine's processes (HERDR_PLANNER_PROC_FILE, PLN-53): a table of stand-ins, never real ones.
// The leftovers of a done task (an agent, its MCP helper, and a broker that ignores SIGTERM), of an archived one, and of a
// copy no task has; what must stay: a task in work, a folder outside the board's copies, another user's process, a shell
// of a done task (listed, but the automatic cleaning leaves it). `FILE.signals` records what was «sent».
{
  const copy = (id) => W[id].worktree;
  const P = (pid, ppid, cwd, cmd, extra = {}) => ({
    pid, ppid, pgid: pid, comm: cmd.split(' ')[0].split('/').pop(), cmd, cwd, rss: 48 * 1024 * 1024, age_ms: 3 * 3600 * 1000, start: 5000 + pid, ...extra,
  });
  write(path.join(dir, 'load', 'procs.json'), JSON.stringify({
    uid: 1000,
    pid: 0,
    procs: [
      P(9101, 1, copy('alpha/13'), 'omp --resume=7f3a1c'),
      P(9102, 9101, copy('alpha/13'), 'node /opt/omp/rea.mjs mcp', { pgid: 9102, rss: 120 * 1024 * 1024 }),
      P(9103, 9101, path.join(copy('alpha/13'), 'src'), 'omp __omp_worker_daemon_broker', { ignore_term: true }),
      P(9111, 1, copy('gamma/2'), 'node /opt/omp/rea.mjs mcp', { pgid: 9111 }),
      P(9121, 1, copy('beta/1'), 'zsh -l', { rss: 6 * 1024 * 1024 }),
      P(9151, 1, path.join(dir, 'worktrees', 'alpha', 'work-12'), 'node /opt/omp/rea.mjs mcp', { pgid: 9151 }),
      P(9131, 1, copy('alpha/6'), 'omp --auto-approve'),
      // a task started after the board was read: its number is above the newest the board knows
      P(9171, 1, path.join(dir, 'worktrees', 'alpha', 'work-70'), 'omp --auto-approve'),
      P(9141, 1, path.join(dir, 'projects', 'AI', 'alpha'), 'node dev-server.js'),
      P(9161, 1, copy('alpha/13'), 'node other-users-process.js', { uid: 0 }),
    ],
  }, null, 1));
}
write(path.join(dir, 'theme.name'), 'Fixture Night');
write(path.join(dir, 'herdr.sock'), '');
fs.rmSync(path.join(dir, '.seed'), { recursive: true, force: true });
console.log(dir);
