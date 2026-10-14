//! `herdr-projects list --all`: one tab-separated line per project,
//! `slug<TAB>status<TAB>summary`, where the summary reads
//! `Ready for review: 3, Resolved: 116` or `no threads`.

use crate::model::{Counts, Project};

pub fn parse_list(stdout: &str) -> Vec<Project> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 || cols[0].is_empty() {
            continue;
        }
        let counts = parse_summary(cols.get(2).copied().unwrap_or(""));
        out.push(Project {
            slug: cols[0].to_string(),
            name: cols[0].to_string(),
            status: cols[1].to_string(),
            counts,
            ..Project::default()
        });
    }
    out
}

/// `Key: N` pairs; unknown keys are ignored.
pub fn parse_summary(summary: &str) -> Counts {
    let mut c = Counts::default();
    for part in summary.split(',') {
        let Some((key, n)) = part.rsplit_once(':') else {
            continue;
        };
        let Ok(v) = n.trim().parse::<u32>() else {
            continue;
        };
        match key.trim().to_lowercase().as_str() {
            "ready for review" => c.review = v,
            "working" => c.working = v,
            "resolved" => c.resolved = v,
            "blocked" => c.blocked = v,
            "needs you" | "waiting on you" => c.needs = v,
            _ => {}
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_and_counts() {
        let text = "apicache\tactive\tReady for review: 2, Resolved: 202\nditis\tactive\tno threads\npaused-one\tpaused\tWorking: 1, Blocked: 2, Waiting on you: 3\n\nbad line\n";
        let p = parse_list(text);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].slug, "apicache");
        assert_eq!(p[0].counts.review, 2);
        assert_eq!(p[0].counts.resolved, 202);
        assert_eq!(p[1].counts, Counts::default());
        assert_eq!(p[2].status, "paused");
        assert_eq!(p[2].counts.working, 1);
        assert_eq!(p[2].counts.blocked, 2);
        assert_eq!(p[2].counts.needs, 3);
    }
}
