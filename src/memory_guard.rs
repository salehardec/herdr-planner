//! The check of a memory text for personal data and secrets, before it is written into a universe's memory (which
//! travels with git, perhaps into a public repository).
//!
//! It looks for: IP addresses, e-mail addresses, internal domains and host names, tokens, keys and passwords, paths into
//! a home folder, and names of people. Most of it is exact (a token has a known shape, an address is an address); the
//! names are a heuristic (a patronymic, initials, a first name from a list next to a capitalised word), and so is the
//! host name (an internal top-level domain, or `gitlab.`/`jenkins.`/`vpn.`… in front of a domain that is not a
//! well-known public one). A finding is not a verdict: the human sees what was found and decides.
//!
//! The terms that are *this* person's — the login name, git's `user.name`/`user.email` — are looked for too
//! (`own_terms`), and so are the names of machines they know: this machine's own name, the `Host` aliases of
//! `~/.ssh/config` and the names in `/etc/hosts` (`app-semantics`, `buildbox`): an internal host name has no dots to
//! give it away. Those come as `host:name` terms and are found as `host`. `HERDR_PLANNER_OWN_TERMS` (a comma list, may
//! be empty; `host:` works there too) replaces all of them, for the checks.

use serde::Serialize;
use std::collections::HashSet;

/// What was found. Shown to the human; the secrets only by their first letters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// `ip`, `email`, `host`, `secret`, `path`, `person`, `own`.
    pub kind: &'static str,
    /// The text found (a secret: its first letters and «…»).
    pub text: String,
    /// `title`, `description`, `body` or `text`.
    pub field: String,
    /// The line in that field, from 1.
    pub line: usize,
    #[serde(skip)]
    start: usize,
    #[serde(skip)]
    end: usize,
}

/// Priority when two findings overlap: the first of these wins.
const ORDER: &[&str] = &["secret", "email", "path", "host", "ip", "own", "person"];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    kind: &'static str,
    start: usize,
    end: usize,
}

const SECRET_WORDS: &[&str] = &[
    "password", "passwd", "pwd", "pass", "secret", "token", "apikey", "api_key", "api-key", "access_key", "private_key",
    "auth", "authorization", "пароль", "пароля", "токен", "секрет",
];

const PREFIXES: &[&str] = &[
    "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-", "npm_", "hf_",
];

const INTERNAL_TLDS: &[&str] = &["local", "internal", "corp", "lan", "intra", "intranet", "localdomain", "private", "home"];

const SERVICE_LABELS: &[&str] = &[
    "gitlab", "jenkins", "vpn", "intranet", "jira", "confluence", "nexus", "artifactory", "sonar", "sonarqube", "grafana",
    "vault", "registry", "git", "ci", "bitbucket", "teamcity", "rancher", "harbor", "wiki", "mail", "ldap", "proxy", "gerrit",
    "kibana",
];

/// Public sites that carry a service label of their own (`gitlab.com`, `git.kernel.org`): not worth a warning.
const PUBLIC_DOMAINS: &[&str] = &[
    "github.com", "gitlab.com", "bitbucket.org", "npmjs.org", "npmjs.com", "kernel.org", "archlinux.org", "crates.io",
    "docker.com", "docker.io", "pypi.org", "example.com", "example.org", "example.net", "googleapis.com", "google.com",
    "microsoft.com", "mozilla.org", "gnu.org", "debian.org", "ubuntu.com", "rust-lang.org",
];

/// Names that look like a login of a program or a CI runner, not of a person.
const GENERIC_LOGINS: &[&str] = &[
    "root", "user", "admin", "runner", "ubuntu", "ci", "build", "node", "git", "www-data", "docker", "pi", "nobody", "ec2-user",
    "debian", "vagrant",
];

const FIRST_NAMES: &[&str] = &[
    "александр", "алексей", "андрей", "антон", "артём", "артем", "борис", "вадим", "валентин", "василий", "виктор", "виталий",
    "владимир", "владислав", "геннадий", "георгий", "григорий", "даниил", "денис", "дмитрий", "евгений", "егор", "иван", "игорь",
    "илья", "кирилл", "константин", "леонид", "максим", "михаил", "никита", "николай", "олег", "павел", "пётр", "петр", "роман",
    "руслан", "сергей", "степан", "тимофей", "фёдор", "федор", "юрий", "ярослав", "александра", "алёна", "алена", "алина", "алла",
    "анастасия", "анна", "валентина", "валерия", "вера", "виктория", "галина", "дарья", "диана", "евгения", "екатерина",
    "елена", "елизавета", "ирина", "кристина", "ксения", "лариса", "лидия", "любовь", "людмила", "маргарита", "марина", "мария",
    "надежда", "наталья", "наталия", "ольга", "полина", "светлана", "софия", "софья", "татьяна", "юлия", "john", "jane", "james",
    "michael", "david", "robert", "william", "richard", "thomas", "mary", "peter", "paul", "mark", "alex", "alexander", "alexey",
    "aleksey", "andrew", "sergey", "ivan", "dmitry", "nikolay", "maria", "olga",
];

const SURNAME_ENDS: &[&str] = &[
    "ов", "ев", "ёв", "ин", "ын", "ский", "цкий", "ной", "ых", "их", "ова", "ева", "ёва", "ина", "ына", "ская", "цкая", "енко",
    "чук", "щук", "юк", "ук",
];

const PATRONYMIC_ENDS: &[&str] = &["ович", "евич", "ьич", "овна", "евна", "ична", "инична", "оглы"];

const FILE_EXTENSIONS: &[&str] = &[
    "md", "toml", "json", "rs", "ts", "tsx", "js", "mjs", "py", "sh", "txt", "yml", "yaml", "html", "css", "lock", "log", "sql",
    "conf", "cfg", "ini", "png", "jpg", "svg", "gif", "pdf", "zip", "tar", "gz", "rb",
];

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | ',' | ';' | '«' | '»')
}

