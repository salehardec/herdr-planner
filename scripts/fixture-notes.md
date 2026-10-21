# The offline fixture: what `scripts/fixture.mjs` generates

`node scripts/fixture.mjs DIR` builds a fake world under DIR in about 0.2 s. `--scale 50x500`
adds 50 generated universes with 500 planets on top of it (about 0.3 s). Nothing touches a real
board. Timestamps (`created`, mtimes, rate-limit resets) are relative to the moment of the run;
ids, names and statuses are fixed.

## Environment the planner needs

`run-checks.sh` already sets `FIXTURE_DIR`, `HERDR_PROJECTS_BIN`, `HERDR_SOCKET_PATH`,
`OMARCHY_THEME_DIR`, `CLAUDE_CONFIG_DIR`, `PATH=DIR/bin:$PATH` and the XDG dirs. The harness
still has to add these (the script is not edited here):

| variable | value | why |
|---|---|---|
| `CODEX_HOME` | `DIR/codex` | **required**: codex sub-agents of `DEL-1` are read from here; without it the server reads the real `~/.codex` |
| `HERDR_PLANNER_SSH` | `DIR/bin/ssh` | recommended: the machines (`vm-a`, `vm-b`). The bare name `ssh` also resolves through `PATH` |
| `HERDR_PLANNER_CLAUDE` | `DIR/bin/claude` | recommended: translation. Also found through `PATH` |
| `HERDR_PLANNER_GH` | `DIR/bin/gh` | recommended: hosting checks (`acme/mine`, `acme/theirs`). Also found through `PATH` |
| `HERDR_PLANNER_GLAB` | `DIR/bin/glab` | recommended: the not-logged-in hint for gitlab.example.com. Also found through `PATH` |
| `HERDR_BIN` | `DIR/bin/herdr` | recommended: `herdr machine list --json` and `herdr agent list`. Also found through `PATH` |

The stand-ins also work when the env is missing, because the names resolve through `PATH=DIR/bin`.
The one that changes the answer is `CODEX_HOME`.

## The board (`DIR/board`, read by the server with `--data`)

- `repos.json` (name → folder) and `repo-keys.json` (`had`: key → name).
- `tasks/<repo>/<n>/task.json` for every row of `list --json` (22 with the base world; 522 with
  `--scale`), and `closing.md` for `alpha/15` (English, 7 lines), `alpha/13` (Russian, 4 lines)
  and `beta/1` (one line).
- `coordinator/default/state.json` (`run.kind` claude) and `chat/` (the transcript folder).

| repo | folder | git | key | notes |
|---|---|---|---|---|
| alpha | `projects/AI/alpha` | clone of `origins/alpha.git`; main two commits behind origin/main; FETCH_HEAD present | ALP | bridge with `vm-a` (same origin) |
| beta | `projects/AI/beta` | clone of `origins/beta.git`, level with origin | BET | quiet: done only |
| gamma | `projects/AI/gamma` | plain | GAM | |
| omega | `projects/AI/omega` | plain | OMG | **no tasks here**; vm-a has `OMG-1`, `OMG-2` |
| delta | `projects/herdr/delta` | plain | DEL | |
| sigma | `projects/herdr/sigma` | plain | SIG | |
| docs | `projects/herdr/docs` | plain | DOC | only archived work (`DOC-1`) |
| newrepo | `projects/herdr/newrepo` | git, origin `https://github.com/acme/mine.git`, not registered | (NWR when added) | the ready folder for the add-universe check |

The two groups (AI, herdr) give the galaxy two arms. `origins/` holds the bare repos.

### Planets (`herdr-projects list --json`, the same tags as before)

Live (17): `ALP-2` waiting (question) · `ALP-3` plan · `ALP-4` ready (pass, PR 12) · `ALP-5` waiting
(review failed) · `ALP-6` working, omp · `ALP-7` review, pi · `ALP-8` failed · `ALP-9` queued ·
`ALP-10` queued, waits for `ALP-9` · `ALP-11` queued · `ALP-16` waiting (permission) · `GAM-1` working,
pi, waits for `ALP-4` · `DEL-1` waiting (codex, checks fail) · `DEL-2`, `DEL-3` waiting (pick, 3 attempts) ·
`SIG-1` failed (no summary; no `worker.agent` setting) · `SIG-2` waiting, omp.
Done (3): `ALP-13`, `ALP-15`, `BET-1`. Archived (2): `GAM-2`, `DOC-1`.

