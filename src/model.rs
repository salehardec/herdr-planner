//! The project list's pure logic: rows, sorting, chips, text filter, groups
//! and the workspace-to-project match. Ported from the Tern plugin 0.17.1.

// Reference logic: the list's sorting, chips and sections are mirrored by the page and used here by tests only.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub review: u32,
    pub working: u32,
    pub resolved: u32,
    pub blocked: u32,
    pub needs: u32,
}

impl Counts {
    pub fn total(&self) -> u32 {
        self.review + self.working + self.resolved + self.blocked + self.needs
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub path: String,
    #[serde(default)]
    pub machine: String,
}

/// One thing that needs the user in a project: the coordinator's question,
/// a thread on a prompt, a broken thread. See `attention`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wait {
    /// `coordinator`, `blocked`, `failed`, `stuck`, `lost`, `asking` or `unknown`.
    pub kind: String,
    /// The thread, empty for the coordinator.
    pub thread: String,
    /// In words: «координатор ждёт ответа», «разрешение или вопрос в t-0231».
    pub text: String,
    /// When it began, unix milliseconds; 0 when unknown.
    pub since: u64,
}

/// The waits in words, for a tooltip.
pub fn wait_hint(waits: &[Wait]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for w in waits {
        if !seen.contains(&w.text.as_str()) {
            seen.push(&w.text);
        }
    }
    seen.join("; ")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    /// `active`, `paused` or `archived`.
    pub status: String,
    pub goal: Option<String>,
    pub repos: Vec<Repo>,
    pub counts: Counts,
    /// Items in `inbox/` other than `done`.
    pub inbox: u32,
    /// The highest thread number: the recency signal for the "new" sort.
    pub activity: u32,
    /// What needs the user (their count is `counts.needs`).
    #[serde(default)]
    pub waits: Vec<Wait>,
    /// When the newest event that needs a look arrived (a coordinator's question, a thread
    /// that stopped on a prompt or finished), unix milliseconds; 0 when nothing does.
    #[serde(default)]
    pub attention_since: u64,
    /// Threads whose pull/merge request is merged or closed: they can be closed. Quiet.
    #[serde(default)]
    pub can_close: u32,
    /// Threads in review whose report is older than the cut-off: forgotten, not counted as review.
    #[serde(default)]
    pub forgotten: u32,
    /// A quiet note about the coordinator (its status says «waits», the question was not recognized).
    #[serde(default)]
    pub coordinator_note: String,
}

impl Project {
    pub fn needs_attention(&self) -> bool {
        self.counts.needs > 0 || self.counts.review > 0 || self.inbox > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    New,
    Attention,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::New => "новые",
            Sort::Attention => "внимание",
        }
    }
}