/// The tokens of a text: byte ranges between delimiters, with the sentence punctuation cut off at the end.
fn tokens(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if is_delim(c) {
            if let Some(s) = from.take() {
                out.push((s, i));
            }
        } else if from.is_none() {
            from = Some(i);
        }
    }
    if let Some(s) = from {
        out.push((s, text.len()));
    }
    out
}

fn trim_tail(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut e = end;
    while e > start {
        let c = text[start..e].chars().next_back().unwrap_or(' ');
        if matches!(c, '.' | ':' | ';' | '!' | '?' | ',' | '=') {
            e -= c.len_utf8();
        } else {
            break;
        }
    }
    (start, e)
}

fn has_secret_word(lhs: &str) -> bool {
    let l: String = lhs.trim_matches(|c: char| c == '-' || c == '"' || c == '\'' || c == '$').to_lowercase();
    SECRET_WORDS.iter().any(|w| l == *w || l.ends_with(&format!("_{w}")) || l.ends_with(&format!("-{w}")) || l.ends_with(w) && w.len() >= 5)
}

fn looks_secret_value(v: &str) -> bool {
    let v = v.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
    let n = v.chars().count();
    if n < 4 {
        return false;
    }
    let lower = v.to_lowercase();
    if v.starts_with('<') || v.starts_with('$') || v.starts_with('{') || v.starts_with('*') || lower.contains("xxx") || v.contains("...") || v.contains("***") {
        return false;
    }
    if ["none", "null", "true", "false", "string", "secret", "password", "token", "empty", "unset", "нет", "пусто"].contains(&lower.as_str()) {
        return false;
    }
    let digit = v.chars().any(|c| c.is_ascii_digit());
    let upper = v.chars().any(|c| c.is_uppercase());
    let low = v.chars().any(|c| c.is_lowercase());
    let punct = v.chars().any(|c| "!@#$%^&*".contains(c));
    digit || (upper && low) || punct || n >= 20
}

/// The words after which a secret usually follows in plain speech: «пароль от стенда: …», «Пароль — …», «token …».
const PHRASE_WORDS: &[&str] = &[
    "пароль", "пароля", "паролю", "паролем", "пароли", "password", "passwd", "passphrase", "токен", "токена", "token", "секрет",
    "secret", "apikey",
];

/// How many words after such a word are looked at, up to the end of the sentence.
const PHRASE_WINDOW: usize = 4;

/// A value that follows a word like «пароль» without a label: stricter than `looks_secret_value` (a plain word after
/// «пароль» is just speech), so it needs a digit with letters, mixed case, a long run, or a long number.
fn phrase_secret_value(v: &str) -> bool {
    let v = v.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';' || c == ')' || c == '(');
    let n = v.chars().count();
    let lower = v.to_lowercase();
    if n < 6 || v.contains(['=', '$', '/', ':', '<', '>']) || v.starts_with('<') || v.starts_with('$') || v.starts_with('{') || v.starts_with('*') || lower.contains("xxx") || v.contains("...") || v.contains("***") {
        return false;
    }
    let digit = v.chars().any(|c| c.is_ascii_digit());
    let letter = v.chars().any(|c| c.is_alphabetic());
    let punct = v.chars().any(|c| "!@#$%^&*".contains(c));
    (digit && letter) || (punct && letter) || (n >= 20 && digit) || (v.chars().all(|c| c.is_ascii_digit()) && n >= 6)
}

/// The start of the label word before position `i` (letters, digits, `_`, `-`).
fn label_start(s: &str, i: usize) -> usize {
    s[..i]
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '-'))
        .map(|(p, c)| p + c.len_utf8())
        .unwrap_or(0)
}

fn secret_token(s: &str) -> bool {
    if PREFIXES.iter().any(|p| s.starts_with(p) && s.len() >= p.len() + 8) {
        return true;
    }
    if s.starts_with("sk-") && s.len() >= 20 || s.starts_with("sk_live_") || s.starts_with("rk_live_") {
        return true;
    }
    if (s.starts_with("AKIA") || s.starts_with("ASIA")) && s.len() == 20 && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return true;
    }
    if s.starts_with("AIza") && s.len() >= 35 {
        return true;
    }
    if s.starts_with("eyJ") && s.matches('.').count() == 2 && s.len() >= 30 {
        return true;
    }
    // a long, mixed token: letters of both cases and digits (a git hash is lower-case hex: not this)
    s.len() >= 32
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "+/_=-".contains(c))
        && s.chars().any(|c| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
        && s.chars().any(|c| c.is_ascii_digit())
}

fn valid_domain(host: &str) -> Option<Vec<&str>> {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-') || !l.chars().all(|c| c.is_alphanumeric() || c == '-')) {
        return None;
    }
    let tld = labels[labels.len() - 1];
    if tld.len() < 2 || !tld.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    Some(labels)
}

/// Whether a host name is one worth a warning: an internal top-level domain, or a service label before a domain that is
/// not a well-known public one.
fn internal_host(host: &str) -> bool {
    let host = host.to_lowercase();
    let Some(labels) = valid_domain(&host) else { return false };
    let tld = labels[labels.len() - 1];
    if FILE_EXTENSIONS.contains(&tld) {
        return false;
    }
    if INTERNAL_TLDS.contains(&tld) {
        return true;
    }
    if host.ends_with(".home.arpa") {
        return true;
    }
    if labels.len() >= 3 && SERVICE_LABELS.contains(&labels[0]) {
        let rest = labels[1..].join(".");
        return !PUBLIC_DOMAINS.iter().any(|d| rest == *d || rest.ends_with(&format!(".{d}")));
    }
    false
}

