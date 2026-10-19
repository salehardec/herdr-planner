//! The project list's pure logic: projects built from the board's tasks,
//! sorting, chips, text filter, groups, the workspace-to-project match and the
//! totals for the bar. The page mirrors the sorting, chips and sections.

// Reference logic: the list's sorting, chips and sections are mirrored by the page and used here by tests only.
#![allow(dead_code)]

use crate::board::{Bucket, Task};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tasks of a project by stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Waiting for an answer, a plan, a choice, a decision on a failed review.
    pub needs: u32,
    /// Finished work that waits for the yes that merges it.
    pub ready: u32,
    pub failed: u32,
    /// The reviewer is on it.
    pub review: u32,
    pub working: u32,
    pub queued: u32,
    pub todo: u32,
    pub done: u32,
}

impl Counts {
    pub fn add(&mut self, b: Bucket) {
        let slot = match b {
            Bucket::Needs => &mut self.needs,
            Bucket::Ready => &mut self.ready,
            Bucket::Failed => &mut self.failed,
            Bucket::Review => &mut self.review,
            Bucket::Working => &mut self.working,
            Bucket::Queued => &mut self.queued,
            Bucket::Todo => &mut self.todo,
            Bucket::Done => &mut self.done,
        };
        *slot += 1;
    }

    pub fn total(&self) -> u32 {
        self.attention() + self.review + self.working + self.queued + self.todo + self.done
    }

    /// What waits on the user: an answer, a yes, a look at what went wrong.
    pub fn attention(&self) -> u32 {
        self.needs + self.ready + self.failed
    }

    /// Something happens or waits: nothing here is quiet.
    pub fn active(&self) -> u32 {
        self.attention() + self.review + self.working + self.queued
    }
}

/// One task that waits on the user, in words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wait {
    pub id: String,
    /// `PLN-1: план готов`.
    pub text: String,
    /// When it came to this stage, unix milliseconds; 0 when unknown.
    pub since: u64,
}

/// The waits in words, for a tooltip.
pub fn wait_hint(waits: &[Wait]) -> String {
    waits.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join("; ")
}

/// Done tasks the page gets per project; the rest are only counted.
pub const DONE_SHOWN: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Project {
    /// The repo's name on the board.
    pub slug: String,
    pub name: String,
    /// The repo's short key (`PLN`); empty when not known.
    pub key: String,
    pub path: String,
    pub counts: Counts,
    /// Its tasks, the ones that need a look first; only the newest done ones.
    pub tasks: Vec<Task>,
    /// Done tasks left out of `tasks`.
    pub done_hidden: u32,
    /// Tasks that wait on the user, the newest first.
    pub waits: Vec<Wait>,
    /// When the newest of them came to its stage, unix milliseconds; 0 when none waits.
    pub attention_since: u64,
    /// The latest stage change of any task: the recency signal for the "new" sort.
    pub activity: u64,
    /// When the newest task under review came to review, unix milliseconds; 0 when none is.
    pub review_since: u64,
}

/// The order a project lists its tasks in: what needs the user first, then
/// what runs, what waits, what is not started, what is done; the newest first inside.
fn rank(b: Bucket) -> u8 {
    match b {
        Bucket::Needs => 0,
        Bucket::Ready => 1,
        Bucket::Failed => 2,
        Bucket::Review => 3,
        Bucket::Working => 4,
        Bucket::Queued => 5,
        Bucket::Todo => 6,
        Bucket::Done => 7,
    }
}