Ages (`created`): `ALP-2` 25 min, `ALP-16` 12 min, `ALP-5` 50 min, `ALP-15` 4 h, `ALP-3` 2 h, `ALP-9` 2 h,
`ALP-6` 5 h, `DEL-1` 6 h, `SIG-1` 7 h (today); `ALP-10`, `GAM-1`, `DEL-2`, `SIG-2`, `ALP-7` yesterday;
`ALP-8`, `DEL-3`, `GAM-2` 3 days; `ALP-11`, `ALP-13`, `BET-1`, `DOC-1` 9 days.

### Waits

`alpha/10` waits for `alpha/9` (`ALP-9`) and `gamma/1` waits for `alpha/4` (`ALP-4`), as before.
`board.json` (the stand-ins' state) and `task.json` (`after`) say the same.

### Task records (`task.json`)

`worker.agent`: `claude/claude-sonnet-5-5` (claude tasks), `omp/deepseek/deepseek-v4-flash` (`ALP-6`,
`SIG-2`), `pi/default` (`ALP-7`, `GAM-1`), `codex/gpt-5-codex` (`DEL-1`); `SIG-1` has none.
`model` is `claude-sonnet-5-5` etc., null for pi. Descriptions are one paragraph each, some in
English, some in Russian. `exitedAt` is set for done and archived tasks only.

## Sub-agents

| task | program | what is there | expected state |
|---|---|---|---|
| `ALP-2` (waiting) | claude | `agent-a1f0c3e9d7b24e10` «Explore Rust backend», last record a tool call, mtime 30 s | running |
| `ALP-2` | claude | `agent-b2e8d5a4c9f31b07` «Проверить клиентов старой схемы», mtime 15 min, no finish | quiet |
| `ALP-2` | claude | `agent-c3d9e6b5daf42c18` «Сравнить схемы», finished, end_turn text (result) | done |
| `ALP-16` (waiting) | claude | `agent-d4aa7f6c0b53d229` «Найти старые ветки» | running |
| `ALP-6` (working) | omp | `Migration-Scan` (+ `.md`, session_exit) | done |
| `ALP-6` | omp | `Migration-Index`, no session_exit | running |
| `DEL-1` (waiting) | codex | rollout `…-bd35d760-…` with `parent_thread_id` = the parent | running |
| `SIG-2` | omp | parent only, no children | empty list |
| `ALP-7`, `GAM-1` | pi | no sub-agents at all | note «у pi нет субагентов» |
| Coordinator | claude | `agent-f6cc9b8e2d75f44b` under the transcript folder | **done** (see below) |

The Coordinator is idle, and the server calls a sub-agent of an idle parent `done`. To see it
running, create `DIR/state/coordinator-busy`: `herdr-projects coordinator` then reports
`working` and the sub-agent turns `running`.

## Coordinator

- Transcript `claude/projects/<dashed chat dir>/fixture.jsonl`: ten turns, the last one unanswered.
  The last assistant turn carries `message.model` `claude-opus-5-5` and `usage` 150000 tokens, so
  `coordinator.context` is 150000 of 200000, 75 %.
- `herdr-projects coordinator tell -- <text>` logs `coordinator.jsonl` (text, argv), and so does
  `tell -- /compact`. `coordinator new` logs `commands.jsonl` and prints `ok`.
- `state/coordinator-dialog` makes it blocked (dialog); `state/coordinator-busy` makes it working.

## Machines (`vm-a`, `vm-b`)
- `vm-a`'s own board (`herdr-projects show|set|unset|doctor|models`, `state/vm-a-settings.json`): its crew is
  `coordinator.agent codex/gpt-5-codex` (effort high) and `reviewer.agent claude/claude-opus-5-5`; its repo alpha
  sets `worker.effort xhigh`. `doctor` says claude, codex and omp are there and signed in (codex signed in; omp
  on API keys). A `for p in claude codex …; do command -v …` line prints the crew's programs on its PATH: claude,
  codex and omp. Home's PATH gets `bin/codex` and `bin/omp` stand-ins (claude is the translator stand-in).

- `herdr machine list --json` → `[{label vm-a, ssh_target deploy@vm-a, remote_session work},
  {label vm-b, ssh_target deploy@vm-b}]`.
- `herdr-projects machines --json` → vm-a ok (version 1.2.4, 4 tasks); vm-b not ok with the `why`
  of the spec («herdr-projects is not found there», code 127).
- `bin/ssh` answers only the board's own form (`ssh … deploy@vm-a 'exec "${SHELL:-/bin/sh}" -lc
  '…''`). The inner command for vm-a must start with `HERDR_SESSION=work `; each call logs
  `session_ok` in `log/ssh.jsonl`. vm-a prints a login banner, then the reply's line.
  - `herdr-projects remote list` → JSON of version, host, coordinator and tasks: `ALP-4` working,
    `ALP-5` blocked (waits for `ALP-4`), `OMG-1` working (omp), `OMG-2` done. Folders
    `/srv/projects/alpha` and `/srv/projects/omega`.
  - `remote do` / `remote dispatch` (base64 payload) → `log/remote.jsonl`, reply `{"ok":true,"msg":…}`.
  - `herdr-projects after <id> <refs>` → `ID: waits for TAG`, `--clear` → `waits for nothing`.
  - the gitlag snippet → TSV lines: vm-a's alpha is one commit behind its origin (same origin as
    the local clone), omega is level.
