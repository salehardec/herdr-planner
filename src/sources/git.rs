//! A thread's git state: branch, commits over the base, diff size, work not
//! committed yet, and the `Co-Authored-By` trailers that name its models.
//! One `sh -c` per thread; `--no-optional-locks` keeps a read from blocking
//! the agent's own git. The scripts print tagged lines that `parse` reads.

use super::runner;
use serde::Serialize;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(20);

/// `$1` is the worktree, `$2` the base herdr recorded (origin/main); a base
/// that is gone falls back to the usual trunk names.
const HEAD: &str = r#"
P="$1"
B="$2"
g() { git -C "$P" --no-optional-locks "$@"; }
[ -d "$P" ] || { echo "ERR папки нет"; exit 0; }
g rev-parse --git-dir >/dev/null 2>&1 || { echo "ERR не git-каталог"; exit 0; }
if [ -z "$B" ] || ! g rev-parse --verify -q "$B^{commit}" >/dev/null 2>&1; then
	B=""
	for c in origin/main main origin/master master; do
		if g rev-parse --verify -q "$c^{commit}" >/dev/null 2>&1; then B="$c"; break; fi
	done
fi
echo "BASE $B"
"#;

const SUMMARY: &str = r#"
echo "BRANCH $(g rev-parse --abbrev-ref HEAD 2>/dev/null)"
echo "DIRTY $(g status --porcelain 2>/dev/null | wc -l)"
if [ -n "$B" ]; then
	echo "AHEAD $(g rev-list --count "$B..HEAD" 2>/dev/null)"
	g diff --numstat "$B...HEAD" 2>/dev/null | sed 's/^/NUM /'
	g log --reverse --format=%B "$B..HEAD" 2>/dev/null | grep -i '^co-authored-by:' | sed 's/^[^:]*: *//' | sed 's/^/CO /'
fi
"#;

const DETAIL: &str = r#"
if [ -n "$B" ]; then
	g diff --stat=100 "$B...HEAD" 2>/dev/null | sed 's/^/STAT /'
	g log -5 --format='%h%x09%s' "$B..HEAD" 2>/dev/null | sed 's/^/LOG /'
fi
g status --porcelain 2>/dev/null | head -8 | sed 's/^/WT /'
"#;

/// `$1` repo, `$2` the thread's branch, `$3` its base, `$4` its PR number.
/// Prints the trailers of the branch's commits while the branch exists, else
/// of the commit that landed the PR (a squash commit titled `... (#N)`, or a
/// merge commit of that PR).
const TRAILERS: &str = r#"
R="$1"
BR="$2"
B="$3"
N="$4"
g() { git -C "$R" --no-optional-locks "$@"; }
[ -d "$R" ] || { echo "ERR репозитория нет"; exit 0; }
g rev-parse --git-dir >/dev/null 2>&1 || { echo "ERR не git-каталог"; exit 0; }
co() { grep -i '^co-authored-by:' | sed 's/^[^:]*: *//' | sed 's/^/CO /'; }
if [ -z "$B" ] || ! g rev-parse --verify -q "$B^{commit}" >/dev/null 2>&1; then
	B=""
	for c in origin/main main origin/master master; do
		if g rev-parse --verify -q "$c^{commit}" >/dev/null 2>&1; then B="$c"; break; fi
	done
fi
if [ -n "$BR" ] && [ -n "$B" ] && g rev-parse --verify -q "refs/heads/$BR^{commit}" >/dev/null 2>&1; then
	out=$(g log --reverse --format=%B "$B..$BR" 2>/dev/null | co)
	if [ -n "$out" ]; then echo "SRC ветка"; echo "$out"; exit 0; fi
fi
if [ -n "$N" ] && [ -n "$B" ]; then
	h=$(g log -n 5000 --format='%H%x09%s' "$B" 2>/dev/null | awk -F'\t' -v n="(#$N)" '{ s=$2; if (substr(s, length(s)-length(n)+1) == n) { print $1; exit } }')
	if [ -n "$h" ]; then
		out=$(g show -s --format=%B "$h" 2>/dev/null | co)
		if [ -n "$out" ]; then echo "SRC слитый коммит"; echo "$out"; exit 0; fi
	fi
	h=$(g log -n 5000 --merges --format='%H%x09%s' "$B" 2>/dev/null | awk -F'\t' -v n="Merge pull request #$N " 'index($2, n) == 1 { print $1; exit }')
	if [ -n "$h" ]; then
		out=$(g log --reverse --format=%B "$h^1..$h^2" 2>/dev/null | co)
		if [ -n "$out" ]; then echo "SRC слитая ветка"; echo "$out"; exit 0; fi
	fi