impl Project {
    /// A project from the tasks the board lists for its repo.
    pub fn build(slug: &str, key: &str, path: &str, mut tasks: Vec<Task>) -> Project {
        tasks.sort_by(|a, b| {
            rank(a.bucket)
                .cmp(&rank(b.bucket))
                .then(b.at.cmp(&a.at))
                .then(b.n.cmp(&a.n))
        });
        let mut counts = Counts::default();
        let mut waits = Vec::new();
        for t in &tasks {
            counts.add(t.bucket);
            if matches!(t.bucket, Bucket::Needs | Bucket::Ready | Bucket::Failed) {
                waits.push(Wait {
                    id: t.id.clone(),
                    text: format!("{}: {}", t.tag, t.label),
                    since: t.at,
                });
            }
        }
        waits.sort_by_key(|w| std::cmp::Reverse(w.since));
        let activity = tasks.iter().map(|t| t.at).max().unwrap_or(0);
        let attention_since = waits.iter().map(|w| w.since).max().unwrap_or(0);
        let review_since = tasks
            .iter()
            .filter(|t| t.bucket == Bucket::Review)
            .map(|t| t.at)
            .max()
            .unwrap_or(0);
        let mut seen_done = 0;
        let before = tasks.len();
        tasks.retain(|t| {
            if t.bucket != Bucket::Done {
                return true;
            }
            seen_done += 1;
            seen_done <= DONE_SHOWN
        });
        Project {
            slug: slug.into(),
            name: slug.into(),
            key: key.into(),
            path: path.into(),
            counts,
            done_hidden: (before - tasks.len()) as u32,
            tasks,
            waits,
            attention_since,
            activity,
            review_since,
        }
    }

    pub fn needs_attention(&self) -> bool {
        self.counts.attention() > 0
    }