- `deploy@vm-b` → exit 255, `ssh: connect to host vm-b port 22: Connection refused`.
  (The machines list says «herdr-projects is not found»; the two are shown on different paths.)
- To link a local task to a far one: `herdr-projects after alpha/10 vm-a:OMG-1` (accepted, stored).

## Other stand-ins (`DIR/bin`)

- `herdr-projects`: `list [--all] [--json]`, `details`, `after`, `pick`, `approve|review|retry|
  delegate|stop|tell|archive|restore|fix|finish`, `coordinator [tell|new]`, `machines --json`,
  `models <prog>` (2 s; `claude`, `omp`, `codex`, `pi`; unknown → error), `show --repo NAME --json`
  (`{repo:{name,key,folder}, settings:[{name,value}…]}` — an array, which the server reads),
  `show --task`, `set <setting> <value> [--repo NAME]`, `unset <setting> [--repo NAME]` (no `--repo` = the home
  board). Settings live in `state/board-settings.json`: one level per name, `""` = home, a repo its own. `show --json`
  (home or `--repo`) gives the board's shape `{settings:[{name,kind,value,from,place,own,note}]}`: own values
  (`from: you|repo`), inherited ones (`own: false`) and built-in ones (`from: built in`). `set` refuses a `*.agent`
  the program does not take and an `*.effort` the program does not take, with the words of the board; `set` also
  writes `repo.key`. `doctor` prints one line per program (`✓ claude 2.1.303, signed in (…)`, `✗ codex …, not signed
  in`), exits 1 when a line is ✗ (as the real board does). `models <prog>` also prints an `Effort (…): low, medium…`
  paragraph (claude, codex, omp).
  `add TITLE --repo NAME|/abs/path [--worker.agent X
  --model M --app A --desc D]`. `add` with an absolute path registers a git folder (name = folder
  name, key from it) and makes `KEY-1`; the new task's `task.json` is written with `created` now.
  Commands are logged in `log/commands.jsonl` (`coordinator.jsonl`, `lists.jsonl`, `details.jsonl`…).
- `herdr`: `machine list --json`, `agent list`, `agent read`, `agent send-keys`.
- `claude`: translation. Reads `<<< … >>>` on stdin, prints `ПЕРЕВОД: ` and the first 60 characters
  (`…` when cut). Logs argv and whether `--tools` was empty to `log/claude.jsonl`.