fn find_ipv4(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_digit() || b[i - 1] == b'.' && i >= 2 && b[i - 2].is_ascii_digit())) {
            let mut j = i;
            let mut parts = Vec::new();
            loop {
                let st = j;
                while j < b.len() && b[j].is_ascii_digit() && j - st < 3 {
                    j += 1;
                }
                if j == st {
                    break;
                }
                parts.push(s[st..j].parse::<u32>().unwrap_or(999));
                if parts.len() == 4 {
                    break;
                }
                if j < b.len() && b[j] == b'.' && j + 1 < b.len() && b[j + 1].is_ascii_digit() {
                    j += 1;
                } else {
                    break;
                }
            }
            let after_digit = j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.' && j + 1 < b.len() && b[j + 1].is_ascii_digit());
            if parts.len() == 4 && parts.iter().all(|p| *p <= 255) && !after_digit {
                let loopback = parts[0] == 127 || parts == [0, 0, 0, 0] || parts == [255, 255, 255, 255];
                if !loopback {
                    out.push((i, j));
                }
                i = j;
                continue;
            }
            // skip the whole digit run
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}

fn find_ipv6(s: &str) -> bool {
    let colons = s.matches(':').count();
    colons >= 2
        && s.len() >= 6
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
        && (s.contains("::") || colons == 7)
        && s.chars().any(|c| c.is_ascii_hexdigit())
}

fn path_start(s: &str) -> Option<usize> {
    for mark in ["/home/", "/Users/", "/root/", "/mnt/", "/media/", "/run/media/", ":\\Users\\", ":/Users/", ":/users/", ":\\users\\"] {
        let mut from = 0;
        while let Some(p) = s[from..].find(mark) {
            let at = from + p;
            let after = s[at + mark.len()..].chars().next();
            let before = s[..at].chars().next_back();
            // a drive path starts one letter earlier
            if mark.starts_with(':') {
                if before.is_some_and(|c| c.is_ascii_alphabetic()) && after.is_some() {
                    return Some(at - 1);
                }
            } else if after.is_some_and(|c| c.is_alphanumeric()) && !before.is_some_and(|c| c.is_alphanumeric() || "_.-".contains(c)) {
                return Some(at);
            }
            from = at + 1;
        }
    }
    None
}

fn cyr_capital(w: &str) -> bool {
    let mut it = w.chars();
    match it.next() {
        Some(c) if ('А'..='Я').contains(&c) || c == 'Ё' => {}
        _ => return false,
    }
    let rest: Vec<char> = it.collect();
    rest.len() >= 2 && rest.iter().all(|c| ('а'..='я').contains(c) || *c == 'ё' || *c == '-' || ('А'..='Я').contains(c))
}

fn lat_capital(w: &str) -> bool {
    let mut it = w.chars();
    matches!(it.next(), Some(c) if c.is_ascii_uppercase()) && w.len() >= 3 && it.all(|c| c.is_ascii_lowercase())
}

fn is_capital(w: &str) -> bool {
    cyr_capital(w) || lat_capital(w)
}

fn is_first_name(w: &str) -> bool {
    is_capital(w) && FIRST_NAMES.contains(&w.to_lowercase().as_str())
}

fn is_surname(w: &str) -> bool {
    let l = w.to_lowercase();
    cyr_capital(w) && SURNAME_ENDS.iter().any(|e| l.ends_with(e) && l.chars().count() > e.chars().count() + 1)
}

fn is_patronymic(w: &str) -> bool {
    let l = w.to_lowercase();
    cyr_capital(w) && PATRONYMIC_ENDS.iter().any(|e| l.ends_with(e) && l.chars().count() > e.chars().count() + 1)
}

fn is_initials(w: &str) -> bool {
    let cs: Vec<char> = w.chars().collect();
    let ok = |c: char| ('А'..='Я').contains(&c) || c == 'Ё' || c.is_ascii_uppercase();
    (cs.len() == 2 && ok(cs[0]) && cs[1] == '.') || (cs.len() == 4 && ok(cs[0]) && cs[1] == '.' && ok(cs[2]) && cs[3] == '.')
}

/// The words of a text with their ranges, punctuation cut off (a dot stays on initials).
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from: Option<usize> = None;
    let stop = |c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '!' | '?' | '(' | ')' | '«' | '»' | '"' | '\'' | '`' | '[' | ']' | '{' | '}' | '|' | '—' | '–' | '/' | '@' | '<' | '>' | '=');
    let flush = |s: usize, e: usize, text: &str, out: &mut Vec<(usize, usize)>| {
        let w = &text[s..e];
        let t = w.trim_end_matches('.');
        let keep = if is_initials(w) { w.len() } else { t.len() };
        if keep > 0 {
            out.push((s, s + keep));
        }
    };
    for (i, c) in text.char_indices() {
        if stop(c) {
            if let Some(s) = from.take() {
                flush(s, i, text, &mut out);
            }
        } else if from.is_none() {
            from = Some(i);
        }
    }
    if let Some(s) = from {
        flush(s, text.len(), text, &mut out);
    }
    out
}

fn person_spans(text: &str) -> Vec<Span> {
    let ws = words(text);
    let w = |i: usize| &text[ws[i].0..ws[i].1];
    let mut out = Vec::new();
    let mut i = 0;
    while i < ws.len() {
        let cur = w(i);
        let next = (i + 1 < ws.len()).then(|| w(i + 1));
        // two words in a row only: «Иван Петров», not «Иван, Петров»
        let adjacent = |a: usize| a + 1 < ws.len() && text[ws[a].1..ws[a + 1].0].chars().all(|c| c == ' ' || c == '.');
        if is_patronymic(cur) {
            out.push(Span { kind: "person", start: ws[i].0, end: ws[i].1 });
        } else if is_initials(cur) && next.is_some_and(cyr_or_lat_name) && adjacent(i) {
            out.push(Span { kind: "person", start: ws[i].0, end: ws[i + 1].1 });
            i += 1;
        } else if let Some(n) = next.filter(|_| adjacent(i)) {
            if cyr_or_lat_name(cur) && is_initials(n) {
                out.push(Span { kind: "person", start: ws[i].0, end: ws[i + 1].1 });
                i += 1;
            } else if (is_first_name(cur) && is_capital(n) && n.chars().count() >= 3)
                || (is_capital(cur) && cyr_capital(cur) && is_surname(n) && cyr_capital(n))
                || (is_surname(cur) && is_first_name(n))
            {
                out.push(Span { kind: "person", start: ws[i].0, end: ws[i + 1].1 });
                i += 1;
            }
        }
        i += 1;
    }
    out
}

