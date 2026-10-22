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
| `HERDR_PLANNER_CLAUDE` | `DIR/bin/claude` | recommended: translation. Also found through `PATH` |
| `HERDR_PLANNER_GH` | `DIR/bin/gh` | recommended: hosting checks (`acme/mine`, `acme/theirs`). Also found through `PATH` |
| `HERDR_PLANNER_GLAB` | `DIR/bin/glab` | recommended: the not-logged-in hint for gitlab.example.com. Also found through `PATH` |
| `HERDR_BIN` | `DIR/bin/herdr` | recommended: `herdr agent list`. Also found through `PATH` |

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
| alpha | `projects/AI/alpha` | clone of `origins/alpha.git`; main two commits behind origin/main; FETCH_HEAD present | ALP | |
| beta | `projects/AI/beta` | clone of `origins/beta.git`, level with origin | BET | quiet: done only |
| gamma | `projects/AI/gamma` | plain | GAM | |
| omega | `projects/AI/omega` | plain | OMG | **no tasks here** |
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
- Commands for the `/` list: `claude/skills/fixture-skill/SKILL.md` («Навык фикстуры…»),
  `claude/commands/fixture-cmd.md` («Команда фикстуры») and `board/coordinator/default/chat/.claude/commands/here-cmd.md`
  («Команда папки Координатора»). Claude Code's built-in commands are the planner's own table (`src/hints.rs`); the
  `@` files are the repositories' (`projects/AI/alpha/README.md` and `projects/AI/beta/README.md` are the shortest).

## Machines the board knows (`vm-a`, `vm-b`)
The planner works with its own board only. The stand-ins still answer as a board with other machines configured
(`board.machines`), and the page checks assert that nothing of them reaches the page:
- `herdr machine list --json` → `[{label vm-a, ssh_target deploy@vm-a, remote_session work},
  {label vm-b, ssh_target deploy@vm-b}]`.
- `herdr-projects machines --json` → vm-a ok (version 1.2.4, 4 tasks); vm-b not ok («herdr-projects is not found
  there», code 127).
- To link a local task to one of another machine: `herdr-projects after alpha/10 vm-a:OMG-1` (accepted, stored).
  The page lists such a link in the task's links and does not draw it.

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
- `herdr`: `machine list --json`, `agent list`, `agent get`, `agent read`, `agent send-keys`, `pane send-text`. `agent read` prints what
  `state/dialog.txt` holds (or a default question); with `--format ansi` every line comes coloured, as a
  terminal would show it. `alpha/8` carries `exitedAt` (its agent has left) for the screen that must stop
  reading it; `state/dialog.txt` with many lines is the long screen the window must keep the end of.
  Keys go to `log/keys.jsonl`. `agent get NAME` gives the pane `w1:p-NAME`, and `pane send-text PANE TEXT`
  logs the words to `log/text.jsonl` (the own answer of a selection menu). A selection menu is a screen written
  to `state/dialog.txt` as Claude Code / omp draw it (see the section `menu card` of `ui-check.mjs`).
- `claude`: translation. Reads `<<< … >>>` on stdin, prints `ПЕРЕВОД: ` and the first 60 characters
  (`…` when cut). Logs argv and whether `--tools` was empty to `log/claude.jsonl`.
- `gh`: `auth status` ok; `repo view acme/mine` → WRITE, `acme/theirs` → READ, others not found;
  `api user` → `fixture-user`.
- `glab`: `auth status --hostname gitlab.example.com` → not logged in (exit 1, the hint); `api` → 401.
- `xdg-terminal-exec`: logs `terminal.jsonl`.
- `nvidia-smi`: a shell script that prints `DIR/load/nvidia.csv` (the lines the real one prints with
  `--query-gpu=name,utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits`) and, with no such file,
  fails like a machine without the driver (exit 9). The load bars' own checks write and remove that file.
  `HERDR_PLANNER_LOAD_FILE=DIR/load/load.json` (set by `run-checks.sh`, together with `HERDR_PLANNER_LOAD_MS=400`)
  replaces the planner's reading of `/proc`: a JSON `Load` (`{ok, cpu:{percent,cores}, mem:{used,total,percent},
  gpus:[…], at}`); `{"ok": false}` is a machine that cannot be read; no file means the real machine.

## The shipyard vm-r (`me@192.0.2.29`)

`192.0.2.0/24` is a documentation address: no real host. The ssh stand-in answers for it with one fixed probe
(`herdr-planner-runner 1`), the disk count (`herdr-planner-disk 1`: `docker system df`, `docker system df -v`,
`docker buildx ls`), `ssh -G`, `true`, `docker version`, and the removals the planner may ask for: `docker rm -f -v` and
`docker rm -v` by id, `docker network rm`, `docker volume rm`, `docker buildx rm` (a running builder is refused),
`docker image prune -a -f`, `docker builder prune -a -f`.

- `settings/runners.json` (next to `board/`, where the planner reads it): one yard `vm-r`, target `me@192.0.2.29`,
  ports on `192.0.2.29`, tags `docker28 gpu x86_64`, masks `buildx_*` and `neighbor` (the compose project next door).
- `state/runner.json`: ids, networks and volume names already removed, builders removed, whether the images and the cache
  were pruned; the probe leaves them out. Its `extra` (`containers`, `networks`, `volumes`, in the shape of
  `DATA.runnerR`) is what a check adds to the yard: the `runners auto` section writes it (a container, a network and a
  volume labelled `herdr.task=ALP-13`, and live ones that must stay). Writing the file anew brings the yard back.
- Six containers: `pg-ALP-3` (a live task, compose project `work-3`), `pg-AL9-4` (the same task alpha/4 under the
  repository's older key `AL9`, from `repo-keys.json` `had`), `web-ALP-15` (task ALP-15 is done → forgotten),
  `x-ALP-99` (tag with no such task → unsure), `neighbor-db-1` (compose project `neighbor`) and
  `buildx_buildkit_x0` (protected by mask). Two networks and two volumes belong to the first and the neighbour project.
- The disk card's yard: four images (two without containers, «2.0GB» reclaimable), a build cache of 2.4 GB, two buildx
  builders (`old`, stopped, state volume 3.1 GB; `x`, running), an orphan builder state `buildx_buildkit_gone0_state`
  (8 GB) and one unused volume of somebody else (`chroma_data`, compose project `chroma`).
- `log/ssh.jsonl` gets a line with `runner: "vm-r"` for every such call. No copy of the list goes to another machine.
- The wizard's local check: `ssh -G me@192.0.2.29` prints its `hostname`, `bin/docker` exists, and the stand-in ssh
  answers `true` and `docker version` with `28.3.0`.

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
