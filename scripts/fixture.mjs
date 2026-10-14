#!/usr/bin/env node
// A self-contained world for the page checks: a projects root with three
// projects, real git repositories and worktrees, and stand-ins for the
// binaries the planner calls (`herdr-projects`, `herdr`, `claude`) that log
// what they were asked and never touch a real coordinator or sidebar.
//
//   node scripts/fixture.mjs DIR        builds DIR (replacing what is there)
//   HERDR_PROJECTS_BIN=DIR/bin/herdr-projects HERDR_BIN=DIR/bin/herdr \
//   PATH=DIR/bin:$PATH FIXTURE_DIR=DIR herdr-planner --root DIR/root --port N
//
// What the stand-ins do is in `bin/`; their logs are `DIR/log/*.jsonl`.

import { execFileSync } from 'node:child_process';
import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const dir = resolve(process.argv[2] || '');
if (!process.argv[2]) { console.error('usage: node scripts/fixture.mjs DIR'); process.exit(2); }
rmSync(dir, { recursive: true, force: true });
for (const d of ['root', 'repos', 'worktrees', 'bin', 'log', 'state', 'data']) mkdirSync(join(dir, d), { recursive: true });

const write = (path, text) => { mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, text); };
const git = (cwd, ...args) => execFileSync('git', args, {
  cwd, stdio: 'pipe',
  env: { ...process.env, GIT_AUTHOR_NAME: 'T', GIT_AUTHOR_EMAIL: 't@t', GIT_COMMITTER_NAME: 'T', GIT_COMMITTER_EMAIL: 't@t' },
});