fi
echo "NONE"
"#;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
}

/// What the scripts printed. Which fields are filled depends on the script.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GitInfo {
    /// Why git could not be read (no folder, not a repository, timeout).
    pub err: Option<String>,
    pub base: Option<String>,
    pub branch: Option<String>,
    /// Files with changes not committed yet.
    pub dirty: u32,
    /// Commits over the base.
    pub ahead: Option<u32>,
    pub files: u32,
    pub add: u32,
    pub del: u32,
    /// `git diff --stat` lines; the last one is the total.
    pub stat: Vec<String>,
    pub log: Vec<Commit>,
    /// `git status --porcelain` lines (first 8).
    pub wt: Vec<String>,
    /// `Co-Authored-By` trailer values, in order of appearance.
    pub co: Vec<String>,
    /// Where the trailers came from (`ветка`, `слитый коммит`, …).
    pub src: Option<String>,
}

impl GitInfo {
    pub fn error(text: impl Into<String>) -> GitInfo {
        GitInfo {
            err: Some(text.into()),
            ..Default::default()
        }
    }
}

/// Reads either script's tagged lines.
pub fn parse(out: &str) -> GitInfo {
    let mut r = GitInfo::default();
    for line in out.lines() {
        let tag_len = line.chars().take_while(|c| c.is_ascii_uppercase()).count();
        if tag_len == 0 {
            continue;
        }
        let (tag, rest) = line.split_at(tag_len);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let number = |s: &str| {
            let digits: String = s
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<u32>().ok()
        };
        match tag {
            "ERR" => r.err = some(rest),
            "BASE" => r.base = some(rest),
            "BRANCH" => r.branch = some(rest),
            "DIRTY" => r.dirty = number(rest).unwrap_or(0),
            "AHEAD" => r.ahead = number(rest),
            "NUM" => {
                let mut cols = rest.split('\t');
                let (a, d) = (cols.next().unwrap_or(""), cols.next().unwrap_or(""));
                r.files += 1;
                // Binary files show "-" for both counts.
                r.add += a.parse::<u32>().unwrap_or(0);
                r.del += d.parse::<u32>().unwrap_or(0);
            }
            "STAT" => r.stat.push(rest.trim_start().to_string()),
            "LOG" => {
                if let Some((hash, subject)) = rest.split_once('\t') {
                    r.log.push(Commit {
                        hash: hash.to_string(),
                        subject: subject.to_string(),
                    });
                }
            }
            "WT" => r.wt.push(rest.to_string()),
            "CO" => r.co.push(rest.to_string()),
            "SRC" => r.src = some(rest),
            _ => {}
        }
    }
    r
}

/// Runs a script; an error text when the process failed or timed out.
async fn run(script: String, args: &[&str]) -> Result<GitInfo, String> {
    let mut argv = runner::argv(["sh", "-c", script.as_str(), "sh"]);
    argv.extend(args.iter().map(|a| a.to_string()));
    match runner::run(&argv, None, TIMEOUT).await {
        Ok(out) if out.ok() => Ok(parse(&out.stdout)),
        Ok(out) => Err(super::tasks::clip(
            &out.error().split_whitespace().collect::<Vec<_>>().join(" "),
            80,
        )),
        Err(_) => Err("git не ответил".into()),
    }
}

/// Branch, commits over the base, diff size, uncommitted files, trailers.
pub async fn summary(path: &str, base: &str) -> GitInfo {
    run(format!("{HEAD}{SUMMARY}"), &[path, base])
        .await
        .unwrap_or_else(GitInfo::error)
}

/// The summary plus the diff stat, the last commits and the files not committed.
pub async fn detail(path: &str, base: &str) -> GitInfo {
    run(format!("{HEAD}{SUMMARY}{DETAIL}"), &[path, base])
        .await
        .unwrap_or_else(GitInfo::error)
}