- `gh`: `auth status` ok; `repo view acme/mine` → WRITE, `acme/theirs` → READ, others not found;
  `api user` → `fixture-user`.
- `glab`: `auth status --hostname gitlab.example.com` → not logged in (exit 1, the hint); `api` → 401.
- `xdg-terminal-exec`: logs `terminal.jsonl`.

## The scale world (`--scale 50x500`)

Adds universes `s01`…`s50` (keys `S01`…`S50`) in four folder groups (AI, herdr, work, lab), 10
planets each (500 in all), with `task.json` records and worktrees. 30 % of the universes (the ones whose number mod 10 is 0, 1 or 2: `s01`, `s02`, `s10`, `s11`, `s12`, …)
have only finished work. The other planets cycle through ten statuses (working,
queued, question, permission, plan, failed, idle, done, reviewing, pick). The state is deterministic
(a fixed seed), apart from `created`.

Totals after `--scale 50x500`: 522 rows (`list --all --json`), 56 universes; the page's
`/api/state` has 520 live+done planets, 57 projects, 18 quiet.

## Checks that depended on the old numbers

- `ui-check.mjs` expects `universes === 5 && quiet === 1`. The fixture now has 7 universes
  (`docs` and `omega` are empty) and 3 quiet ones (`beta`, `docs`, `omega`). Those numbers change.
- `ui-check.mjs` line 12 imports `../web/requests.js`, which this branch no longer has (the page is
  now `web/src/*.ts`). The check does not start on this branch. This predates the fixture.

## The far machine vm-c (`dev@vm-c`): adding a galaxy from SSH

A fresh machine, with nothing of herdr's on it. The ssh stand-in runs its commands with `/bin/sh -c`, with HOME =
`machines/vm-c/home` and PATH = `machines/vm-c/bin` (symlinks to the host's git, node, env, sh and the shell tools
the checks use) plus `home/.local/bin`, where what gets installed goes. So the probe sees what the machine has:
git and node (and npm, from node), no curl, no herdr, no plugin, no agents.

- `home/Projects`: `api`, `web` (no origin), `labs/tools` (two folders deep). All on `main`.
- `machines/vm-c/state.json`: `plugin` (version, ref), `board` (repos, tasks), `agents` (version, signedIn).
- `herdr machine list --json` reads `state/herdr-machines.json`, which starts as the saved machines; `machine
  add` and `machine remove` change it.
- `herdr machine add dev@vm-c --label vm-c` asks `[y/N]` only in a terminal (stdin must be a tty); `y` installs
  herdr on the machine and saves it. A machine already saved is a no-op. The real prompt text is unknown: the
  fixture's text is invented.
- Home `herdr plugin list`: `herdr-projects 1.2.4  eliasstravik/herdr-projects@7d8bba7c…`. On vm-c,
  `herdr plugin install eliasstravik/herdr-projects [--ref X] -y` installs herdr-projects 1.2.4 and records the ref.
- vm-c's `herdr-projects`: `version`, `install`, `doctor` (one line per installed agent: `✓ codex 0.5.1, signed in
  (fixture)` or `⚠ … not signed in`), `remote list`, `add TITLE --repo DIR` (the first task is `KEY-1`), `archive`,
  `show --repo N --json`, `set NAME VALUE --repo N`.
- `npm install -g <package>` on vm-c installs the program's command into `home/.local/bin` (codex, omp, pi,
  opencode; the packages are `DATA.packages`). `claude` needs curl, which vm-c lacks: its install says so.
- `<program> login` (`claude auth login` for claude) signs the agent in there. `xdg-terminal-exec` runs a login
  window's `ssh` call at once (there is no person to finish a browser login), so `agent-check` then says signed in.
- `log/far.jsonl`: each command vm-c's stand-ins got.
- The planner's probe and plugin step read the home board's `herdr-projects version` (`herdr-projects 1.2.4 …`).
  The home stand-in must answer `version`.