fn cyr_or_lat_name(w: &str) -> bool {
    cyr_capital(w) || lat_capital(w)
}

/// A term that names a machine rather than a person: `host:app-semantics`.
const HOST_TERM: &str = "host:";

fn own_spans(text: &str, own: &[String]) -> Vec<Span> {
    let lower = text.to_lowercase();
    // `to_lowercase` may change byte lengths; positions are only trusted when it did not
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for term in own {
        let term = term.trim();
        let (kind, term) = match term.strip_prefix(HOST_TERM) {
            Some(rest) => ("host", rest),
            None => ("own", term),
        };
        let t = term.to_lowercase();
        if t.chars().count() < 3 {
            continue;
        }
        let mut from = 0;
        while let Some(p) = lower[from..].find(&t) {
            let at = from + p;
            let end = at + t.len();
            let before = lower[..at].chars().next_back();
            let after = lower[end..].chars().next();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
            if !word(before) && !word(after) {
                out.push(Span { kind, start: at, end });
            }
            from = end;
        }
    }
    out
}

fn token_spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut pending_secret = false;
    let mut phrase_left = 0usize;
    let mut prev = String::new();
    for (ts, te) in tokens(text) {
        let (s0, e0) = trim_tail(text, ts, te);
        let had_sep = text[e0..te].contains(':') || text[e0..te].contains('=');
        if e0 <= s0 {
            pending_secret = false;
            continue;
        }
        let s = &text[s0..e0];
        let mut done = false;
        // the value after «пароль:» / «password =» in the next word
        if pending_secret {
            let scheme = s.eq_ignore_ascii_case("bearer") || s.eq_ignore_ascii_case("basic");
            if scheme {
                // «Authorization: Bearer <token>»: the value is the next word
                pending_secret = true;
            } else if looks_secret_value(s) {
                out.push(Span { kind: "secret", start: s0, end: e0 });
                done = true;
            }
            if !scheme {
                pending_secret = false;
            }
        }
        // plain speech: «пароль от стенда: Qwerty123», «Пароль — Qwerty123»
        let word = s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if !done && phrase_left > 0 {
            phrase_left -= 1;
            if phrase_secret_value(s) {
                out.push(Span { kind: "secret", start: s0, end: e0 });
                done = true;
                phrase_left = 0;
            } else if text[..te].ends_with(['.', '!', '?']) {
                phrase_left = 0;
            }
        }
        if PHRASE_WORDS.contains(&word.as_str()) {
            phrase_left = PHRASE_WINDOW;
        }
        // `mysql -pSecret123`: the password is glued to the option
        if !done && s.starts_with("-p") && !s.starts_with("--") && phrase_secret_value(&s[2..]) {
            out.push(Span { kind: "secret", start: s0 + 2, end: e0 });
            done = true;
        }
        // `--password Secret123`: the value is the next word
        if s.starts_with("--") && has_secret_word(s) && !s.contains('=') {
            pending_secret = true;
        }
        if !done && (prev == "bearer" || prev == "basic") && s.len() >= 16 && !s.contains('/') {
            out.push(Span { kind: "secret", start: s0, end: e0 });
            done = true;
        }
        if !done && secret_token(s) {
            out.push(Span { kind: "secret", start: s0, end: e0 });
            done = true;
        }
        // a password inside an address: scheme://user:password@host
        if !done {
            if let Some(p) = s.find("://") {
                let rest = &s[p + 3..];
                if let (Some(at), Some(colon)) = (rest.find('@'), rest.find(':')) {
                    if colon < at && !rest[..at].contains('/') && at > colon + 1 {
                        let a = s0 + p + 3 + colon + 1;
                        out.push(Span { kind: "secret", start: a, end: a + (at - colon - 1) });
                        done = true;
                    }
                }
            }
        }
        // key=value pairs anywhere in the token (a URL query, a connection string): `…?user=app&password=Secret123`
        if !done {
            let mut inner_sep = false;
            for (i, c) in s.char_indices() {
                if !(c == '=' || c == ':') || s[i..].starts_with("://") {
                    continue;
                }
                inner_sep = true;
                let lhs = &s[label_start(s, i)..i];
                if lhs.is_empty() || !has_secret_word(lhs) {
                    continue;
                }
                let vstart = i + 1;
                let vend = s[vstart..].find(['&', ';']).map(|p| vstart + p).unwrap_or(s.len());
                let v = &s[vstart..vend];
                if looks_secret_value(v) {
                    let off = v.len() - v.trim_start_matches(['"', '\'']).len();
                    let end = if vend == s.len() { e0 } else { s0 + vend };
                    out.push(Span { kind: "secret", start: s0 + vstart + off, end });
                }
            }
            // `пароль:` and the value in the next word
            if !inner_sep && had_sep && has_secret_word(s) {
                pending_secret = true;
            }
        }
        if !done {
            // paths
            if let Some(p) = path_start(s) {
                out.push(Span { kind: "path", start: s0 + p, end: e0 });
                done = true;
            }
        }
        if !done {
            let in_url = s.contains("://");
            let host_part = |t: &str| {
                let t = t.split_once("://").map(|x| x.1).unwrap_or(t);
                let t = t.rsplit_once('@').map(|x| x.1).unwrap_or(t);
                t.split(['/', ':', '?', '#']).next().unwrap_or("").to_string()
            };
            if let Some(at) = s.find('@').filter(|_| !in_url) {
                let local = &s[..at];
                let dom = &s[at + 1..];
                let scp = dom.contains(':');
                let host = dom.split([':', '/']).next().unwrap_or("");
                let local_ok = !local.is_empty() && local.chars().all(|c| c.is_alphanumeric() || "._%+-".contains(c));
                if local_ok && !scp && valid_domain(host).is_some() && !FILE_EXTENSIONS.contains(&host.rsplit('.').next().unwrap_or("")) {
                    out.push(Span { kind: "email", start: s0, end: e0 });
                    done = true;
                } else if local_ok && (scp || !host.contains('.')) && !host.is_empty() && host != "localhost" && (internal_host(host) || !host.contains('.') && !scp) {
                    // user@host: the host name (a dotless one only in the `ssh user@host` form)
                    out.push(Span { kind: "host", start: s0 + at + 1, end: s0 + at + 1 + host.len() });
                    done = true;
                }
            }
            if !done {
                let h = host_part(s);
                if internal_host(&h) {
                    let off = s.find(&h).unwrap_or(0);
                    out.push(Span { kind: "host", start: s0 + off, end: s0 + off + h.len() });
                    done = true;
                }
            }
        }
        if !done {
            for (a, b) in find_ipv4(s) {
                out.push(Span { kind: "ip", start: s0 + a, end: s0 + b });
                done = true;
            }
            if !done && find_ipv6(s.trim_matches(['[', ']'])) {
                out.push(Span { kind: "ip", start: s0, end: e0 });
            }
        }
        prev = s.to_lowercase();
    }
    // a private key block: from its first line to the last
    let mut from = 0;
    while let Some(p) = text[from..].find("-----BEGIN ") {
        let start = from + p;
        let line_end = text[start..].find('\n').map(|n| start + n).unwrap_or(text.len());
        if !text[start..line_end].contains("PRIVATE KEY-----") {
            from = line_end;
            continue;
        }
        let end = match text[line_end..].find("-----END ") {
            Some(e) => {
                let at = line_end + e;
                text[at..].find('\n').map(|n| at + n).unwrap_or(text.len())
            }
            None => line_end,
        };
        out.push(Span { kind: "secret", start, end });
        from = end;
    }
    out
}