/// The trailers of a finished thread's commits, read from the repository.
pub async fn trailers(
    repo: &str,
    branch: &str,
    base: &str,
    pr: Option<u32>,
) -> Result<GitInfo, String> {
    let pr = pr.map(|n| n.to_string()).unwrap_or_default();
    run(TRAILERS.to_string(), &[repo, branch, base, &pr]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_summary() {
        let out = "BASE origin/main\nBRANCH hp/p/t-0001\nDIRTY       3\nAHEAD 2\nNUM 10\t2\tsrc/a.rs\nNUM -\t-\timg.png\nNUM 5\t0\tREADME.md\nCO Claude Sonnet 5.5 <noreply@anthropic.com>\nCO Anna <a@b.c>\n";
        let g = parse(out);
        assert_eq!(g.base.as_deref(), Some("origin/main"));
        assert_eq!(g.branch.as_deref(), Some("hp/p/t-0001"));
        assert_eq!(g.dirty, 3);
        assert_eq!(g.ahead, Some(2));
        assert_eq!((g.files, g.add, g.del), (3, 15, 2));
        assert_eq!(g.co.len(), 2);
        assert_eq!(g.err, None);
    }

    #[test]
    fn parses_the_detail_and_errors() {
        let g = parse("BASE main\nSTAT  src/a.rs | 12 ++--\nSTAT  1 file changed\nLOG abc123\tFix the thing\nWT ?? new.txt\nSRC слитый коммит\nNONE\n");
        assert_eq!(g.stat, vec!["src/a.rs | 12 ++--", "1 file changed"]);
        assert_eq!(g.log[0].hash, "abc123");
        assert_eq!(g.log[0].subject, "Fix the thing");
        assert_eq!(g.wt, vec!["?? new.txt"]);
        assert_eq!(g.src.as_deref(), Some("слитый коммит"));
        let e = parse("ERR папки нет\n");
        assert_eq!(e.err.as_deref(), Some("папки нет"));
        assert_eq!(parse("BASE \n").base, None);
    }

    /// The real scripts against a fixture repository with a worktree.
    #[tokio::test]
    async fn reads_a_fixture_repo() {
        let sh = |dir: &std::path::Path, cmd: &str| {
            let ok = std::process::Command::new("sh")
                .current_dir(dir)
                .arg("-c")
                .arg(cmd)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success();
            assert!(ok, "{cmd}");
        };
        let root = std::env::temp_dir().join(format!("hp-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(
            &repo,
            "git init -q -b main && echo a > a.txt && git add . && git commit -qm first",
        );
        sh(
            &repo,
            &format!("git worktree add -q -b feat {}", root.join("wt").display()),
        );
        let wt = root.join("wt");
        sh(
            &wt,
            "echo b > b.txt && git add . && git commit -qm 'second' -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>' && echo dirty > c.txt",
        );
        let g = summary(wt.to_str().unwrap(), "origin/main").await;
        // origin/main does not exist: the base falls back to main.
        assert_eq!(g.err, None);
        assert_eq!(g.base.as_deref(), Some("main"));
        assert_eq!(g.branch.as_deref(), Some("feat"));
        assert_eq!(g.ahead, Some(1));
        assert_eq!((g.files, g.add, g.del), (1, 1, 0));
        assert_eq!(g.dirty, 1);
        assert_eq!(g.co, vec!["Claude Opus 5.5 <noreply@anthropic.com>"]);
        let d = detail(wt.to_str().unwrap(), "main").await;
        assert_eq!(d.log.len(), 1);
        assert_eq!(d.log[0].subject, "second");
        assert_eq!(d.wt, vec!["?? c.txt"]);
        assert!(d.stat.last().unwrap().contains("1 file changed"));
        let t = trailers(repo.to_str().unwrap(), "feat", "", None)
            .await
            .unwrap();
        assert_eq!(t.src.as_deref(), Some("ветка"));
        assert_eq!(t.co.len(), 1);
        let none = trailers(repo.to_str().unwrap(), "gone", "main", Some(7))
            .await
            .unwrap();
        assert!(none.co.is_empty());
        let missing = summary(root.join("nope").to_str().unwrap(), "").await;
        assert_eq!(missing.err.as_deref(), Some("папки нет"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