// --- the repository of `alpha`: main with a squash-merged PR, three worktrees
const repo = join(dir, 'repos/alpha');
mkdirSync(repo, { recursive: true });
git(repo, 'init', '-q', '-b', 'main');
write(join(repo, 'README.md'), '# alpha\n');
git(repo, 'add', '.'); git(repo, 'commit', '-qm', 'Initial commit');
write(join(repo, 'export.txt'), 'экспорт\n');
git(repo, 'add', '.');
git(repo, 'commit', '-qm', 'Экспорт отчёта (#7)', '-m', 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\nCo-Authored-By: Anna Petrova <anna@example.com>');
const wt = (name, branch) => { const p = join(dir, 'worktrees', name); git(repo, 'worktree', 'add', '-q', '-b', branch, p); return p; };
const w5 = wt('t-0005', 'hp/alpha/t-0005-1');   // working: a commit and files not committed
write(join(w5, 'src/a.rs'), 'fn a() {}\n'); write(join(w5, 'src/b.rs'), 'fn b() {}\n');
git(w5, 'add', '.'); git(w5, 'commit', '-qm', 'Добавить a и b', '-m', 'Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>');
write(join(w5, 'notes.txt'), 'wip\n'); write(join(w5, 'src/a.rs'), 'fn a() { 1 }\n');
const w3 = wt('t-0003', 'hp/alpha/t-0003-1');   // ready for review: a PR open
write(join(w3, 'docs.md'), 'docs\n'); git(w3, 'add', '.'); git(w3, 'commit', '-qm', 'Документация');
const w8 = wt('t-0008', 'hp/alpha/t-0008-1');   // finished, worktree kept: its trailers come from the branch
write(join(w8, 'x.txt'), 'x\n'); git(w8, 'add', '.'); git(w8, 'commit', '-qm', 'Икс', '-m', 'Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>');

// --- repositories the «Новый проект» form finds: a git one with a subfolder, a plain one, a hidden one
const delta = join(dir, 'repos/delta');
mkdirSync(delta, { recursive: true });
git(delta, 'init', '-q', '-b', 'main');
write(join(delta, 'src/lib.rs'), 'fn main() {}\n');
git(delta, 'add', '.'); git(delta, 'commit', '-qm', 'Initial commit');
mkdirSync(join(dir, 'repos/plain'), { recursive: true });
write(join(dir, 'repos/.hidden/x'), '');

// --- the projects
const project = (slug, name, goal, repos = '', status) => write(join(dir, 'root', slug, 'PROJECT.md'),
  `+++\nname = "${name}"\ngoal = "${goal}"\n${status ? `status = "${status}"\n` : ''}${repos}+++\n\n# Instructions\n`);
project('alpha', 'Alpha', 'Проект для проверки карточки', `[[repos]]\npath = "${repo}"\n\n[[repos]]\npath = "/srv/alpha-mac"\nmachine = "mac"\n`);
project('beta', 'Beta', 'Пустой проект на паузе', '', 'paused');
project('gamma', 'Gamma', 'Только задачи', `[[repos]]\npath = "${repo}"\n`);
write(join(dir, 'root/alpha/TASKS.md'), `# Tasks

## Backlog
- [ ] Починить экспорт отчёта (claude-sonnet)
  Заметки: экспорт падает на пустом файле.
  Смотреть export.txt.
- [ ] Добавить **тёмную** тему
- [ ] (me) Выпустить ключ API
- [ ] Миграция базы (claude-opus) · t-0005
- [x] Готовая задача (2026-10-01)
`);
write(join(dir, 'root/gamma/TASKS.md'), '# Tasks\n\n- [ ] Единственная задача гаммы\n');
const report = (slug, id, text) => write(join(dir, 'root', slug, 'threads', `${id}.md`), text);
report('alpha', 't-0001', 'PR: https://github.com/o/alpha/pull/7\n\n## Report\n\n**Что сделано.**\n\nЭкспорт отчёта готов, PR слит, тесты зелёные.\n\n## Next\n\nУдалить ворктри\n');
report('alpha', 't-0004', '## Report\n\nСравнил три варианта и выбрал второй: он проще в поддержке.\n\n## Next\n');

// --- threads as `thread list --json` prints them
const base = { agent: 'claude', branch: '', base: 'origin/main', worktree_path: '', cwd: '', kind: 'worktree', pr: '', pr_state: '', profile: 'claude-sonnet', profile_args: [], machine: '', remote_profile: false, repo, error: '', launched_at: '2026-10-14T05:00:00Z', percent: null, activity: '', next: [], created: '', updated: '', rank: 4, status: 'open', note: 'working', group: 'Working' };
const threads = [
  { id: 't-0007', title: 'Сбой запуска при старте', group: 'Waiting on you', status: 'failed', note: 'failed: herdr: timed out', error: 'herdr: timed out waiting for the pane to appear', launched_at: '', branch: 'hp/alpha/t-0007-1' },
  { id: 't-0005', title: 'Миграция базы', percent: 40, activity: 'Writing migration', branch: 'hp/alpha/t-0005-1', worktree_path: w5, cwd: w5 },
  { id: 't-0003', title: 'Документация', group: 'Ready for review', note: 'pane closed', profile: 'claude-haiku', pr: 'https://github.com/o/alpha/pull/12', pr_state: 'OPEN', branch: 'hp/alpha/t-0003-1', worktree_path: w3, cwd: w3 },
  { id: 't-0002', title: 'Разбор логов во вкладке', group: 'Idle', note: 'idle', kind: 'tab', cwd: repo, profile: 'pi', agent: 'pi' },
  { id: 't-0001', title: 'Экспорт отчёта', group: 'Resolved', status: 'resolved', note: 'merged', pr: 'https://github.com/o/alpha/pull/7', pr_state: 'MERGED', branch: 'hp/alpha/t-0001-1', cwd: join(dir, 'worktrees/gone') },
  { id: 't-0004', title: 'Выбор подхода', group: 'Resolved', status: 'resolved', note: 'manual', profile: 'deepseek-flash', agent: 'omp', branch: '' },
  { id: 't-0006', title: 'Не запустился и закрыт', group: 'Resolved', status: 'resolved', note: 'manual', error: 'herdr: timed out', launched_at: '', branch: '' },
  { id: 't-0008', title: 'Икс', group: 'Resolved', status: 'resolved', note: 'manual', branch: 'hp/alpha/t-0008-1', worktree_path: w8, cwd: w8, profile: 'claude-opus' },
].map((t) => ({ ...base, ...t }));
write(join(dir, 'data/threads-alpha.json'), JSON.stringify(threads, null, 1));
write(join(dir, 'data/profiles.json'), JSON.stringify({
  'claude-sonnet': { agent: 'claude', args: ['--model', 'claude-sonnet-5-5', '--effort', 'high'], name: 'claude-sonnet' },
  'claude-haiku': { agent: 'claude', args: ['--model', 'claude-haiku-5-5'], name: 'claude-haiku' },
  'claude-opus': { agent: 'claude', args: ['--model', 'claude-opus-5-5'], name: 'claude-opus' },
  'deepseek-flash': { agent: 'omp', args: ['--model', 'deepseek/deepseek-v4-flash'], name: 'deepseek-flash' },
  pi: { agent: 'pi', args: [], name: 'pi' },
}, null, 1));

write(join(dir, 'data/profile-list.txt'), `Profiles (built-ins: the agent CLIs installed and signed in here: pi, claude, omp):
  claude         claude · model claude-opus-5-5 · effort medium — Координатор
  claude-haiku   claude · model claude-haiku-5-5 · effort high — Лёгкий тред
  claude-sonnet  claude · model claude-sonnet-5-5 · effort high — Тред по умолчанию
  pi             pi · default model  (built-in)

Defaults for new projects: thread_profile = claude-sonnet, coordinator_profile = claude
Allowed for threads in projects without their own list: claude-sonnet, claude-haiku
Allowed for the coordinator in projects without their own list: claude
`);

// --- stand-ins
const bin = (name, body) => { const p = join(dir, 'bin', name); write(p, `#!/usr/bin/env node\n${body}`); chmodSync(p, 0o755); };
const prelude = `const fs = require('node:fs'); const path = require('node:path');
const D = process.env.FIXTURE_DIR; const log = (f, o) => fs.appendFileSync(path.join(D, 'log', f), JSON.stringify(o) + '\\n');
const readStdin = () => { try { return fs.readFileSync(0, 'utf8'); } catch { return ''; } };
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
`;
bin('herdr-projects', `${prelude}
const argv = process.argv.slice(2); const i = argv.indexOf('--root'); if (i >= 0) argv.splice(i, 2);
const [cmd, ...rest] = argv;
const opened = (slug) => fs.existsSync(path.join(D, 'state', 'opened-' + slug));
const created = () => { try { return fs.readFileSync(path.join(D, 'state', 'created'), 'utf8').split('\\n').filter(Boolean); } catch { return []; } };
if (cmd === 'list') {
  console.log('alpha\\tactive\\tReady for review: 1, Working: 1, Resolved: 4\\nbeta\\tpaused\\tno threads\\ngamma\\tactive\\tno threads' + created().map((s) => '\\n' + s + '\\tactive\\tno threads').join(''));
} else if (cmd === 'profile' && rest[0] === 'list') {
  console.log(fs.readFileSync(path.join(D, 'data', 'profile-list.txt'), 'utf8'));
} else if (cmd === 'new') {
  // The arguments as the planner sends them: new --repo=P[@M] ... --goal=G ... -- NAME
  const dd = rest.indexOf('--'); const name = dd >= 0 ? rest.slice(dd + 1).join(' ') : rest[0];
  log('new.jsonl', { argv: rest });
  const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  const repos = rest.filter((a) => a.startsWith('--repo=')).map((a) => a.slice(7));
  const goal = (rest.find((a) => a.startsWith('--goal=')) || '--goal=').slice(7);
  if (!slug) { console.error('empty slug'); process.exit(1); }
  if (fs.existsSync(path.join(D, 'data', 'refuse-new'))) { console.error('herdr-projects: refusing to create ' + slug); process.exit(1); }
  const root = process.argv[process.argv.indexOf('--root') + 1];
  fs.mkdirSync(path.join(root, slug), { recursive: true });
  const toml = repos.map((r) => { const [p, m] = r.split(/@(?=[^/@]*$)/); return '[[repos]]\\npath = "' + p + '"\\n' + (m ? 'machine = "' + m + '"\\n' : ''); }).join('\\n');
  fs.writeFileSync(path.join(root, slug, 'PROJECT.md'), '+++\\nname = "' + name + '"\\ngoal = "' + goal + '"\\n' + toml + '+++\\n');
  fs.appendFileSync(path.join(D, 'state', 'created'), slug + '\\n');
  console.log('created \`' + slug + '\` in ' + root);
} else if (cmd === 'set') {
  log('set.jsonl', { argv: rest });
} else if (cmd === 'thread' && rest[0] === 'list') {
  const f = path.join(D, 'data', 'threads-' + rest[1] + '.json');
  console.log(fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '[]');
} else if (cmd === 'profile' && rest[0] === 'resolve') {
  const p = JSON.parse(fs.readFileSync(path.join(D, 'data', 'profiles.json'), 'utf8'))[rest[1]];
  if (!p) { console.error('unknown profile ' + rest[1]); process.exit(1); }
  console.log(JSON.stringify(p));
} else if (cmd === 'assignable') {
  console.log('me: the user\\nProfiles here: claude-sonnet, claude-opus, deepseek-flash\\n@mac: pi (1 profile)');
} else if (cmd === 'coordinator' && rest[0] === 'prompt') {
  const slug = rest[1]; const text = readStdin();
  log('prompts.jsonl', { slug, text, opened: opened(slug) });
  if (fs.existsSync(path.join(D, 'data', 'refuse-' + slug))) { console.error('the coordinator of ' + slug + ' refuses prompts'); process.exit(1); }
  if (!opened(slug)) { console.error('no coordinator of ' + slug + ' can take a prompt right now; \`open ' + slug + '\` starts one'); process.exit(1); }
} else if (cmd === 'open') {
  log('opens.jsonl', { slug: rest[0] });
  sleep(700);
  fs.writeFileSync(path.join(D, 'state', 'opened-' + rest[0]), '');
} else if (cmd === 'focus') {
  log('focus.jsonl', { slug: rest[0] });
} else { console.error('fixture herdr-projects: unsupported ' + argv.join(' ')); process.exit(2); }
`);
bin('herdr', `${prelude}
const argv = process.argv.slice(2);
if (argv[0] === 'workspace') { console.log(JSON.stringify({ result: { workspaces: [] } })); }
else if (argv[0] === 'machine' && argv[1] === 'list') {
  console.log(fs.existsSync(path.join(D, 'data', 'no-machines')) ? '[]' : JSON.stringify([{ label: 'mac', enabled: true }, { label: 'old', enabled: false }, { label: 'nas', enabled: true }]));
}
else if (argv[0] === 'worktree' && argv[1] === 'open') {
  const at = argv.indexOf('--path'); const p = at >= 0 ? argv[at + 1] : '';
  log('herdr.jsonl', { argv });
  if (!p || !fs.existsSync(p)) { console.error('no such worktree'); process.exit(1); }
} else { console.error('fixture herdr: unsupported ' + argv.join(' ')); process.exit(2); }
`);
bin('claude', `${prelude}
const argv = process.argv.slice(2); const prompt = readStdin();
const entry = { argv, prompt, cwd: process.cwd() };
if (prompt.includes('Задачи бэклога:')) {
  // The freshness check: the first answer says «частично» for the export task and «сделано» (with a file)
  // for the theme one; after a new commit the export task is no longer a worry.
  const f = path.join(D, 'state', 'fresh-count'); const n = (fs.existsSync(f) ? Number(fs.readFileSync(f, 'utf8')) : 0) + 1;
  fs.writeFileSync(f, String(n)); log('fresh.jsonl', { ...entry, n });
  if (fs.existsSync(path.join(D, 'data', 'fresh-fails'))) { console.error('the fake agent fails'); process.exit(1); }
  sleep(n === 1 ? 2500 : 600);
  const rows = [...prompt.matchAll(/^(\\d+)\\. (.*)$/gm)].map((m) => {
    const text = m[2]; const row = { n: Number(m[1]), v: 'actual', why: 'нужна' };
    if (/экспорт/.test(text)) Object.assign(row, n === 1 ? { v: 'partial', why: 'часть экспорта уже есть в export.txt' } : { v: 'actual', why: 'не сделано' });
    if (/тёмную/.test(text)) Object.assign(row, { v: 'done', why: 'тема уже есть', proof: 'export.txt' });
    if (/Единственная/.test(text)) Object.assign(row, { v: 'done', why: 'кажется, есть', proof: 'нет/такого/файла.rs' });
    return row;
  });
  console.log('Проверил по коду.\\n\`\`\`json\\n' + JSON.stringify(rows) + '\\n\`\`\`');
} else if (prompt.includes('Проекты (slug | название | цель):')) {
  log('groups.jsonl', entry); sleep(300);
  const slugs = prompt.split('Проекты (slug | название | цель):')[1].split('\\n').map((l) => l.split(' | ')[0].trim()).filter(Boolean);
  const group = (s) => (s === 'delta' ? 'Новая область' : s.startsWith('a') ? 'Omarchy' : s.startsWith('b') ? 'Личное' : 'ИИ-инфраструктура');
  console.log('Вот раскладка:\\n' + JSON.stringify(slugs.map((slug) => ({ slug, group: group(slug) }))));
} else {
  log('claude.jsonl', entry);
  sleep(1200);
  console.log('Ответ из теста: **готово**.\\n\\n- первый пункт\\n- второй пункт с \`кодом\`\\n\\n<script>alert(1)</script>');
}
`);
console.log(dir);
