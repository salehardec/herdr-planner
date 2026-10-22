//! How far a repository's main branch lags behind its origin (plan §4.8).
//!
//! Read-only: nothing here fetches or pulls. The board fetches on its own, and `FETCH_HEAD`'s mtime says when.

use serde::Serialize;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const LOCAL_WAIT: Duration = Duration::from_secs(20);

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

/// A repository's place on this machine: its name on the board and its folder.
pub type Folder = (String, String);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;

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

    /// A bare origin, a seed that pushes three commits and a `local` clone left at the first one (fetched later, so it is
    /// two behind).
    fn scenario(root: &Path) -> (PathBuf, PathBuf) {
        let origin = root.join("origin.git");
        let seed = root.join("seed");
        let local = root.join("local");
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
        (origin, local)
    }

    #[tokio::test]
    async fn local_clone_two_behind_after_a_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let (_origin, local) = scenario(dir.path());
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
}
