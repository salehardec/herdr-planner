//! How far a repository's main branch lags behind its origin (plan §4.8).
//!
//! Read-only: nothing here fetches or pulls. The board fetches on its own, and `FETCH_HEAD`'s mtime says when.
//! The same read-only snippet runs here (`sh -c`) and on another machine (one ssh call per machine, over the folders
//! of its repositories), so both sides read the same facts.

use crate::machines::{self, sh_quote, MachineRef};
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const LOCAL_WAIT: Duration = Duration::from_secs(20);
const REMOTE_WAIT: Duration = Duration::from_secs(25);

/// One repository's main branch against its origin. `behind`/`ahead` count commits; `as_of_ms` is when the
/// origin was last fetched (`FETCH_HEAD`'s mtime), `tip_ms` the time of the origin's main tip commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Lag {
    /// The origin with no scheme, user or `.git`: `github.com/owner/repo`.
    pub origin: String,
    /// The main branch's name (`main` or `master`).
    pub main: String,
    /// The local main branch's commit.
    pub head_sha: String,
    /// `origin/<main>`'s commit; empty when the origin was never fetched here.
    pub origin_sha: String,
    pub behind: u32,
    pub ahead: u32,
    pub as_of_ms: u64,
    pub tip_ms: u64,
    pub known: bool,
    pub note: String,
}

/// A repository's place on another (or this) machine: its name on the board and its folder.
pub type Folder = (String, String);

/// The result of comparing two clones of one origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Cmp {
    /// Both main branches are the same commit.
    Same,
    /// The first clone is behind the second by this many commits.
    Behind(u32),
    /// The first clone is ahead of the second by this many commits.
    Ahead(u32),
    /// Each has commits the other lacks.
    Diverged,
    /// Not computable: a clone never fetched its origin, or the local clone lacks one of the commits. The page
    /// words this as "впереди или расходится".
    Unknown,
}

/// A repository of one galaxy, with its origin as its folder reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GalaxyRepo {
    pub galaxy: String,
    pub repo: String,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoRef {
    pub galaxy: String,
    pub repo: String,
}

/// Two clones of one origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bridge {
    pub origin: String,
    pub a: RepoRef,
    pub b: RepoRef,
}