fn spans(text: &str, own: &[String]) -> Vec<Span> {
    let mut all = token_spans(text);
    all.extend(own_spans(text, own));
    all.extend(person_spans(text));
    let mut kept: Vec<Span> = Vec::new();
    for kind in ORDER {
        for s in all.iter().filter(|s| s.kind == *kind) {
            if !kept.iter().any(|k| k.start < s.end && s.start < k.end) {
                kept.push(*s);
            }
        }
    }
    kept.sort_by_key(|s| s.start);
    kept
}

fn shown(kind: &str, text: &str) -> String {
    if kind == "secret" {
        let head: String = text.chars().take(3).collect();
        format!("{head}…")
    } else {
        text.to_string()
    }
}

/// What a text holds that should not travel with git. `field` names where the text comes from.
pub fn scan(text: &str, field: &str, own: &[String]) -> Vec<Finding> {
    spans(text, own)
        .into_iter()
        .map(|s| Finding {
            kind: s.kind,
            text: shown(s.kind, &text[s.start..s.end]),
            field: field.to_string(),
            line: text[..s.start].matches('\n').count() + 1,
            start: s.start,
            end: s.end,
        })
        .collect()
}

/// The same text with what `scan` found replaced by placeholders (`<ip>`, `<email>`, …; a home path keeps what is
/// below the home folder as `~/…`).
pub fn redact(text: &str, own: &[String]) -> String {
    let mut out = text.to_string();
    for s in spans(text, own).into_iter().rev() {
        let found = &text[s.start..s.end];
        let to = match s.kind {
            "secret" => "<секрет>".to_string(),
            "email" => "<email>".to_string(),
            "ip" => "<ip>".to_string(),
            "host" => "<хост>".to_string(),
            "path" => home_relative(found),
            _ => "<имя>".to_string(),
        };
        out.replace_range(s.start..s.end, &to);
    }
    out
}

/// `/home/ivan/work/x` → `~/work/x`; any other path into a personal place → `<путь>`.
fn home_relative(path: &str) -> String {
    for mark in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(mark) {
            return match rest.split_once('/') {
                Some((_, tail)) => format!("~/{tail}"),
                None => "~".to_string(),
            };
        }
    }
    "<путь>".to_string()
}

/// Names that are not worth a hit as a machine's name: they are in every `/etc/hosts` and `~/.ssh/config`, or are words.
const GENERIC_HOSTS: &[&str] = &[
    "localhost", "localhost.localdomain", "broadcasthost", "ip6-localhost", "ip6-loopback", "ip6-localnet", "ip6-mcastprefix",
    "ip6-allnodes", "ip6-allrouters", "ip6-allhosts", "host", "server", "web", "dev", "prod", "test", "stage", "staging", "home",
    "work", "main", "local", "default",
];

fn is_ip_like(name: &str) -> bool {
    name.contains(':') || name.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Whether a name from a host list is worth looking for: not a pattern, not an address (those are found as they are), not
/// a public site, not a word every machine has.
fn host_name_ok(name: &str) -> bool {
    let n = name.to_lowercase();
    n.chars().count() >= 4
        && !n.contains(['*', '?', '!', '/', '@', '%', '$'])
        && !is_ip_like(&n)
        && !GENERIC_HOSTS.contains(&n.as_str())
        && !PUBLIC_DOMAINS.iter().any(|d| n == *d || n.ends_with(&format!(".{d}")))
}

/// `DokployMain_10.1.2.3` → `DokployMain`: tools that export an ssh config (Tabby, …) write the address into the alias,
/// and a memory text names the machine without it.
fn without_address(alias: &str) -> Option<String> {
    let (head, tail) = alias.rsplit_once(['_', '-'])?;
    (!head.is_empty() && is_ip_like(tail) && tail.contains('.')).then(|| head.to_string())
}

/// The names to look for in a text, as `host:` terms: each name whole, and without an address glued to its end. A name
/// shorter than `min` letters is left out (it would be a word).
pub fn host_terms(names: impl IntoIterator<Item = String>, min: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.trim().to_string();
        for name in std::iter::once(n.clone()).chain(without_address(&n)) {
            let ok = if min >= 4 {
                host_name_ok(&name)
            } else {
                name.chars().count() >= min && !GENERIC_HOSTS.contains(&name.to_lowercase().as_str()) && !is_ip_like(&name) && !name.contains(['*', '?', '!', '/', '@'])
            };
            let term = format!("{HOST_TERM}{name}");
            if ok && !out.iter().any(|t| t.eq_ignore_ascii_case(&term)) {
                out.push(term);
            }
        }
    }
    out
}