pub fn sort_projects(projects: &mut [Project], mode: Sort) {
    match mode {
        Sort::Attention => projects.sort_by(|a, b| {
            b.needs_attention()
                .cmp(&a.needs_attention())
                .then(b.attention_since.cmp(&a.attention_since))
                .then(b.counts.review.cmp(&a.counts.review))
                .then(b.inbox.cmp(&a.inbox))
                .then((b.status == "active").cmp(&(a.status == "active")))
                .then(b.counts.resolved.cmp(&a.counts.resolved))
        }),
        // "new": the project holding the newest thread leads, oldest threads last.
        Sort::New => projects.sort_by(|a, b| {
            b.activity
                .cmp(&a.activity)
                .then(b.counts.review.cmp(&a.counts.review))
                .then(a.slug.cmp(&b.slug))
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chip {
    All,
    Review,
    Inbox,
    Idle,
}

pub const CHIPS: [Chip; 4] = [Chip::All, Chip::Review, Chip::Inbox, Chip::Idle];

impl Chip {
    pub fn label(self) -> &'static str {
        match self {
            Chip::All => "Все",
            Chip::Review => "Ревью",
            Chip::Inbox => "Входящие",
            Chip::Idle => "Пауза",
        }
    }

    pub fn matches(self, p: &Project) -> bool {
        match self {
            Chip::All => true,
            Chip::Review => p.counts.review > 0,
            Chip::Inbox => p.inbox > 0,
            Chip::Idle => p.status != "active",
        }
    }
}

/// Projects per chip, over the whole list (the numbers do not move while typing).
pub fn chip_counts(projects: &[Project]) -> BTreeMap<String, usize> {
    CHIPS
        .iter()
        .map(|c| {
            (
                format!("{c:?}").to_lowercase(),
                projects.iter().filter(|p| c.matches(p)).count(),
            )
        })
        .collect()
}

/// Case-insensitive match of every word of `needle` somewhere in `haystack`.
pub fn text_matches(haystack: &str, needle: &str) -> bool {
    let hay = haystack.to_lowercase();
    needle
        .split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

pub fn visible_projects(projects: &[Project], chip: Chip, query: &str, sort: Sort) -> Vec<Project> {
    let mut out: Vec<Project> = projects
        .iter()
        .filter(|p| {
            chip.matches(p)
                && text_matches(
                    &format!("{} {} {}", p.name, p.slug, p.goal.as_deref().unwrap_or("")),
                    query,
                )
        })
        .cloned()
        .collect();
    sort_projects(&mut out, sort);
    out
}

// ---------------------------------------------------------------------------
// Groups: projects collected by area. `Groups` is the saved form.

pub const OTHER: &str = "Прочее";
pub const MAX_GROUPS: usize = 24;
pub const GROUP_NAME_MAX: usize = 40;
pub const DEFAULT_GROUPS: [&str; 5] = [
    "Omarchy",
    "ЯНАО",
    "ИИ-инфраструктура",
    "Агенты и herdr",
    "Личное",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Assign {
    /// The group name, "" for "no group".
    pub g: String,
    /// True when the cheap agent guessed it; a manual pick is final.
    #[serde(default)]
    pub auto: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Groups {
    pub configured: bool,
    pub list: Vec<String>,
    pub assign: BTreeMap<String, Assign>,
    pub collapsed: Vec<String>,
    /// The user's grouping switch; unset, grouping is on when any project has a group.
    pub on: Option<bool>,
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// A usable group name: control characters and `=` dropped, whitespace
/// collapsed, cut to the limit; none for empty or for «Прочее» (reserved).
pub fn group_name_clean(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_control() {
                ' '
            } else if c == '=' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(GROUP_NAME_MAX).collect();
    let s = s.trim().to_string();
    if s.is_empty() || lower(&s) == lower(OTHER) {
        return None;
    }
    Some(s)
}

/// Why `group_name_clean` refused `raw`.
fn name_error(raw: &str) -> String {
    if lower(raw.trim()) == lower(OTHER) {
        "«Прочее» занято: туда попадают проекты без группы".into()
    } else {
        "введите название группы".into()
    }
}

impl Groups {
    pub fn index(&self, name: &str) -> Option<usize> {
        let key = lower(name);
        self.list.iter().position(|g| lower(g) == key)
    }

    /// Saved groups, cleaned: no duplicates, no assignment to a group that is
    /// gone (a manual one becomes a manual "no group", an auto one is forgotten).
    pub fn normalize(raw: Option<Groups>) -> Groups {
        let mut out = Groups {
            configured: true,
            ..Default::default()
        };
        match raw {
            Some(src) if src.configured || !src.list.is_empty() => {
                for n in &src.list {
                    if let Some(name) = group_name_clean(n) {
                        if out.index(&name).is_none() && out.list.len() < MAX_GROUPS {
                            out.list.push(name);
                        }
                    }
                }
                for (slug, e) in src.assign {
                    if e.g.is_empty() {
                        out.assign.insert(
                            slug,
                            Assign {
                                g: String::new(),
                                auto: e.auto,
                            },
                        );
                    } else if let Some(at) = out.index(&e.g) {
                        out.assign.insert(
                            slug,
                            Assign {
                                g: out.list[at].clone(),
                                auto: e.auto,
                            },
                        );
                    } else if !e.auto {
                        out.assign.insert(
                            slug,
                            Assign {
                                g: String::new(),
                                auto: false,
                            },
                        );
                    }
                }
                for n in src.collapsed {
                    if n == OTHER {
                        out.collapsed.push(n);
                    } else if let Some(at) = out.index(&n) {
                        let canon = out.list[at].clone();
                        if !out.collapsed.contains(&canon) {
                            out.collapsed.push(canon);
                        }
                    }
                }
                out.collapsed.sort();
                out.on = src.on;
            }
            _ => out.list = DEFAULT_GROUPS.iter().map(|s| s.to_string()).collect(),
        }
        out
    }

    pub fn has_groups(&self) -> bool {
        self.assign.values().any(|e| !e.g.is_empty())
    }

    /// Grouping is on when the user said so, else when any project has a group.
    pub fn grouping_on(&self) -> bool {
        self.on.unwrap_or_else(|| self.has_groups())
    }

    /// The group a project is shown in (OTHER for none) and whether that is the agent's guess.
    pub fn group_of(&self, slug: &str) -> (String, bool) {
        match self.assign.get(slug) {
            Some(e) if !e.g.is_empty() && self.index(&e.g).is_some() => (e.g.clone(), e.auto),
            Some(e) => (OTHER.to_string(), e.auto),
            None => (OTHER.to_string(), false),
        }
    }

    /// Puts a project in a group ("" = no group). A manual pick is final.
    pub fn assign(&mut self, slug: &str, name: &str, auto: bool) {
        self.assign.insert(
            slug.to_string(),
            Assign {
                g: name.to_string(),
                auto,
            },
        );
    }

    /// A new group at the end, or the existing one of that name (any case).
    pub fn add(&mut self, raw: &str) -> Result<String, String> {
        let Some(name) = group_name_clean(raw) else {
            return Err(name_error(raw));
        };
        if let Some(at) = self.index(&name) {
            return Ok(self.list[at].clone());
        }
        if self.list.len() >= MAX_GROUPS {
            return Err("слишком много групп".into());
        }
        self.list.push(name.clone());
        Ok(name)
    }

    pub fn is_collapsed(&self, name: &str) -> bool {
        self.collapsed.iter().any(|c| c == name)
    }

    /// Folds or unfolds a group; `folded` none toggles.
    pub fn fold(&mut self, name: &str, folded: Option<bool>) {
        if name != OTHER && self.index(name).is_none() {
            return;
        }
        let want = folded.unwrap_or(!self.is_collapsed(name));
        self.collapsed.retain(|c| c != name);
        if want {
            self.collapsed.push(name.to_string());
            self.collapsed.sort();
        }
    }

    pub fn toggle_grouping(&mut self) {
        self.on = Some(!self.grouping_on());
    }

    /// Renames a group; its projects and its fold state follow.
    pub fn rename(&mut self, name: &str, raw: &str) -> Result<String, String> {
        let Some(at) = self.index(name) else {
            return Err("нет такой группы".into());
        };
        let Some(new) = group_name_clean(raw) else {
            return Err(name_error(raw));
        };
        if self.index(&new).is_some_and(|other| other != at) {
            return Err("такая группа уже есть".into());
        }
        let old = std::mem::replace(&mut self.list[at], new.clone());
        for e in self.assign.values_mut() {
            if e.g == old {
                e.g = new.clone();
            }
        }
        if self.is_collapsed(&old) {
            self.collapsed.retain(|c| *c != old);
            self.collapsed.push(new.clone());
            self.collapsed.sort();
        }
        Ok(new)
    }

    /// Moves a group by `delta` places; false when it cannot move that far.
    pub fn move_group(&mut self, name: &str, delta: i32) -> bool {
        let Some(at) = self.index(name) else {
            return false;
        };
        let to = at as i64 + delta as i64;
        if to < 0 || to >= self.list.len() as i64 {
            return false;
        }
        self.list.swap(at, to as usize);
        true
    }

    /// Removes a group. Without `target` its projects go to «Прочее» (a manual
    /// choice stays manual, an agent's guess stays a guess that «Разложить
    /// заново» can redo); with the name of another group they move there as a
    /// manual choice.
    pub fn delete(&mut self, name: &str, target: Option<&str>) -> bool {
        let Some(at) = self.index(name) else {
            return false;
        };
        let old = self.list.remove(at);
        let target = target
            .filter(|t| !t.is_empty())
            .and_then(|t| self.index(t))
            .map(|i| self.list[i].clone());
        self.collapsed.retain(|c| *c != old);
        for e in self.assign.values_mut() {
            if e.g == old {
                match &target {
                    Some(t) => {
                        e.g = t.clone();
                        e.auto = false;
                    }
                    None => e.g.clear(),
                }
            }
        }
        true
    }

    /// Forgets the agent's guesses (manual picks stay).
    pub fn forget_auto(&mut self) {
        self.assign.retain(|_, e| !e.auto);
    }

    /// Projects without a group (candidates for the cheap agent's suggestion).
    pub fn unassigned<'a>(&self, projects: &'a [Project]) -> Vec<&'a Project> {
        projects
            .iter()
            .filter(|p| !self.assign.contains_key(&p.slug))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub name: String,
    pub other: bool,
    pub collapsed: bool,
    pub projects: Vec<Project>,
    pub auto: Vec<bool>,
    pub needs: u32,
    pub review: u32,
    pub inbox: u32,
}

/// Projects split into the groups' sections, in the list's group order,
/// «Прочее» last. Empty groups show only while nothing filters the list.
pub fn group_sections(groups: &Groups, visible: &[Project], show_empty: bool) -> Vec<Section> {
    let mut by: BTreeMap<String, Vec<(Project, bool)>> = BTreeMap::new();
    for p in visible {
        let (g, auto) = groups.group_of(&p.slug);
        by.entry(g).or_default().push((p.clone(), auto));
    }
    let mut out = Vec::new();
    let mut add = |name: &str, other: bool| {
        let rows = by.remove(name).unwrap_or_default();
        if rows.is_empty() && !(show_empty && !other) {
            return;
        }
        out.push(Section {
            name: name.to_string(),
            other,
            collapsed: groups.is_collapsed(name),
            needs: rows.iter().map(|(p, _)| p.counts.needs).sum(),
            review: rows.iter().map(|(p, _)| p.counts.review).sum(),
            inbox: rows.iter().map(|(p, _)| p.inbox).sum(),
            auto: rows.iter().map(|(_, a)| *a).collect(),
            projects: rows.into_iter().map(|(p, _)| p).collect(),
        });
    };
    for name in &groups.list {
        add(name, false);
    }
    add(OTHER, true);
    out
}

// ---------------------------------------------------------------------------
// The active herdr workspace

/// The slug `herdr-projects new NAME` would make: lowercase ASCII words joined by `-`.
pub fn slug_preview(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Slug of the project a workspace label names, or none. A display name wins;
/// two projects with one name are told apart by slug, else the label stays unmatched.
pub fn project_for_label(projects: &[Project], label: &str) -> Option<String> {
    let key = lower(label.trim());
    let named: Vec<&Project> = projects
        .iter()
        .filter(|p| lower(p.name.trim()) == key)
        .collect();
    if named.len() == 1 {
        return Some(named[0].slug.clone());
    }
    let preview = slug_preview(label);
    if let Some(p) = named.iter().find(|p| p.slug == preview) {
        return Some(p.slug.clone());
    }
    if named.len() > 1 {
        return None;
    }
    projects
        .iter()
        .find(|p| p.slug == key)
        .map(|p| p.slug.clone())
}

/// What the shell icon shows: attention totals over the projects that are not archived.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Things waiting on the user (see `attention`).
    pub waiting: u32,
    /// Threads ready for review.
    pub review: u32,
    /// Items in the projects' `inbox/`.
    pub inbox: u32,
    /// Projects counted (not archived).
    pub projects: u32,
    /// When the newest event that needs a look arrived (ms); 0 when none does.
    pub attention_since: u64,
    /// What waits, in words, the newest first: «Alpha: координатор ждёт ответа; …».
    pub waiting_hint: String,
}

/// Projects that need something, the newest event first.
fn by_arrival(a: &Project, b: &Project) -> std::cmp::Ordering {
    b.attention_since
        .cmp(&a.attention_since)
        .then(b.counts.needs.cmp(&a.counts.needs))
        .then(b.counts.review.cmp(&a.counts.review))
        .then(b.activity.cmp(&a.activity))
        .then(a.slug.cmp(&b.slug))
}

pub fn summarize(projects: &[Project]) -> Summary {
    let mut s = Summary::default();
    let mut waiting: Vec<&Project> = Vec::new();
    for p in projects.iter().filter(|p| p.status != "archived") {
        s.waiting += p.counts.needs;
        s.review += p.counts.review;
        s.inbox += p.inbox;
        s.projects += 1;
        s.attention_since = s.attention_since.max(p.attention_since);
        if p.counts.needs > 0 {
            waiting.push(p);
        }
    }
    waiting.sort_by(|a, b| by_arrival(a, b));
    let mut lines: Vec<String> = waiting
        .iter()
        .take(5)
        .map(|p| format!("{}: {}", p.name, wait_hint(&p.waits)))
        .collect();
    if waiting.len() > 5 {
        lines.push(format!("…и ещё {}", waiting.len() - 5));
    }
    s.waiting_hint = lines.join("\n");
    s
}

/// One project row of the bar popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverviewProject {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub waiting: u32,
    pub review: u32,
    pub working: u32,
    pub blocked: u32,
    pub resolved: u32,
    pub inbox: u32,
    /// What waits, in words (empty when nothing does).
    pub wait_hint: String,
    /// When the newest event that needs a look arrived (ms); 0 when none does.
    pub attention_since: u64,
}

impl OverviewProject {
    fn of(p: &Project) -> Self {
        OverviewProject {
            slug: p.slug.clone(),
            name: p.name.clone(),
            status: p.status.clone(),
            waiting: p.counts.needs,
            review: p.counts.review,
            working: p.counts.working,
            blocked: p.counts.blocked,
            resolved: p.counts.resolved,
            inbox: p.inbox,
            wait_hint: wait_hint(&p.waits),
            attention_since: p.attention_since,
        }
    }
}

/// What the bar popup lists: who waits on the user (the newest first), who has
/// something to review (the newest first), who works; the projects where nothing
/// happens are only counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub summary: Summary,
    pub waiting: Vec<OverviewProject>,
    pub review: Vec<OverviewProject>,
    pub working: Vec<OverviewProject>,
    /// Projects with no waiting, review, working or stuck thread.
    pub quiet: u32,
}

pub fn overview(projects: &[Project]) -> Overview {
    let live: Vec<&Project> = projects.iter().filter(|p| p.status != "archived").collect();
    let mut waiting: Vec<&Project> = live
        .iter()
        .copied()
        .filter(|p| p.counts.needs > 0)
        .collect();
    let mut review: Vec<&Project> = live
        .iter()
        .copied()
        .filter(|p| p.counts.needs == 0 && p.counts.review > 0)
        .collect();
    let mut working: Vec<&Project> = live
        .iter()
        .copied()
        .filter(|p| {
            p.counts.needs == 0
                && p.counts.review == 0
                && (p.counts.working > 0 || p.counts.blocked > 0)
        })
        .collect();
    let quiet = live.len() - waiting.len() - review.len() - working.len();
    waiting.sort_by(|a, b| by_arrival(a, b));
    review.sort_by(|a, b| by_arrival(a, b));
    working.sort_by(|a, b| {
        b.counts
            .working
            .cmp(&a.counts.working)
            .then(b.activity.cmp(&a.activity))
            .then(a.slug.cmp(&b.slug))
    });
    let rows = |l: Vec<&Project>| l.into_iter().map(OverviewProject::of).collect();
    Overview {
        summary: summarize(projects),
        waiting: rows(waiting),
        review: rows(review),
        working: rows(working),
        quiet: quiet as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(slug: &str, review: u32, inbox: u32, activity: u32, status: &str) -> Project {
        Project {
            slug: slug.into(),
            name: slug.to_uppercase(),
            status: status.into(),
            counts: Counts {
                review,
                resolved: activity,
                ..Default::default()
            },
            inbox,
            activity,
            ..Default::default()
        }
    }

    #[test]
    fn sorts_new_and_attention() {
        let mut v = vec![
            p("a", 0, 0, 5, "active"),
            p("b", 2, 0, 3, "active"),
            p("c", 0, 1, 9, "paused"),
        ];
        sort_projects(&mut v, Sort::New);
        assert_eq!(
            v.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        sort_projects(&mut v, Sort::Attention);
        assert_eq!(
            v.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            ["b", "c", "a"]
        );
    }

    #[test]
    fn summary_skips_archived() {
        let mut v = vec![
            p("a", 2, 1, 5, "active"),
            p("b", 1, 0, 3, "paused"),
            p("c", 9, 9, 9, "archived"),
        ];
        v[0].counts.needs = 3;
        v[2].counts.needs = 7;
        let s = summarize(&v);
        assert_eq!((s.waiting, s.review, s.inbox, s.projects), (3, 3, 1, 2));
        assert_eq!(summarize(&[]), Summary::default());
    }

    fn at(mut p: Project, needs: u32, since: u64) -> Project {
        p.counts.needs = needs;
        p.attention_since = since;
        if needs > 0 {
            p.waits = vec![Wait {
                kind: "blocked".into(),
                thread: "t-1".into(),
                text: format!("запрос в {}", p.slug),
                since,
            }];
        }
        p
    }

    #[test]
    fn overview_sorts_by_arrival_and_counts_the_quiet() {
        let mut busy = p("busy", 0, 0, 7, "active");
        busy.counts.working = 2;
        let mut stuck = p("stuck", 0, 0, 2, "active");
        stuck.counts.blocked = 1;
        let v = vec![
            p("quiet", 0, 0, 5, "active"),
            at(p("old-review", 2, 0, 3, "active"), 0, 1000),
            at(p("wait-old", 0, 0, 1, "active"), 1, 2000),
            at(p("wait-new", 0, 0, 1, "active"), 2, 9000),
            at(p("new-review", 1, 0, 1, "active"), 0, 5000),
            p("gone", 4, 0, 9, "archived"),
            p("paused", 0, 0, 1, "paused"),
            busy,
            stuck,
            // Both waits and reviews: listed once, with the waits.
            at(p("both", 3, 0, 4, "active"), 1, 4000),
        ];
        let o = overview(&v);
        let slugs = |l: &[OverviewProject]| l.iter().map(|p| p.slug.clone()).collect::<Vec<_>>();
        assert_eq!(
            slugs(&o.waiting),
            ["wait-new", "both", "wait-old"],
            "the newest first"
        );
        assert_eq!(
            slugs(&o.review),
            ["new-review", "old-review"],
            "the newest first"
        );
        assert_eq!(slugs(&o.working), ["busy", "stuck"]);
        assert_eq!(o.quiet, 2, "quiet and paused; archived is not counted");
        assert_eq!(o.summary.waiting, 4);
        assert_eq!(o.summary.attention_since, 9000);
        assert_eq!(o.waiting[0].wait_hint, "запрос в wait-new");
        assert_eq!(o.waiting[0].attention_since, 9000);
        // Everything quiet: nothing is listed.
        let o = overview(&[p("a", 0, 0, 1, "active"), p("b", 0, 0, 2, "active")]);
        assert!(o.waiting.is_empty() && o.review.is_empty() && o.working.is_empty());
        assert_eq!(o.quiet, 2);
    }

    #[test]
    fn summary_hint_names_what_waits_newest_first() {
        let v = vec![
            at(p("a", 0, 0, 1, "active"), 1, 1000),
            at(p("b", 0, 0, 2, "active"), 1, 3000),
            p("c", 0, 0, 3, "active"),
        ];
        let s = summarize(&v);
        assert_eq!(s.waiting_hint, "B: запрос в b\nA: запрос в a");
        assert_eq!(summarize(&[p("c", 0, 0, 3, "active")]).waiting_hint, "");
        let mut many: Vec<Project> = (0..7)
            .map(|i| at(p(&format!("p{i}"), 0, 0, 1, "active"), 1, 100 + i))
            .collect();
        many.push(p("z", 0, 0, 1, "active"));
        assert!(summarize(&many).waiting_hint.ends_with("…и ещё 2"));
    }

    #[test]
    fn chips_and_filter() {
        let v = vec![
            p("a", 0, 0, 5, "active"),
            p("b", 2, 0, 3, "active"),
            p("c", 0, 1, 9, "paused"),
        ];
        let n = chip_counts(&v);
        assert_eq!(n["all"], 3);
        assert_eq!(n["review"], 1);
        assert_eq!(n["inbox"], 1);
        assert_eq!(n["idle"], 1);
        assert_eq!(visible_projects(&v, Chip::Review, "", Sort::New).len(), 1);
        assert_eq!(visible_projects(&v, Chip::All, "C", Sort::New)[0].slug, "c");
        assert!(text_matches("Планировщик проектов", "план проект"));
        assert!(!text_matches("abc", "abd"));
    }

    #[test]
    fn groups_normalize_and_layout() {
        let mut raw = Groups {
            configured: true,
            list: vec![
                "Omarchy".into(),
                "omarchy".into(),
                "Прочее".into(),
                "X".into(),
            ],
            ..Default::default()
        };
        raw.assign.insert(
            "a".into(),
            Assign {
                g: "omarchy".into(),
                auto: true,
            },
        );
        raw.assign.insert(
            "b".into(),
            Assign {
                g: "Gone".into(),
                auto: false,
            },
        );
        raw.assign.insert(
            "c".into(),
            Assign {
                g: "Gone".into(),
                auto: true,
            },
        );
        raw.collapsed = vec!["x".into(), "Прочее".into(), "Nope".into()];
        let g = Groups::normalize(Some(raw));
        assert_eq!(g.list, vec!["Omarchy", "X"]);
        assert_eq!(g.group_of("a"), ("Omarchy".to_string(), true));
        assert_eq!(g.group_of("b"), (OTHER.to_string(), false));
        assert_eq!(g.assign.get("c"), None);
        assert_eq!(g.collapsed, vec!["X".to_string(), "Прочее".to_string()]);
        assert!(g.grouping_on());

        let v = vec![
            p("a", 1, 0, 1, "active"),
            p("b", 0, 2, 2, "active"),
            p("z", 0, 0, 3, "active"),
        ];
        let s = group_sections(&g, &v, true);
        assert_eq!(
            s.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Omarchy", "X", OTHER]
        );
        assert_eq!(s[0].review, 1);
        assert!(s[1].collapsed && s[1].projects.is_empty());
        assert_eq!(s[2].inbox, 2);
        assert_eq!(s[2].projects.len(), 2);
        assert_eq!(group_sections(&g, &v, false).len(), 2);
    }

    #[test]
    fn defaults_without_saved_groups() {
        let g = Groups::normalize(None);
        assert_eq!(g.list.len(), 5);
        assert!(!g.grouping_on());
        let mut g2 = g.clone();
        g2.toggle_grouping();
        assert_eq!(g2.on, Some(true));
        assert_eq!(
            g2.add(" Прочее ").unwrap_err(),
            "«Прочее» занято: туда попадают проекты без группы"
        );
        assert_eq!(g2.add("  New=Group\n ").unwrap(), "New-Group");
        assert_eq!(g2.add("new-group").unwrap(), "New-Group");
        g2.fold("New-Group", None);
        assert!(g2.is_collapsed("New-Group"));
        g2.fold("New-Group", Some(false));
        assert!(!g2.is_collapsed("New-Group"));
    }

    #[test]
    fn groups_rename_move_delete() {
        let mut g = Groups::normalize(None);
        g.assign("a", "Omarchy", false);
        g.assign("b", "Omarchy", true);
        g.assign("c", "ЯНАО", false);
        g.fold("Omarchy", Some(true));
        assert_eq!(
            g.rename("Omarchy", " Мой  Omarchy ").unwrap(),
            "Мой Omarchy"
        );
        assert_eq!(g.group_of("a").0, "Мой Omarchy");
        assert!(g.is_collapsed("Мой Omarchy") && !g.is_collapsed("Omarchy"));
        assert_eq!(g.rename("Мой Omarchy", "яна").unwrap(), "яна");
        assert!(g.rename("яна", "ЯНАО").is_err(), "a name that is taken");
        assert_eq!(
            g.rename("яна", "Прочее").unwrap_err(),
            "«Прочее» занято: туда попадают проекты без группы"
        );
        assert_eq!(g.rename("нет", "x").unwrap_err(), "нет такой группы");
        assert_eq!(
            g.rename("яна", "Omarchy").unwrap(),
            "Omarchy",
            "the old name is free again"
        );
        // move
        assert!(g.move_group("Omarchy", 1));
        assert_eq!(g.list[..2], ["ЯНАО".to_string(), "Omarchy".to_string()]);
        assert!(!g.move_group("ЯНАО", -1), "the first one cannot go up");
        assert!(!g.move_group("Личное", 1), "the last one cannot go down");
        assert!(g.move_group("Omarchy", -1));
        // delete: to «Прочее», then to another group
        assert!(g.delete("ЯНАО", None));
        assert_eq!(g.group_of("c").0, OTHER);
        assert_eq!(
            g.assign["c"],
            Assign {
                g: String::new(),
                auto: false
            }
        );
        g.assign("d", "Личное", true);
        assert!(g.delete("Личное", Some("Omarchy")));
        assert_eq!(
            g.assign["d"],
            Assign {
                g: "Omarchy".into(),
                auto: false
            }
        );
        assert!(g.delete("Omarchy", Some("Omarchy")), "itself is no target");
        assert_eq!(g.group_of("a").0, OTHER);
        assert!(!g.delete("Omarchy", None));
        g.assign("e", "", true);
        g.assign("f", "", false);
        g.forget_auto();
        assert!(!g.assign.contains_key("e") && g.assign.contains_key("f"));
    }

    #[test]
    fn matches_workspace_label() {
        let mut v = vec![
            p("apicache", 0, 0, 1, "active"),
            p("other", 0, 0, 1, "active"),
            p("dup-a", 0, 0, 1, "active"),
            p("dup-b", 0, 0, 1, "active"),
        ];
        v[0].name = "APICache".into();
        v[2].name = "Dup".into();
        v[3].name = "Dup".into();
        assert_eq!(
            project_for_label(&v, "apicache").as_deref(),
            Some("apicache")
        );
        assert_eq!(project_for_label(&v, "other").as_deref(), Some("other"));
        assert_eq!(project_for_label(&v, "Dup"), None);
        assert_eq!(project_for_label(&v, "Nope"), None);
        assert_eq!(slug_preview("Test Проект X"), "test-x");
    }
}