/// The read-only snippet. Its arguments are folders; each prints one tab-separated line:
/// index, origin URL, main branch, local main sha, origin/main sha, ahead, behind, FETCH_HEAD mtime, tip time.
/// A folder that is not a git repository prints `index NOGIT`.
const SNIPPET: &str = r#"i=0
for d in "$@"; do
  i=$((i + 1))
  if ! git -C "$d" rev-parse --git-dir >/dev/null 2>&1; then printf '%s\tNOGIT\n' "$i"; continue; fi
  o=$(git -C "$d" remote get-url origin 2>/dev/null)
  m=$(git -C "$d" symbolic-ref -q refs/remotes/origin/HEAD 2>/dev/null)
  m=${m#refs/remotes/origin/}
  if [ -z "$m" ]; then
    for c in main master; do
      if git -C "$d" rev-parse --verify -q "refs/remotes/origin/$c" >/dev/null 2>&1; then m=$c; break; fi
    done
  fi
  [ -n "$m" ] || m=main
  h=$(git -C "$d" rev-parse --verify -q "refs/heads/$m" 2>/dev/null)
  s=$(git -C "$d" rev-parse --verify -q "refs/remotes/origin/$m" 2>/dev/null)
  l=$(git -C "$d" rev-list --left-right --count "refs/heads/$m...refs/remotes/origin/$m" 2>/dev/null | cut -f1)
  r=$(git -C "$d" rev-list --left-right --count "refs/heads/$m...refs/remotes/origin/$m" 2>/dev/null | cut -f2)
  t=$(git -C "$d" log -1 --format=%ct "refs/remotes/origin/$m" 2>/dev/null)
  g=$(git -C "$d" rev-parse --absolute-git-dir 2>/dev/null)
  f=$(stat -c %Y "$g/FETCH_HEAD" 2>/dev/null || stat -f %m "$g/FETCH_HEAD" 2>/dev/null)
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$i" "$o" "$m" "$h" "$s" "$l" "$r" "$f" "$t"
done
"#;

/// The origin as one comparable string: no scheme, no user or password, no port, no `.git`, lowercase.
/// `git@github.com:Owner/repo.git`, `https://github.com/owner/repo.git` and `ssh://git@github.com/owner/repo` are one.
pub fn normalize_origin(url: &str) -> String {
    let u = url.trim();
    let (host, path) = if let Some((_, rest)) = u.split_once("://") {
        let (auth, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = auth.rsplit('@').next().unwrap_or(auth);
        let host = host.split(':').next().unwrap_or(host);
        (host.to_string(), path.to_string())
    } else if let Some((h, p)) = u.split_once(':').filter(|(h, _)| !h.contains('/')) {
        // scp form: user@host:owner/repo
        (h.rsplit('@').next().unwrap_or(h).to_string(), p.to_string())
    } else {
        (String::new(), u.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    let joined = if host.is_empty() {
        path.to_string()
    } else {
        format!("{host}/{path}")
    };
    joined.to_lowercase()
}

fn unknown(note: &str) -> Lag {
    Lag {
        note: note.to_string(),
        ..Lag::default()
    }
}

/// Reads the snippet's lines. `keys` are the arguments in order; the lines are keyed by them.
fn parse_lines(stdout: &str, keys: &[String]) -> BTreeMap<String, Lag> {
    let mut out = BTreeMap::new();
    for line in stdout.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 2 {
            continue;
        }
        let Some(key) = f[0]
            .parse::<usize>()
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| keys.get(i))
        else {
            continue;
        };
        if f[1] == "NOGIT" {
            out.insert(key.clone(), unknown("не git-репозиторий"));
            continue;
        }
        if f.len() != 9 {
            continue;
        }
        let origin = if f[1].is_empty() {
            String::new()
        } else {
            normalize_origin(f[1])
        };
        let ahead = f[5].parse::<u32>();
        let behind = f[6].parse::<u32>();
        let known = !f[4].is_empty() && ahead.is_ok() && behind.is_ok();
        let note = if origin.is_empty() {
            "нет origin: сравнивать не с чем".to_string()
        } else if f[4].is_empty() {
            format!("origin/{} ещё не получали: данных нет", f[2])
        } else {
            String::new()
        };
        out.insert(
            key.clone(),
            Lag {
                origin,
                main: f[2].to_string(),
                head_sha: f[3].to_string(),
                origin_sha: f[4].to_string(),
                ahead: ahead.unwrap_or(0),
                behind: behind.unwrap_or(0),
                as_of_ms: f[7].parse::<u64>().unwrap_or(0) * 1000,
                tip_ms: f[8].parse::<u64>().unwrap_or(0) * 1000,
                known,
                note,
            },
        );
    }
    out
}

/// Runs the snippet over unique folders and gives each (name, folder) its lag.
fn by_folders(stdout: Option<&str>, folders: &[Folder], note: &str) -> BTreeMap<String, Lag> {
    let mut unique: Vec<String> = Vec::new();
    for (_, f) in folders {
        if !unique.contains(f) {
            unique.push(f.clone());
        }
    }
    let got = match stdout {
        Some(s) => parse_lines(s, &unique),
        None => BTreeMap::new(),
    };
    folders
        .iter()
        .map(|(name, f)| {
            let lag = got.get(f).cloned().unwrap_or_else(|| unknown(note));
            (name.clone(), lag)
        })
        .collect()
}

/// The lags of this machine's repositories (`name`, `root`).
pub async fn local(repos: &[Folder]) -> BTreeMap<String, Lag> {
    if repos.is_empty() {
        return BTreeMap::new();
    }
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(SNIPPET)
        .arg("sh")
        .args(repos.iter().map(|(_, root)| root))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match timeout(LOCAL_WAIT, cmd.output()).await {
        Ok(Ok(out)) => by_folders(
            Some(&String::from_utf8_lossy(&out.stdout)),
            repos,
            "git не ответил",
        ),
        _ => by_folders(None, repos, "git не запустился"),
    }
}

/// The lags of the repositories of another machine: one ssh call, the same snippet over the folders there.
/// `label` names the machine as the planner has it (`machines::resolve`).
pub async fn remote(
    cfg: &crate::config::AppConfig,
    label: &str,
    folders: &[Folder],
) -> BTreeMap<String, Lag> {
    remote_with(cfg, &machines::herdr_bin(), label, folders).await
}

pub(crate) async fn remote_with(
    cfg: &crate::config::AppConfig,
    herdr: &str,
    label: &str,
    folders: &[Folder],
) -> BTreeMap<String, Lag> {
    if folders.is_empty() {
        return BTreeMap::new();
    }
    let m: MachineRef = match machines::resolve_with(herdr, label).await {
        Ok(m) => m,
        Err(e) => return by_folders(None, folders, &e),
    };
    let mut unique: Vec<String> = Vec::new();
    for (_, f) in folders {
        if !unique.contains(f) {
            unique.push(f.clone());
        }
    }
    let args: Vec<String> = unique.iter().map(|f| sh_quote(f)).collect();
    let cmd = format!("sh -c {} sh {}", sh_quote(SNIPPET), args.join(" "));
    match machines::ssh_ended(cfg, &m, &cmd, REMOTE_WAIT).await {
        Ok(end) if !end.timed_out && end.error.is_empty() => {
            by_folders(Some(&end.stdout), folders, "машина не ответила по git")
        }
        _ => by_folders(None, folders, "машина не ответила по git"),
    }
}

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The commits in `range` (`a..b`) of the clone at `root`, when that clone has them all.
fn rev_count(root: &str, range: &str) -> Option<u32> {
    let out = StdCommand::new("git")
        .args(["-C", root, "rev-list", "--count", range])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// How clone `a` stands against clone `b` of one origin. Equal main commits are `Same`; otherwise the commit counts
/// come from the local clone at `local_root` when it has both commits. Otherwise `Unknown`.
pub fn compare(a: &Lag, b: &Lag, local_root: Option<&str>) -> Cmp {
    if !a.known || !b.known || !is_sha(&a.head_sha) || !is_sha(&b.head_sha) {
        return Cmp::Unknown;
    }
    if a.head_sha == b.head_sha {
        return Cmp::Same;
    }
    let Some(root) = local_root else {
        return Cmp::Unknown;
    };
    let only_b = rev_count(root, &format!("{}..{}", a.head_sha, b.head_sha));
    let only_a = rev_count(root, &format!("{}..{}", b.head_sha, a.head_sha));
    match (only_a, only_b) {
        (Some(0), Some(0)) => Cmp::Same,
        (Some(0), Some(n)) => Cmp::Behind(n),
        (Some(n), Some(0)) => Cmp::Ahead(n),
        (Some(_), Some(_)) => Cmp::Diverged,
        _ => Cmp::Unknown,
    }
}

/// Pairs of repositories, of any galaxies, that share an origin (after normalizing). Each pair once.
pub fn bridges(repos: &[GalaxyRepo]) -> Vec<Bridge> {
    let mut groups: BTreeMap<String, Vec<&GalaxyRepo>> = BTreeMap::new();
    for r in repos {
        let origin = normalize_origin(&r.origin);
        if !origin.is_empty() {
            groups.entry(origin).or_default().push(r);
        }
    }
    let mut out = Vec::new();
    for (origin, members) in groups {
        for (i, a) in members.iter().enumerate() {
            for b in &members[i + 1..] {
                if a.galaxy == b.galaxy && a.repo == b.repo {
                    continue;
                }
                out.push(Bridge {
                    origin: origin.clone(),
                    a: RepoRef {
                        galaxy: a.galaxy.clone(),
                        repo: a.repo.clone(),
                    },
                    b: RepoRef {
                        galaxy: b.galaxy.clone(),
                        repo: b.repo.clone(),
                    },
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::path::{Path, PathBuf};

    #[test]
    fn origins_are_one_string_however_they_are_written() {
        let want = "github.com/salehardec/herdr-planner";
        assert_eq!(
            normalize_origin("https://github.com/Salehardec/herdr-planner.git"),
            want
        );
        assert_eq!(
            normalize_origin("git@github.com:salehardec/herdr-planner.git"),
            want
        );
        assert_eq!(
            normalize_origin("ssh://git@github.com/salehardec/herdr-planner"),
            want
        );
        assert_eq!(
            normalize_origin("https://github.com/salehardec/herdr-planner/"),
            want
        );
        assert_eq!(
            normalize_origin("https://user:tok@gitlab.example.ru/team/proj.git"),
            "gitlab.example.ru/team/proj"
        );
        assert_eq!(
            normalize_origin("ssh://git@gitlab.example.ru:2222/team/proj.git/"),
            "gitlab.example.ru/team/proj"
        );
        assert_eq!(
            normalize_origin("git@gitlab.local:Team/Proj"),
            "gitlab.local/team/proj"
        );
        assert_eq!(normalize_origin(""), "");
    }

    #[test]
    fn snippet_lines_become_lags() {
        let keys = vec![
            "herdr-planner".to_string(),
            "scratch".to_string(),
            "lost".to_string(),
        ];
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let out = format!(
            "Welcome banner\n1\tgit@github.com:salehardec/herdr-planner.git\tmain\t{sha_a}\t{sha_b}\t0\t2\t1700000000\t1700000500\n2\tNOGIT\n3\t\tmaster\t{sha_a}\t\t\t\t\t\n"
        );
        let map = parse_lines(&out, &keys);
        let hp = &map["herdr-planner"];
        assert!(hp.known);
        assert_eq!((hp.ahead, hp.behind), (0, 2));
        assert_eq!(hp.origin, "github.com/salehardec/herdr-planner");
        assert_eq!(hp.as_of_ms, 1_700_000_000_000);
        assert_eq!(hp.tip_ms, 1_700_000_500_000);
        assert!(!map["scratch"].known);
        assert_eq!(map["scratch"].note, "не git-репозиторий");
        assert!(!map["lost"].known);
        // "lost" has no origin at all: nothing to compare with.
        assert!(map["lost"].note.contains("нет origin"), "{:?}", map["lost"]);
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.org",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git");
        assert!(st.success(), "git {args:?}");
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", file]);
    }

    /// A bare origin, a seed that pushes three commits, a `local` clone left at the first one (fetched later, so it is
    /// two behind), and an `other` clone made after all three (up to date).
    fn scenario(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let origin = root.join("origin.git");
        let seed = root.join("seed");
        let local = root.join("local");
        let other = root.join("other");
        std::fs::create_dir_all(&seed).unwrap();
        let st = StdCommand::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&origin)
            .status()
            .unwrap();
        assert!(st.success());
        git(&seed, &["init", "-q", "-b", "main"]);
        commit(&seed, "one");
        git(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&seed, &["push", "-q", "origin", "main"]);
        let clone = |to: &Path| {
            let st = StdCommand::new("git")
                .args(["clone", "-q"])
                .arg(&origin)
                .arg(to)
                .status()
                .unwrap();
            assert!(st.success());
        };
        clone(&local);
        commit(&seed, "two");
        commit(&seed, "three");
        git(&seed, &["push", "-q", "origin", "main"]);
        git(&local, &["fetch", "-q", "origin"]);
        clone(&other);
        (origin, local, other)
    }

    #[tokio::test]
    async fn local_clone_two_behind_after_a_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let (_origin, local, _other) = scenario(dir.path());
        let lags = local_for(&local).await;
        let lag = &lags["hp"];
        assert!(lag.known, "{lag:?}");
        assert_eq!(lag.main, "main");
        assert_eq!((lag.ahead, lag.behind), (0, 2));
        assert_ne!(lag.head_sha, lag.origin_sha);
        assert!(lag.as_of_ms > 0);
    }

    async fn local_for(root: &Path) -> BTreeMap<String, Lag> {
        local(&[("hp".to_string(), root.display().to_string())]).await
    }

    #[tokio::test]
    async fn clones_compare_by_commits_in_the_local_clone() {
        let dir = tempfile::tempdir().unwrap();
        let (_origin, local_clone, other) = scenario(dir.path());
        let behind = local_for(&local_clone).await.remove("hp").unwrap();
        let up = local_for(&other).await.remove("hp").unwrap();
        let other_root = other.display().to_string();
        assert_eq!(compare(&behind, &up, Some(&other_root)), Cmp::Behind(2));
        assert_eq!(compare(&up, &behind, Some(&other_root)), Cmp::Ahead(2));
        assert_eq!(compare(&up, &up.clone(), None), Cmp::Same);
        // Without the commits in the local clone, the answer is not known.
        assert_eq!(compare(&behind, &up, None), Cmp::Unknown);
        let never = Lag::default();
        assert_eq!(compare(&never, &up, Some(&other_root)), Cmp::Unknown);
    }

    #[tokio::test]
    async fn remote_reads_the_same_snippet_over_the_one_ssh_call() {
        let dir = tempfile::tempdir().unwrap();
        let (_origin, local_clone, _other) = scenario(dir.path());
        let cfg = machines::test_cfg(dir.path());
        let ssh = dir.path().join("ssh");
        // A stand-in for ssh: runs the remote command (the last argument) in a shell here.
        std::fs::write(
            &ssh,
            "#!/bin/sh\nfor a; do last=$a; done\nexec sh -c \"$last\"\n",
        )
        .unwrap();
        make_executable(&ssh);
        let herdr = dir.path().join("herdr");
        std::fs::write(
            &herdr,
            "#!/bin/sh\necho '[{\"label\":\"vm-a\",\"ssh_target\":\"vm-a\",\"remote_session\":\"default\"}]'\n",
        )
        .unwrap();
        make_executable(&herdr);
        let cfg = AppConfig {
            ssh_bin: ssh.display().to_string(),
            ..cfg
        };
        let folders = vec![("hp".to_string(), local_clone.display().to_string())];
        let lags = remote_with(&cfg, &herdr.display().to_string(), "vm-a", &folders).await;
        let lag = &lags["hp"];
        assert!(lag.known, "{lag:?}");
        assert_eq!(lag.behind, 2);
        // An unknown machine says so for each repository.
        let none = remote_with(&cfg, &herdr.display().to_string(), "vm-zz", &folders).await;
        assert!(!none["hp"].known);
        assert!(!none["hp"].note.is_empty());
    }

    fn make_executable(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::testutil::settle(p);
    }

    #[test]
    fn two_clones_of_one_origin_make_one_bridge() {
        let repos = vec![
            GalaxyRepo {
                galaxy: "home".into(),
                repo: "herdr-planner".into(),
                origin: "git@github.com:salehardec/herdr-planner.git".into(),
            },
            GalaxyRepo {
                galaxy: "vm-a".into(),
                repo: "herdr-planner".into(),
                origin: "https://github.com/salehardec/herdr-planner".into(),
            },
            GalaxyRepo {
                galaxy: "vm-a".into(),
                repo: "other".into(),
                origin: "https://github.com/salehardec/other".into(),
            },
            GalaxyRepo {
                galaxy: "home".into(),
                repo: "nothing".into(),
                origin: String::new(),
            },
        ];
        let got = bridges(&repos);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].origin, "github.com/salehardec/herdr-planner");
        assert_eq!(got[0].a.galaxy, "home");
        assert_eq!(got[0].b.galaxy, "vm-a");
    }
}