    /// Nothing waits, runs or is queued.
    pub fn quiet(&self) -> bool {
        self.counts.active() == 0
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
                .then(b.counts.attention().cmp(&a.counts.attention()))
                .then(b.counts.review.cmp(&a.counts.review))
                .then(b.counts.working.cmp(&a.counts.working))
                .then(b.activity.cmp(&a.activity))
                .then(a.slug.cmp(&b.slug))
        }),
        // "new": the project whose tasks moved last leads.
        Sort::New => projects.sort_by(|a, b| {
            b.activity
                .cmp(&a.activity)
                .then(b.counts.attention().cmp(&a.counts.attention()))
                .then(a.slug.cmp(&b.slug))
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chip {
    All,
    /// Something waits on the user.
    Waiting,
    /// Something works, is reviewed or queued.
    Active,
    Quiet,
}

pub const CHIPS: [Chip; 4] = [Chip::All, Chip::Waiting, Chip::Active, Chip::Quiet];

impl Chip {
    pub fn label(self) -> &'static str {
        match self {
            Chip::All => "Все",
            Chip::Waiting => "Ждут",
            Chip::Active => "В работе",
            Chip::Quiet => "Тихие",
        }
    }

    pub fn matches(self, p: &Project) -> bool {
        match self {
            Chip::All => true,
            Chip::Waiting => p.counts.attention() > 0,
            Chip::Active => p.counts.review + p.counts.working + p.counts.queued > 0,
            Chip::Quiet => p.quiet(),
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
        .filter(|p| chip.matches(p) && text_matches(&format!("{} {} {}", p.name, p.slug, p.key), query))
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
    /// Tasks that wait on the user in the group.
    pub waiting: u32,
    pub review: u32,
    pub working: u32,
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
            waiting: rows.iter().map(|(p, _)| p.counts.attention()).sum(),
            review: rows.iter().map(|(p, _)| p.counts.review).sum(),
            working: rows.iter().map(|(p, _)| p.counts.working + p.counts.queued).sum(),
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

/// Slug of the project a workspace names, or none. In this order:
/// the task tag the label starts with (`PLN-1 Заголовок`: the key of a repo),
/// the folder the board opened a space for (`spaces`: folder → workspace id),
/// then the project's name or slug as the label.
pub fn project_for_workspace(
    projects: &[Project],
    spaces: &[(String, String)],
    id: &str,
    label: &str,
) -> Option<String> {
    let label = label.trim();
    if let Some(tag) = label.split_whitespace().next() {
        if let Some((key, n)) = tag.rsplit_once('-') {
            if !key.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty() {
                let by_key: Vec<&Project> = projects
                    .iter()
                    .filter(|p| !p.key.is_empty() && p.key.eq_ignore_ascii_case(key))
                    .collect();
                if let [p] = by_key[..] {
                    return Some(p.slug.clone());
                }
            }
        }
    }
    if !id.is_empty() {
        if let Some((path, _)) = spaces.iter().find(|(_, ws)| ws == id) {
            if let Some(p) = projects.iter().find(|p| &p.path == path) {
                return Some(p.slug.clone());
            }
        }
    }
    let key = lower(label);
    let named: Vec<&Project> = projects.iter().filter(|p| lower(p.name.trim()) == key).collect();
    if let [p] = named[..] {
        return Some(p.slug.clone());
    }
    if named.len() > 1 {
        return None;
    }
    projects.iter().find(|p| p.slug == key).map(|p| p.slug.clone())
}

/// What the shell icon shows: totals over the tasks of the board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Tasks that wait on the user: `needs` + `ready` + `failed`.
    pub waiting: u32,
    pub needs: u32,
    pub ready: u32,
    pub failed: u32,
    /// Tasks under review.
    pub review: u32,
    pub working: u32,
    pub queued: u32,
    /// Projects counted (repos of the board).
    pub projects: u32,
    /// When the newest task that waits came to its stage (ms); 0 when none does.
    pub attention_since: u64,
    /// What waits, in words, the newest first: «Alpha: PLN-1: план готов; …».
    pub waiting_hint: String,
}

/// Projects that need something, the newest event first.
fn by_arrival(a: &Project, b: &Project) -> std::cmp::Ordering {
    b.attention_since
        .cmp(&a.attention_since)
        .then(b.counts.attention().cmp(&a.counts.attention()))
        .then(b.counts.review.cmp(&a.counts.review))
        .then(b.activity.cmp(&a.activity))
        .then(a.slug.cmp(&b.slug))
}

pub fn summarize(projects: &[Project]) -> Summary {
    let mut s = Summary::default();
    let mut waiting: Vec<&Project> = Vec::new();
    for p in projects {
        s.needs += p.counts.needs;
        s.ready += p.counts.ready;
        s.failed += p.counts.failed;
        s.review += p.counts.review;
        s.working += p.counts.working;
        s.queued += p.counts.queued;
        s.projects += 1;
        s.attention_since = s.attention_since.max(p.attention_since);
        if p.counts.attention() > 0 {
            waiting.push(p);
        }
    }
    s.waiting = s.needs + s.ready + s.failed;
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
    /// Tasks that wait on the user (`needs` + `ready` + `failed`).
    pub waiting: u32,
    pub ready: u32,
    pub failed: u32,
    pub review: u32,
    /// Tasks being worked on and queued ones.
    pub working: u32,
    pub done: u32,
    /// All tasks of the project, for the progress.
    pub total: u32,
    /// What waits, in words (empty when nothing does).
    pub wait_hint: String,
    /// When the newest task that waits came to its stage (ms); 0 when none does.
    pub attention_since: u64,
    /// The time the popup shows: that of the newest waiting task, else of the newest one under review; 0 for the rest.
    pub since: u64,
    /// Its tasks that wait, run or are queued, the ones that need the user first.
    pub tasks: Vec<TaskLine>,
}

/// A task as one line of the popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskLine {
    pub id: String,
    pub tag: String,
    pub title: String,
    pub bucket: Bucket,
    pub label: String,
    pub progress: Option<u32>,
}

/// Tasks of a project the popup lists under it.
pub const POPUP_TASKS: usize = 5;

impl OverviewProject {
    fn of(p: &Project) -> Self {
        let c = &p.counts;
        OverviewProject {
            slug: p.slug.clone(),
            name: p.name.clone(),
            waiting: c.attention(),
            ready: c.ready,
            failed: c.failed,
            review: c.review,
            working: c.working + c.queued,
            done: c.done,
            total: c.total(),
            wait_hint: wait_hint(&p.waits),
            attention_since: p.attention_since,
            since: if c.attention() > 0 {
                p.attention_since
            } else if c.review > 0 {
                p.review_since
            } else {
                0
            },
            tasks: p
                .tasks
                .iter()
                .filter(|t| !matches!(t.bucket, Bucket::Todo | Bucket::Done))
                .take(POPUP_TASKS)
                .map(|t| TaskLine {
                    id: t.id.clone(),
                    tag: t.tag.clone(),
                    title: t.title.clone(),
                    bucket: t.bucket,
                    label: t.label.clone(),
                    progress: t.progress.filter(|_| matches!(t.bucket, Bucket::Working | Bucket::Review)),
                })
                .collect(),
        }
    }
}

/// What the bar popup lists: who waits on the user (the newest first), whose
/// work is under review (the newest first), who works; the projects where
/// nothing happens are only counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub summary: Summary,
    pub waiting: Vec<OverviewProject>,
    pub review: Vec<OverviewProject>,
    pub working: Vec<OverviewProject>,
    /// Projects with no waiting, reviewed, working or queued task.
    pub quiet: u32,
}

pub fn overview(projects: &[Project]) -> Overview {
    let mut waiting: Vec<&Project> = projects.iter().filter(|p| p.counts.attention() > 0).collect();
    let mut review: Vec<&Project> = projects
        .iter()
        .filter(|p| p.counts.attention() == 0 && p.counts.review > 0)
        .collect();
    let mut working: Vec<&Project> = projects
        .iter()
        .filter(|p| p.counts.attention() == 0 && p.counts.review == 0 && p.counts.working + p.counts.queued > 0)
        .collect();
    let quiet = projects.len() - waiting.len() - review.len() - working.len();
    waiting.sort_by(|a, b| by_arrival(a, b));
    // Under review: by when the newest of the project's reviews began.
    review.sort_by(|a, b| {
        b.review_since
            .cmp(&a.review_since)
            .then(b.counts.review.cmp(&a.counts.review))
            .then(a.slug.cmp(&b.slug))
    });
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
    use serde_json::json;

    /// A project with `n` tasks in each of the given stages: (state, kind, stage).
    fn project(slug: &str, key: &str, tasks: &[(&str, &str, &str, u64)]) -> Project {
        let rows: Vec<Task> = tasks
            .iter()
            .enumerate()
            .map(|(i, (state, kind, stage, at))| {
                let mut t = crate::board::parse_task(&json!({
                    "id": format!("{slug}/{}", i + 1), "state": state, "kind": kind, "stage": stage, "title": format!("T{}", i + 1)
                }))
                .unwrap();
                t.at = *at;
                t.tag = format!("{key}-{}", i + 1);
                t
            })
            .collect();
        Project::build(slug, key, &format!("/p/{slug}"), rows)
    }

    const ASK: (&str, &str, &str) = ("blocked", "question", "working");
    const READY: (&str, &str, &str) = ("blocked", "permission", "approval");
    const WORK: (&str, &str, &str) = ("working", "", "working");
    const REVIEW: (&str, &str, &str) = ("working", "", "reviewing");
    const DONE: (&str, &str, &str) = ("done", "", "done");

    fn t(kind: (&'static str, &'static str, &'static str), at: u64) -> (&'static str, &'static str, &'static str, u64) {
        (kind.0, kind.1, kind.2, at)
    }

    #[test]
    fn a_project_counts_its_tasks_by_stage() {
        let p = project(
            "alpha",
            "ALP",
            &[t(DONE, 1), t(ASK, 50), t(READY, 70), t(WORK, 10), t(REVIEW, 20), t(DONE, 2)],
        );
        let c = p.counts;
        assert_eq!((c.needs, c.ready, c.review, c.working, c.done), (1, 1, 1, 1, 2));
        assert_eq!((c.attention(), c.active(), c.total()), (2, 4, 6));
        // What needs the user first, the newest first inside a stage.
        let order: Vec<&str> = p.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, ["alpha/2", "alpha/3", "alpha/5", "alpha/4", "alpha/6", "alpha/1"]);
        assert_eq!(p.attention_since, 70);
        assert_eq!(p.activity, 70);
        assert_eq!(p.waits.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["ALP-3: готова к одобрению", "ALP-2: ждёт ответа"]);
        assert!(!p.quiet());
        assert!(project("z", "Z", &[t(DONE, 1)]).quiet());
        assert!(project("z", "Z", &[]).quiet());
    }

    #[test]
    fn only_the_newest_done_tasks_travel() {
        let many: Vec<_> = (0..DONE_SHOWN as u64 + 5).map(|i| t(DONE, i + 1)).collect();
        let p = project("alpha", "ALP", &many);
        assert_eq!(p.counts.done as usize, DONE_SHOWN + 5);
        assert_eq!(p.tasks.len(), DONE_SHOWN);
        assert_eq!(p.done_hidden, 5);
        assert_eq!(p.tasks[0].at, DONE_SHOWN as u64 + 5);
    }

    #[test]
    fn sorts_new_and_attention() {
        let mut v = vec![
            project("a", "A", &[t(DONE, 500)]),
            project("b", "B", &[t(ASK, 300)]),
            project("c", "C", &[t(WORK, 900)]),
        ];
        sort_projects(&mut v, Sort::New);
        assert_eq!(v.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["c", "a", "b"]);
        sort_projects(&mut v, Sort::Attention);
        assert_eq!(v.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
    }

    #[test]
    fn chips_and_filter() {
        let v = vec![
            project("alpha", "ALP", &[t(DONE, 5)]),
            project("beta", "BET", &[t(READY, 3)]),
            project("gamma", "GAM", &[t(WORK, 9)]),
            project("delta", "DEL", &[]),
        ];
        let n = chip_counts(&v);
        assert_eq!((n["all"], n["waiting"], n["active"], n["quiet"]), (4, 1, 1, 2));
        assert_eq!(visible_projects(&v, Chip::Waiting, "", Sort::New)[0].slug, "beta");
        assert_eq!(visible_projects(&v, Chip::All, "GAM", Sort::New)[0].slug, "gamma");
        assert_eq!(visible_projects(&v, Chip::All, "del", Sort::New).len(), 1);
        assert!(text_matches("Планировщик проектов", "план проект"));
        assert!(!text_matches("abc", "abd"));
    }

    #[test]
    fn summary_counts_the_tasks_of_the_board() {
        let v = vec![
            project("a", "A", &[t(ASK, 1000), t(READY, 2000), t(REVIEW, 5)]),
            project("b", "B", &[("error", "", "working", 3000), t(WORK, 1)]),
            project("c", "C", &[t(DONE, 9)]),
        ];
        let s = summarize(&v);
        assert_eq!((s.needs, s.ready, s.failed, s.waiting), (1, 1, 1, 3));
        assert_eq!((s.review, s.working, s.projects, s.attention_since), (1, 1, 3, 3000));
        assert_eq!(s.waiting_hint, "b: B-1: упала\na: A-2: готова к одобрению; A-1: ждёт ответа");
        assert_eq!(summarize(&[]), Summary::default());
        assert_eq!(summarize(&v[2..]).waiting_hint, "");
        let many: Vec<Project> = (0..7).map(|i| project(&format!("p{i}"), "P", &[t(ASK, 100 + i)])).collect();
        assert!(summarize(&many).waiting_hint.ends_with("…и ещё 2"));
    }

    #[test]
    fn overview_lists_each_project_once_and_counts_the_quiet() {
        let v = vec![
            project("quiet", "Q", &[t(DONE, 5)]),
            project("empty", "E", &[]),
            project("wait-old", "WO", &[t(ASK, 2000)]),
            project("wait-new", "WN", &[t(READY, 9000), t(WORK, 1)]),
            project("review-old", "RO", &[t(REVIEW, 1000)]),
            project("review-new", "RN", &[t(REVIEW, 5000)]),
            project("busy", "B", &[t(WORK, 7), t(WORK, 8), ("working", "", "queued", 1)]),
            project("queued", "QU", &[("working", "", "after", 1)]),
            // Waits and is under review: listed once, with the waits.
            project("both", "BO", &[t(ASK, 4000), t(REVIEW, 4500)]),
        ];
        let o = overview(&v);
        let slugs = |l: &[OverviewProject]| l.iter().map(|p| p.slug.clone()).collect::<Vec<_>>();
        assert_eq!(slugs(&o.waiting), ["wait-new", "both", "wait-old"], "the newest first");
        assert_eq!(slugs(&o.review), ["review-new", "review-old"]);
        assert_eq!((o.review[0].since, o.review[1].since), (5000, 1000), "a project under review shows when the review began");
        assert_eq!(o.waiting[0].since, 9000);
        assert_eq!(o.working[0].since, 0);
        assert_eq!(slugs(&o.working), ["busy", "queued"]);
        assert_eq!(o.quiet, 2);
        assert_eq!(o.summary.waiting, 3);
        assert_eq!(o.waiting[0].wait_hint, "WN-1: готова к одобрению");
        assert_eq!((o.waiting[0].waiting, o.waiting[0].total, o.waiting[0].working), (1, 2, 1));
        assert_eq!(o.working[0].working, 3, "queued tasks count with the working ones");
        // The popup lists what is going on in a project, what needs the user first; not what is done or not started.
        let lines: Vec<(&str, Bucket)> = o.waiting[1].tasks.iter().map(|l| (l.tag.as_str(), l.bucket)).collect();
        assert_eq!(lines, [("BO-1", Bucket::Needs), ("BO-2", Bucket::Review)]);
        let o = overview(&[project("a", "A", &[]), project("b", "B", &[t(DONE, 1)])]);
        assert!(o.waiting.is_empty() && o.review.is_empty() && o.working.is_empty());
        assert_eq!(o.quiet, 2);
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
            project("a", "A", &[t(READY, 1)]),
            project("b", "B", &[t(REVIEW, 2), t(REVIEW, 3)]),
            project("z", "Z", &[]),
        ];
        let s = group_sections(&g, &v, true);
        assert_eq!(
            s.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Omarchy", "X", OTHER]
        );
        assert_eq!(s[0].waiting, 1);
        assert!(s[1].collapsed && s[1].projects.is_empty());
        assert_eq!(s[2].review, 2);
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
    fn matches_workspace() {
        let mut v = vec![
            project("apicache", "APC", &[]),
            project("other", "OTH", &[]),
            project("dup-a", "DA", &[]),
            project("dup-b", "DB", &[]),
        ];
        v[0].name = "APICache".into();
        v[2].name = "Dup".into();
        v[3].name = "Dup".into();
        let spaces = vec![("/p/other".to_string(), "wDC".to_string())];
        let by = |id: &str, label: &str| project_for_workspace(&v, &spaces, id, label);
        // The label of a task's space starts with its tag.
        assert_eq!(by("", "APC-2 Живой счётчик").as_deref(), Some("apicache"));
        assert_eq!(by("", "apc-12 x").as_deref(), Some("apicache"));
        assert_eq!(by("", "ZZZ-1 x"), None);
        // A repo's own space is known by its id.
        assert_eq!(by("wDC", "herdr-planner").as_deref(), Some("other"));
        assert_eq!(by("wXX", "herdr-planner"), None);
        // Or by the name.
        assert_eq!(by("", "apicache").as_deref(), Some("apicache"));
        assert_eq!(by("", "other").as_deref(), Some("other"));
        assert_eq!(by("", "Dup"), None);
        assert_eq!(by("", "Nope"), None);
    }
}