/// The `Host` aliases and `HostName`s of an `ssh_config` text.
pub fn ssh_config_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.splitn(2, |c: char| c.is_whitespace() || c == '=');
        let (Some(key), Some(rest)) = (it.next(), it.next()) else { continue };
        if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("hostname") {
            out.extend(rest.trim_start_matches(['=', ' ', '\t']).split_whitespace().map(|h| h.trim_matches('"').to_string()));
        }
    }
    out.into_iter().filter(|h| host_name_ok(h)).collect()
}

/// The names in an `/etc/hosts` text (everything after the address, comments left out).
pub fn etc_hosts_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        out.extend(line.split_whitespace().skip(1).map(String::from));
    }
    out.into_iter().filter(|h| host_name_ok(h)).collect()
}

/// `~/.ssh/config` with its `Include`s (a file, or a folder's files with `*`; one level deep, no further).
fn ssh_config_text(home: &std::path::Path) -> String {
    let ssh = home.join(".ssh");
    let mut text = std::fs::read_to_string(ssh.join("config")).unwrap_or_default();
    let includes: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            (l.len() > 8 && l[..7].eq_ignore_ascii_case("include") && l[7..].starts_with([' ', '\t', '='])).then(|| l[8..].trim().to_string())
        })
        .collect();
    for inc in includes {
        for pat in inc.split_whitespace() {
            let path = if pat.starts_with('/') {
                std::path::PathBuf::from(pat)
            } else if let Some(r) = pat.strip_prefix("~/") {
                home.join(r)
            } else {
                ssh.join(pat)
            };
            let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else { continue };
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let (pre, post) = match file.split_once('*') {
                Some((a, b)) => (a, Some(b)),
                None => (file, None),
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let hit = match post {
                    Some(post) => name.starts_with(pre) && name.ends_with(post),
                    None => name == pre,
                };
                if hit && e.path().is_file() {
                    text.push('\n');
                    text.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
    }
    text
}

/// The terms of this person that must not appear in a project's memory: login and git identity, and (as `host:` terms)
/// the names of machines they know: this one's, the `~/.ssh/config` aliases, the `/etc/hosts` names.
pub fn own_terms() -> Vec<String> {
    if let Ok(v) = std::env::var("HERDR_PLANNER_OWN_TERMS") {
        return v.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    }
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut people: Vec<String> = Vec::new();
            for key in ["USER", "LOGNAME"] {
                people.extend(std::env::var(key).ok());
            }
            for key in ["user.name", "user.email"] {
                if let Ok(o) = std::process::Command::new("git").args(["config", "--global", key]).stdin(std::process::Stdio::null()).output() {
                    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !v.is_empty() {
                        people.push(v);
                    }
                }
            }
            let mut seen = HashSet::new();
            let mut out: Vec<String> = people
                .into_iter()
                .filter(|t| t.chars().count() >= 3 && !GENERIC_LOGINS.contains(&t.to_lowercase().as_str()) && seen.insert(t.to_lowercase()))
                .collect();
            let mut hosts: Vec<String> = Vec::new();
            hosts.extend(std::env::var("HOSTNAME").ok());
            hosts.extend(std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()));
            hosts.extend(etc_hosts_names(&std::fs::read_to_string("/etc/hosts").unwrap_or_default()));
            hosts.extend(ssh_config_hosts(&ssh_config_text(&crate::config::home())));
            out.extend(host_terms(hosts, 4).into_iter().filter(|t| seen.insert(t.to_lowercase())));
            out
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<&'static str> {
        scan(text, "text", &[]).iter().map(|f| f.kind).collect()
    }

    fn own(t: &str) -> Vec<String> {
        vec![t.to_string()]
    }

    #[test]
    fn plain_project_text_is_clean() {
        for t in [
            "Слияние идёт squash: ветки задач короткие, лента main остаётся читаемой.",
            "Решили не хранить тему в базе: хватает localStorage и системной настройки.",
            "Сервер слушает 127.0.0.1:7600, а не 0.0.0.0; версия 2.1.303, коммит 7a0b10a1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7.",
            "См. docs/memory/MEMORY.md и src/memory.rs, затем https://github.com/o/r/pull/12 или git@github.com:o/r.git.",
            "Claude Code и Pull Request — это названия, а не люди. Token хранится в keyring. Время 12:30:45.",
            "Пароль хранится в связке ключей, токен — тоже; пример: password: <секрет>, token=$TOKEN.",
        ] {
            assert!(scan(t, "text", &[]).is_empty(), "{t:?} → {:?}", scan(t, "text", &[]));
        }
    }

    #[test]
    fn ip_addresses_are_found_but_loopback_is_not() {
        assert_eq!(kinds("сервер 203.0.113.7 за VPN"), ["ip"]);
        assert_eq!(kinds("host=198.51.100.20:2222"), ["ip"]);
        assert_eq!(kinds("ssh dev@203.0.113.9"), ["ip"]);
        assert_eq!(kinds("адрес 2001:db8::1 в сети"), ["ip"]);
        assert!(kinds("localhost 127.0.0.1 и 0.0.0.0").is_empty());
        assert!(kinds("версия 1.2.3 и 300.1.1.1").is_empty());
        assert_eq!(scan("203.0.113.7", "text", &[])[0].text, "203.0.113.7");
    }

    #[test]
    fn email_addresses_are_found_but_a_git_remote_is_not_one() {
        let f = scan("пишите ivan.petrov@corp-mail.example.org, потом ещё a@b.co", "body", &[]);
        assert_eq!(f.iter().map(|f| f.kind).collect::<Vec<_>>(), ["email", "email"]);
        assert_eq!(f[0].text, "ivan.petrov@corp-mail.example.org");
        assert_eq!(f[0].field, "body");
        assert!(kinds("git@github.com:o/r.git и ssh://git@github.com/o/r").is_empty());
        assert!(kinds("файл README.md и config@2x.png").is_empty());
    }

    #[test]
    fn internal_domains_and_host_names_are_found() {
        assert_eq!(kinds("репозиторий на gitlab.example-corp.io/team/app"), ["host"]);
        assert_eq!(kinds("https://jenkins.acme-corp.io/job/x"), ["host"]);
        assert_eq!(kinds("сервис db01.office.local падает"), ["host"]);
        assert_eq!(kinds("git@gitlab.acme-corp.io:team/app.git"), ["host"]);
        assert_eq!(kinds("зайти по ssh dev@buildbox и запустить"), ["host"]);
        assert!(kinds("gitlab.com, github.com, git.kernel.org, registry.npmjs.org, docs.rs").is_empty());
        assert!(kinds("файлы main.rs и index.html и memory.md").is_empty());
    }

    #[test]
    fn tokens_keys_and_passwords_are_found_and_shown_only_by_their_beginning() {
        let cases = [
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_11ABCDEFG0abcdefghijkl_mnopqrstuvwxyz",
            "glpat-abcdefghij0123456789",
            "AKIAIOSFODNN7EXAMPLE",
            "sk-abcdefghijklmnopqrstuvwxyz012345",
            "xoxb-1234567890-abcdefghij",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
            "aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5hJ7",
        ];
        for c in cases {
            let f = scan(&format!("ключ {c} вот"), "text", &[]);
            assert_eq!(f.len(), 1, "{c}");
            assert_eq!(f[0].kind, "secret", "{c}");
            assert!(f[0].text.ends_with('…') && f[0].text.chars().count() <= 4, "a secret is shown by its beginning: {:?}", f[0].text);
        }
        assert_eq!(kinds("password: hunter2 и всё"), ["secret"]);
        assert_eq!(kinds("пароль: Qwerty123"), ["secret"]);
        assert_eq!(kinds("export API_TOKEN=abc123def456"), ["secret"]);
        assert_eq!(kinds("curl --password=s3cret!x"), ["secret"]);
        assert_eq!(kinds("Authorization: Bearer abcdef0123456789abcdef"), ["secret"]);
        assert_eq!(kinds("clone https://bob:p4ssw0rd@git.example.net/r.git"), ["secret"]);
        assert_eq!(kinds("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"), ["secret"]);
    }

    #[test]
    fn paths_into_a_home_folder_are_found() {
        assert_eq!(kinds("лежит в /home/ivan/work/app/src"), ["path"]);
        assert_eq!(kinds("файл /Users/maria/Projects/x"), ["path"]);
        assert_eq!(kinds("диск /mnt/Archive/Projects"), ["path"]);
        assert_eq!(kinds(r"C:\Users\petrov\Documents"), ["path"]);
        assert!(kinds("в src/home/page.rs и docs/mnt/x и ~/Projects").is_empty());
    }

    #[test]
    fn names_of_people_are_found_by_form() {
        assert_eq!(kinds("решение принял Иван Петров на встрече"), ["person"]);
        assert_eq!(kinds("согласовано с Ивановым"), Vec::<&str>::new(), "a declined surname alone is not enough");
        assert_eq!(kinds("автор: Сергей Викторович"), ["person"]);
        assert_eq!(kinds("ответственный Смирнова А. В. сказал"), ["person"]);
        assert_eq!(kinds("И. И. Сидоров просил"), ["person"]);
        assert_eq!(kinds("написал Aleksey Smith"), ["person"]);
        assert!(kinds("Claude Code, Pull Request, Open Source и Merge Request").is_empty());
        assert!(kinds("Решили сливать squash. Дальше смотрим логи.").is_empty());
    }

    #[test]
    fn this_persons_own_terms_are_found_as_whole_words() {
        let f = scan("машина buildhost, логин Jdoe и jd", "text", &["buildhost".into(), "jdoe".into(), "ab".into()]);
        assert_eq!(f.iter().map(|f| f.kind).collect::<Vec<_>>(), ["own", "own"]);
        assert!(scan("buildhostile", "text", &own("buildhost")).is_empty(), "a part of a word is not a hit");
    }

    #[test]
    fn names_of_machines_from_ssh_config_and_hosts_are_found_as_hosts() {
        let ssh = "Host app-semantics\n    HostName 10.31.21.29\nHost salehost dokploymain\n  HostName dokploy.acme-corp.io\nHost *\n  User x\nHost github.com\n  IdentityFile ~/.ssh/k # buildbox\nHost web\n";
        let hosts = ssh_config_hosts(ssh);
        assert_eq!(hosts, ["app-semantics", "salehost", "dokploymain", "dokploy.acme-corp.io"], "no patterns, addresses, public sites, generic words");
        let etc = "127.0.0.1 localhost\n::1 localhost ip6-localhost\n10.0.0.5 gigabox gigabox.lan # note\n";
        assert_eq!(etc_hosts_names(etc), ["gigabox", "gigabox.lan"]);
        let terms: Vec<String> = hosts.iter().chain(etc_hosts_names(etc).iter()).map(|h| format!("host:{h}")).collect();
        let f = scan("поднял стенд на app-semantics, ключ от salehost, деплой через dokploymain; web и github.com не тронуты", "text", &terms);
        assert_eq!(f.iter().map(|f| (f.kind, f.text.as_str())).collect::<Vec<_>>(), [("host", "app-semantics"), ("host", "salehost"), ("host", "dokploymain")]);
        assert!(scan("сервис app-semanticsx", "text", &terms).is_empty(), "a part of a word is not a hit");
        assert_eq!(redact("зайти на app-semantics по ssh", &terms), "зайти на <хост> по ssh");
    }

    #[test]
    fn an_alias_with_an_address_glued_on_is_found_by_the_bare_name() {
        // a Tabby export: `Host DokployMain_10.1.2.3`
        let ssh = "Host DokployMain_10.1.2.3\n  HostName 10.1.2.3\nHost flowise_10.14.129.139\nHost web_10.0.0.1\n";
        let terms = host_terms(ssh_config_hosts(ssh), 4);
        assert!(terms.contains(&"host:DokployMain_10.1.2.3".to_string()) && terms.contains(&"host:DokployMain".to_string()), "{terms:?}");
        assert!(terms.contains(&"host:flowise".to_string()), "{terms:?}");
        assert!(!terms.contains(&"host:web".to_string()), "a generic word stays a word: {terms:?}");
        let f = scan("Деплой идёт через dokploymain, а ещё DokployMain_10.1.2.3 и flowise", "text", &terms);
        assert_eq!(f.iter().map(|f| f.kind).collect::<Vec<_>>(), ["host", "host", "host"], "{f:?}");
        assert_eq!(redact("через dokploymain", &terms), "через <хост>");
        // runner names are short by choice: three letters are enough there
        let r = host_terms(["vm-r".to_string(), "ab".to_string(), "10.2.3.4".to_string()], 3);
        assert_eq!(r, ["host:vm-r"]);
    }

    #[test]
    fn ssh_config_includes_are_followed_one_level() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(ssh.join("config"), "Include conf.d/*.conf\nInclude ~/.ssh/extra\nHost first-box\n").unwrap();
        std::fs::write(ssh.join("conf.d/a.conf"), "Host dokploymain\n").unwrap();
        std::fs::write(ssh.join("conf.d/b.txt"), "Host not-included\n").unwrap();
        std::fs::write(ssh.join("extra"), "Host salehost\n  HostName 10.1.1.1\n").unwrap();
        let mut hosts = ssh_config_hosts(&ssh_config_text(home.path()));
        hosts.sort();
        assert_eq!(hosts, ["dokploymain", "first-box", "salehost"]);
    }

    #[test]
    fn a_finding_says_the_field_and_the_line() {
        let f = scan("первая\nвторая 203.0.113.5", "body", &[]);
        assert_eq!((f[0].field.as_str(), f[0].line), ("body", 2));
    }

    #[test]
    fn redaction_replaces_what_was_found_and_nothing_else() {
        let t = "Сервер 203.0.113.7, пишите a@b.example.org, пароль: Qwerty123, лежит в /home/ivan/work/x, gitlab.acme-corp.io, Иван Петров, диск /mnt/Archive/p, buildhost.";
        let r = redact(t, &own("buildhost"));
        assert_eq!(
            r,
            "Сервер <ip>, пишите <email>, пароль: <секрет>, лежит в ~/work/x, <хост>, <имя>, диск <путь>, <имя>."
        );
        assert!(scan(&r, "text", &own("buildhost")).is_empty(), "what remains is clean: {r}");
        assert_eq!(redact("чисто", &[]), "чисто");
    }

    #[test]
    fn own_terms_can_be_set_for_the_checks() {
        std::env::set_var("HERDR_PLANNER_OWN_TERMS", "bob-the-builder, ,x");
        assert_eq!(own_terms(), ["bob-the-builder", "x"]);
        std::env::set_var("HERDR_PLANNER_OWN_TERMS", "");
        assert!(own_terms().is_empty());
        std::env::remove_var("HERDR_PLANNER_OWN_TERMS");
    }

    #[test]
    fn a_password_in_a_url_query_or_a_connection_string_is_found_next_to_other_findings() {
        let text = "строка jdbc:postgresql://db.internal:5432/app?user=app&password=Secr3t";
        let f = scan(text, "text", &[]);
        let k: Vec<_> = f.iter().map(|f| f.kind).collect();
        assert!(k.contains(&"host") && k.contains(&"secret"), "{f:?}");
        assert_eq!(kinds("https://api.example.org/v1?token=abc123def456&page=2"), ["secret"]);
        assert_eq!(kinds("postgres://app:Secr3t@localhost/db?sslmode=require"), ["secret"]);
        // what is offered instead is clean
        let r = redact(text, &[]);
        assert!(!r.contains("Secr3t") && scan(&r, "text", &[]).is_empty(), "{r}");
    }

    #[test]
    fn a_password_in_plain_phrasing_is_found() {
        for t in [
            "пароль от стенда: Qwerty123",
            "Пароль — Qwerty123",
            "мой пароль Qwerty123 не показывать",
            "пароль от БД Qwerty123.",
            "the password is Secr3tPass",
            "token hunter2hunter2",
            "mysql -u root -pSecr3t123 app",
            "mysql --password Secr3t123 app",
        ] {
            let f = scan(t, "text", &[]);
            assert!(f.iter().any(|f| f.kind == "secret"), "{t:?} → {f:?}");
            let r = redact(t, &[]);
            assert!(!r.contains("Qwerty123") && !r.contains("Secr3t") && !r.contains("hunter2"), "{t:?} → {r}");
        }
        for t in [
            "пароль хранится в связке ключей, а токен выдаёт сервер",
            "Пароль. Qwerty123 — это пример имени переменной?",
            "смена пароля раз в квартал",
            "token: string и password: <секрет>",
            "mkdir -p src/memory и cp -p a.txt b.txt",
        ] {
            let f: Vec<_> = scan(t, "text", &[]).into_iter().filter(|f| f.kind == "secret").collect();
            assert!(f.is_empty() || t.starts_with("Пароль. "), "{t:?} → {f:?}");
        }
    }
}
