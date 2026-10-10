// SPDX-License-Identifier: GPL-3.0-or-later

//! Repository chores, run as `cargo xtask <subcommand>`; not a workspace member.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// The copyright notice lives in `LICENSE`, so a name or year change is one edit.
const HEADER: &str = "// SPDX-License-Identifier: GPL-3.0-or-later";

/// `__pycache__` embeds the compiling machine's absolute paths.
const SKIP_DIRS: &[&str] = &[".git", "target", "__pycache__"];

const HOOKS: &[&str] = &["commit-msg", "pre-commit"];

/// The SPRT worker's `-T`: the figure depends on concurrency, so it is measured at the
/// worker's.
const DEFAULT_CONCURRENCY: usize = 6;

/// Each pair is dev then base, as the worker benches before every workload.
const DEFAULT_PAIRS: usize = 5;

/// What `check-headers` leaves unread, printed after its verdict whatever the verdict.
const HEADERS_NOT_EXAMINED: &[&str] = &[
    "files other than Rust, which nothing reads for a licence line",
    "the directories in SKIP_DIRS",
];

/// What `check-boundary` leaves unread, printed after its verdict whatever the verdict.
/// The rules file is skipped because it spells the rules out, and the exact-name sweep run
/// outside this tree still reads it.
const BOUNDARY_NOT_EXAMINED: &[&str] = &[
    "xtask/src/main.rs, which spells the rules out",
    "commit messages (.githooks/check-message-metadata reads them)",
    "exact names from outside this tree (a list of them here would inventory what is withheld)",
];

/// Added to [`BOUNDARY_NOT_EXAMINED`] when only the index is read.
const STAGED_NOT_EXAMINED: &str =
    "files this commit does not add or change, and deleted ones (CI reads the whole tree)";

/// What `check-changelog` leaves unread, printed after its verdict whatever the verdict.
const CHANGELOG_NOT_EXAMINED: &[&str] = &[
    "what an entry says (its presence and its heading's shape are checked)",
    "tags this clone has not fetched (the tags are the local clone's)",
];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("check-headers") => stating(check_headers(), HEADERS_NOT_EXAMINED),
        Some("check-boundary") => {
            let rest: Vec<String> = args.collect();
            match rest
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                [] => stating(check_boundary(Source::WorkingTree), BOUNDARY_NOT_EXAMINED),
                ["--staged"] => stating(
                    stating(check_boundary(Source::Index), BOUNDARY_NOT_EXAMINED),
                    &[STAGED_NOT_EXAMINED],
                ),
                _ => {
                    eprintln!("xtask check-boundary: expected no argument or `--staged`");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-changelog") => stating(check_changelog(), CHANGELOG_NOT_EXAMINED),
        Some("install-hooks") => install_hooks(),
        Some("nps") => nps(&args.collect::<Vec<_>>()),
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}`");
            usage();
            ExitCode::FAILURE
        }
        None => {
            usage();
            ExitCode::FAILURE
        }
    }
}

/// A pass saying only "OK" reads like one that looked at everything, so each check names what
/// it did not.
fn stating(code: ExitCode, spots: &[&str]) -> ExitCode {
    for spot in spots {
        println!("  not examined: {spot}");
    }
    code
}

fn usage() {
    eprintln!("usage: cargo xtask <subcommand>");
    eprintln!();
    eprintln!("subcommands:");
    eprintln!("  check-headers   verify every .rs file carries the GPL notice");
    eprintln!("  check-boundary  verify references resolve here, and punctuation and vocabulary");
    eprintln!("                  --staged: read the index rather than the working tree");
    eprintln!("  check-changelog verify every version tag has a CHANGELOG.md entry");
    eprintln!("  install-hooks   point git at .githooks/ for this clone");
    eprintln!("  nps             measure the bench's speed the way the SPRT harness measures it");
    eprintln!();
    eprintln!(
        "usage: cargo xtask nps [--binary PATH] [--concurrency N] [--pairs N] [--reference N]"
    );
}

/// Resolved at compile time, so the subcommand works from any directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

fn check_headers() -> ExitCode {
    let root = repo_root();
    let mut files = Vec::new();
    if let Err(e) = collect_rs(&root, &mut files) {
        eprintln!("xtask check-headers: {e}");
        return ExitCode::FAILURE;
    }
    files.sort();

    let mut bad = Vec::new();
    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                if let Some(reason) = header_defect(&text) {
                    bad.push((path.clone(), reason));
                }
            }
            Err(e) => bad.push((path.clone(), format!("unreadable: {e}"))),
        }
    }

    if bad.is_empty() {
        println!("check-headers: {} file(s) OK", files.len());
        return ExitCode::SUCCESS;
    }

    for (path, reason) in &bad {
        let shown = path.strip_prefix(&root).unwrap_or(path);
        eprintln!("{}: {reason}", shown.display());
    }
    eprintln!(
        "\ncheck-headers: {} of {} file(s) missing or misquoting the SPDX tag.",
        bad.len(),
        files.len()
    );
    eprintln!("\nEvery .rs file must open with this exact line:\n");
    eprintln!("{HEADER}");
    ExitCode::FAILURE
}

fn header_defect(text: &str) -> Option<String> {
    match text.lines().next() {
        Some(line) if line == HEADER => None,
        Some(line) => Some(format!("line 1: expected `{HEADER}`, found `{line}`")),
        None => Some("file is empty".to_string()),
    }
}

// ---------------------------------------------------------------------------
// check-boundary
// ---------------------------------------------------------------------------
//
// Every reference must resolve inside this repository. The rules are shapes, so
// nothing private is named here:
//
//   * a `docs/...` path must exist;
//   * the only capitals `NAME.md` are [`ROOT_DOCUMENTS`];
//   * no `/Users/` path;
//   * ASCII outside [`ALLOWED_NON_ASCII`] and [`ALLOWED_IN_PLACE`];
//   * no planning noun with a number;
//   * no `ADR` citation;
//   * no `F` with two or three digits in a comment, squares and `noqa:` codes aside.
//
// It cannot see a wrong use of an allowed character, a hyphen standing in for a
// dash, or a planning reference with no number.

/// A character allowed only in named places.
struct InPlace {
    c: char,
    /// Prefixes, so a directory works.
    paths: &'static [&'static str],
    /// Completes "allowed only in...", so the author reads where it may go.
    allowed_only: &'static str,
}

/// Empty; a row needs a character whose every ASCII spelling is worse.
const ALLOWED_IN_PLACE: &[InPlace] = &[];

fn in_place_allowances(rel: &str) -> Vec<char> {
    ALLOWED_IN_PLACE
        .iter()
        .filter(|entry| entry.paths.iter().any(|p| rel.starts_with(p)))
        .map(|entry| entry.c)
        .collect()
}

fn in_place_entry(c: char) -> Option<&'static InPlace> {
    ALLOWED_IN_PLACE.iter().find(|entry| entry.c == c)
}

/// Notation, and the whole list: anything else non-ASCII fails. `µ` is the micro sign, not
/// Greek mu.
const ALLOWED_NON_ASCII: &[char] = &[
    '\u{d7}',   // × dimensions and products
    '\u{b1}',   // ± error bars on an Elo estimate
    '\u{b5}',   // µ microseconds
    '\u{b7}',   // · field separators
    '\u{2192}', // → mapping
    '\u{21d2}', // ⇒ implication
    '\u{2194}', // ↔ equivalence
    '\u{2264}', // ≤
    '\u{2265}', // ≥
    '\u{2260}', // ≠
    '\u{2286}', // ⊆
    '\u{222a}', // ∪
    '\u{3b1}',  // α SPRT error rate
    '\u{3b2}',  // β SPRT error rate
    '\u{3c3}',  // σ standard deviation
    '\u{394}',  // Δ an evaluation delta
    '\u{2500}', // ─ box drawing
    '\u{2502}', // │
    '\u{251c}', // ├
    '\u{2514}', // └
];

/// A numbered plan step means nothing without the plan. Not searched: `step` (a loop stride),
/// `gate` with a letter (test prose), and references with no number.
const PLANNING_WORDS: &[&str] = &[
    "phase",
    "item",
    "gate",
    "task",
    "batch",
    "milestone",
    "checkpoint",
    "review",
];

/// The evaluation's game phase is a number here, so a plan reference in these files is not
/// caught.
const PLANNING_ALLOWED: &[(&str, &str)] = &[
    ("engine/src/eval.rs", "phase"),
    ("engine/tests/eval.rs", "phase"),
];

/// These spell the citation rules out to enforce them; every other rule still reads them.
const CITATION_RULE_FILES: &[&str] = &[".githooks/check-message-metadata"];

/// Adding one publishes a document, so it lands with the file.
const ROOT_DOCUMENTS: &[&str] = &["README.md", "CHANGELOG.md"];

const fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')
}

/// Sentence punctuation is trimmed from the end.
fn docs_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (start, _) in line.match_indices("docs/") {
        // `cadence/docs/` is rooted here; `xyzdocs/` is another name.
        if line[..start].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let rest = &line[start..];
        let end = rest.find(|c| !is_path_char(c)).unwrap_or(rest.len());
        out.push(rest[..end].trim_end_matches(['.', '/']));
    }
    out
}

fn caps_md_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (at, _) in line.match_indices(".md") {
        if line[at + 3..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        let name_start = line[..at]
            .rfind(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .map_or(0, |i| i + c_len(&line[..at], i));
        let name = &line[name_start..at];
        let bounded =
            name_start == 0 || !line[..name_start].ends_with(|c: char| is_path_char(c) && c != '/');
        if name.len() >= 2 && bounded {
            out.push(&line[name_start..at + 3]);
        }
    }
    out
}

fn c_len(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(1, char::len_utf8)
}

/// Named rather than quoted, and told to go: it stands in for nothing.
struct Invisible {
    name: &'static str,
    advice: &'static str,
}

const DELETE: &str = "nothing: delete it. It is invisible and stands in for nothing, \
                      so there is no ASCII spelling to find";

const PLAIN_SPACE: &str = "an ordinary space. This one is invisible and is not one, \
                           which is why it survived being read";

/// Ranges, because std has no Unicode categories and a chore merits no dependency.
fn invisible(c: char) -> Option<Invisible> {
    let (name, advice) = match c {
        '\u{a0}' => ("a no-break space", PLAIN_SPACE),
        '\u{ad}' => ("a soft hyphen", DELETE),
        '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{1680}' => {
            ("a typographic space", PLAIN_SPACE)
        }
        '\u{200b}' => ("a zero-width space", DELETE),
        '\u{200c}' => ("a zero-width non-joiner", DELETE),
        '\u{200d}' => ("a zero-width joiner", DELETE),
        '\u{200e}' | '\u{200f}' => ("a bidirectional mark", DELETE),
        '\u{2028}' => (
            "a line separator",
            "a newline, which is what ends a line here",
        ),
        '\u{2029}' => ("a paragraph separator", "a blank line"),
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => (
            "a bidirectional override",
            "nothing: delete it. It reorders how the line reads without changing \
             what the line holds, which is the whole of its use",
        ),
        '\u{2060}'..='\u{2064}' => ("a word joiner or invisible operator", DELETE),
        '\u{feff}' => (
            "a byte-order mark",
            "nothing: delete it. UTF-8 needs no mark, and at the head of a source \
             file it also breaks the SPDX header check",
        ),
        '\u{fff9}'..='\u{fffb}' => ("an interlinear annotation mark", DELETE),
        '\u{fffd}' => (
            "a replacement character",
            "whatever the byte was. This is not a character in the file: it is what \
             decoding produced, so the file is not valid UTF-8 at this point",
        ),
        _ => return None,
    };
    Some(Invisible { name, advice })
}

/// The code point is the part that survives a terminal.
fn describe(c: char) -> String {
    match invisible(c) {
        Some(i) => format!("{} (U+{:04X})", i.name, c as u32),
        None => format!("`{c}` (U+{:04X})", c as u32),
    }
}

/// Naming the replacement spares a guess, and for a dash the guess is a hyphen.
fn replacement_for(c: char) -> &'static str {
    match c {
        '\u{2014}' => {
            "whichever of a comma, a colon, a full stop or parentheses the sentence \
             wants. A hyphen substituted everywhere reads worse than any of them"
        }
        '\u{2013}' => "an ASCII hyphen: a range is `150-250 ms`, `rows 8-14`",
        '\u{2018}' | '\u{2019}' => "the ASCII apostrophe",
        '\u{201c}' | '\u{201d}' => "the ASCII quote character",
        '\u{2026}' => "three full stops, `...`",
        '\u{2212}' => "the ASCII hyphen, which is what a minus is here",
        '\u{3bc}' => "U+00B5, the micro sign, which is the spelling on the exempt list",
        '\u{2022}' => "a `-` list marker in prose; in a layout diagram the exempt U+00B7",
        '\u{a7}' => {
            "the word: `section 4`, which is what it abbreviates. Everything in this \
             repository that cites a section says so in words"
        }
        _ => match invisible(c) {
            Some(i) => i.advice,
            None => {
                "an ASCII spelling. If every ASCII spelling is worse, the character \
                 belongs on the exempt list in this file, and putting it there is a \
                 decision someone takes rather than a character that arrives in a commit"
            }
        },
    }
}

/// Once each; `in_place` is resolved once per file.
fn stray_non_ascii(line: &str, in_place: &[char]) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in line.chars() {
        if c.is_ascii() || out.contains(&c) || in_place.contains(&c) {
            continue;
        }
        if !ALLOWED_NON_ASCII.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Not a full stop: "the phase. 3 of them" is a sentence.
const fn is_label_gap(c: char) -> bool {
    matches!(c, ' ' | '\t' | '-' | '_' | ':' | '#')
}

/// Sliced from `line`, so the report quotes the author; ASCII lowercasing keeps offsets.
fn planning_label<'a>(line: &'a str, rel: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    for word in PLANNING_WORDS {
        if PLANNING_ALLOWED
            .iter()
            .any(|(path, allowed)| allowed == word && rel.starts_with(path))
        {
            continue;
        }
        for (at, _) in lower.match_indices(word) {
            // A word boundary before, so `delegate 3` is not `gate 3`.
            if lower[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let after_word = at + word.len();
            let plural = usize::from(lower[after_word..].starts_with('s'));
            let rest = &lower[after_word + plural..];
            let digits = rest.trim_start_matches(is_label_gap);
            if digits.starts_with(|c: char| c.is_ascii_digit()) {
                let gap = rest.len() - digits.len();
                let number = digits
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(digits.len());
                return Some(&line[at..after_word + plural + gap + number]);
            }
        }
    }
    None
}

/// Case-sensitive, because the letters occur inside lower-case words.
fn record_citation(line: &str) -> Option<&str> {
    for (at, _) in line.match_indices("ADR") {
        if line[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let after_word = at + 3;
        let plural = usize::from(line[after_word..].starts_with('s'));
        let rest = &line[after_word + plural..];
        if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            continue;
        }
        let digits = rest.trim_start_matches(['-', ' ']);
        let number = if digits.starts_with(|c: char| c.is_ascii_digit()) {
            let gap = rest.len() - digits.len();
            gap + digits
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(digits.len())
        } else {
            0
        };
        return Some(&line[at..after_word + plural + number]);
    }
    None
}

/// A Markdown or text file is all comment.
fn comment_of<'a>(line: &'a str, rel: &str) -> Option<(usize, &'a str)> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let marker = match ext.as_deref() {
        Some("rs") => "//",
        Some("md" | "txt") => return Some((0, line)),
        Some("py" | "sh" | "toml" | "yml" | "yaml") => "#",
        _ if rel.starts_with(".githooks/") => "#",
        _ => return None,
    };
    line.find(marker).map(|at| (at, &line[at..]))
}

fn finding_citation<'a>(line: &'a str, rel: &str) -> Option<&'a str> {
    let (offset, comment) = comment_of(line, rel)?;
    for (at, _) in comment.match_indices('F') {
        if comment[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &comment[at + 1..];
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if !(2..=3).contains(&digits) {
            continue;
        }
        if rest[digits..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if comment[..at].trim_end().ends_with("noqa:") {
            continue;
        }
        return Some(&line[offset + at..offset + at + 1 + digits]);
    }
    None
}

/// Reading the tree while the index differs would pass an unstaged fix and fail a line not in
/// the commit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    WorkingTree,
    Index,
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `-z`, because `core.quotePath` mangles non-ASCII paths.
fn nul_separated(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn index_paths(root: &Path) -> Result<Vec<String>, String> {
    Ok(nul_separated(&git(root, &["ls-files", "--cached", "-z"])?))
}

/// Only what this commit introduces; CI reads the whole tree.
fn staged_changes(root: &Path) -> Result<Vec<String>, String> {
    if git(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        // The first commit has nothing to diff against.
        return index_paths(root);
    }
    let args = [
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=ACMR",
        "-z",
        "HEAD",
    ];
    Ok(nul_separated(&git(root, &args)?))
}

fn boundary_inputs(root: &Path, source: Source) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    match source {
        Source::WorkingTree => {
            let mut paths = Vec::new();
            collect_all(root, &mut paths).map_err(|e| e.to_string())?;
            paths.sort();
            for path in paths {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let text = std::fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .map_err(|e| format!("{rel_str}: unreadable: {e}"))?;
                out.push((rel_str, text));
            }
        }
        Source::Index => {
            let mut paths = staged_changes(root)?;
            paths.sort();
            for rel in paths {
                let skipped = rel
                    .split('/')
                    .any(|p| SKIP_DIRS.contains(&p) || p == ".DS_Store");
                if skipped {
                    continue;
                }
                let text = String::from_utf8_lossy(&git(root, &["show", &format!(":{rel}")])?)
                    .into_owned();
                out.push((rel, text));
            }
        }
    }
    Ok(out)
}

/// Separate from [`check_boundary`] because clippy refuses one function that long.
fn scan(rel: &str, text: &str, resolves: &impl Fn(&str) -> bool) -> Vec<(usize, String)> {
    let in_place = in_place_allowances(rel);
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let at = i + 1;
        for token in docs_tokens(line) {
            if !token.is_empty() && token != "docs" && !resolves(token) {
                out.push((at, format!("`{token}` does not exist in this repository")));
            }
        }
        for token in caps_md_tokens(line) {
            if !ROOT_DOCUMENTS.contains(&token) {
                out.push((
                    at,
                    format!("`{token}` is not a document in this repository"),
                ));
            }
        }
        if line.contains("/Users/") {
            out.push((at, "contains an absolute `/Users/` path".to_string()));
        }
        for c in stray_non_ascii(line, &in_place) {
            let reason = if let Some(entry) = in_place_entry(c) {
                format!(
                    "{} is allowed only in {}. Here, write {}",
                    describe(c),
                    entry.allowed_only,
                    replacement_for(c)
                )
            } else {
                format!(
                    "{} is not on the exempt list. Write {}",
                    describe(c),
                    replacement_for(c)
                )
            };
            out.push((at, reason));
        }
        if let Some(label) = planning_label(line, rel) {
            out.push((
                at,
                format!(
                    "the planning label `{label}`. Name the condition instead: \
                     what has to be true, not which numbered step it was"
                ),
            ));
        }
        let spells_the_rule = CITATION_RULE_FILES.contains(&rel);
        if let Some(cite) = record_citation(line).filter(|_| !spells_the_rule) {
            out.push((
                at,
                format!(
                    "the record citation `{cite}`. The record is not in this tree, \
                     so write the reason it gave instead"
                ),
            ));
        }
        if let Some(cite) = finding_citation(line, rel).filter(|_| !spells_the_rule) {
            out.push((
                at,
                format!(
                    "the finding citation `{cite}`. The finding is not in this tree, \
                     so write what it found instead"
                ),
            ));
        }
    }
    out
}

/// A whole-tree read of no file would pass anything, so it is a check that did not run; a staged
/// read of none is an empty or deletion-only commit.
fn read_nothing(source: Source, files: usize) -> bool {
    source == Source::WorkingTree && files == 0
}

fn check_boundary(source: Source) -> ExitCode {
    let root = repo_root();
    let inputs = match boundary_inputs(&root, source) {
        Ok(inputs) => inputs,
        Err(e) => {
            eprintln!("xtask check-boundary: {e}");
            return ExitCode::FAILURE;
        }
    };
    if read_nothing(source, inputs.len()) {
        eprintln!(
            "check-boundary: DID NOT RUN: read no file under {}",
            root.display()
        );
        return ExitCode::FAILURE;
    }
    // Against the index in a staged run, because that is what the commit publishes.
    let cached = if source == Source::Index {
        match index_paths(&root) {
            Ok(paths) => Some(paths),
            Err(e) => {
                eprintln!("xtask check-boundary: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let resolves = |token: &str| match &cached {
        Some(paths) => paths
            .iter()
            .any(|p| p == token || p.starts_with(&format!("{token}/"))),
        None => root.join(token).exists(),
    };

    let mut bad = Vec::new();
    for (rel_str, text) in &inputs {
        // This file spells the rules out.
        if rel_str == "xtask/src/main.rs" {
            continue;
        }
        for (line, reason) in scan(rel_str, text, &resolves) {
            bad.push((rel_str.clone(), line, reason));
        }
    }

    let what = match source {
        Source::WorkingTree => "file(s)",
        Source::Index => "staged file(s)",
    };
    if bad.is_empty() {
        println!("check-boundary: {} {what} OK", inputs.len());
        return ExitCode::SUCCESS;
    }
    for (path, line, reason) in &bad {
        eprintln!("{path}:{line}: {reason}");
    }
    eprintln!(
        "\ncheck-boundary: {} problem(s) in {} {what}.",
        bad.len(),
        inputs.len()
    );
    eprintln!("The boundary is one-way, the punctuation is ASCII outside the exempt list in");
    eprintln!("this file, and a deferral names its condition rather than a numbered step.");
    ExitCode::FAILURE
}

struct Heading {
    line: usize,
    label: String,
    date: Option<String>,
}

/// Digits only, so `+1` or an empty part is not a version.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
        } else if in_section && line.split('=').next().map(str::trim) == Some("version") {
            return line.split('"').nth(1).map(str::to_string);
        }
    }
    None
}

/// `[Unreleased]` is checked against nothing, being ahead of every tag.
fn changelog_problems(changelog: &str, tags: &[String], version: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut headings = Vec::new();
    let mut links = Vec::new();
    for (i, line) in changelog.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("## [") {
            let Some((label, after)) = rest.split_once(']') else {
                out.push(format!("line {}: a heading with no closing `]`", i + 1));
                continue;
            };
            let date = after.strip_prefix(" - ").map(str::to_string);
            if date.is_none() && !after.is_empty() {
                out.push(format!(
                    "line {}: `{line}` is not `## [version] - YYYY-MM-DD`",
                    i + 1
                ));
            }
            headings.push(Heading {
                line: i + 1,
                label: label.to_string(),
                date,
            });
        } else if let Some((label, _)) = line.strip_prefix('[').and_then(|r| r.split_once("]: ")) {
            links.push(label.to_string());
        }
    }

    let mut previous: Option<(&Heading, (u64, u64, u64))> = None;
    for (index, h) in headings.iter().enumerate() {
        if !links.contains(&h.label) {
            out.push(format!(
                "line {}: `[{}]` has no link definition at the foot",
                h.line, h.label
            ));
        }
        if h.label == "Unreleased" {
            if index != 0 || h.date.is_some() {
                out.push(format!(
                    "line {}: `[Unreleased]` must be the first heading, with no date",
                    h.line
                ));
            }
            continue;
        }
        let Some(v) = parse_version(&h.label) else {
            out.push(format!(
                "line {}: `{}` is not an x.y.z version",
                h.line, h.label
            ));
            continue;
        };
        match &h.date {
            Some(d) if is_iso_date(d) => {}
            _ => out.push(format!(
                "line {}: `{}` needs a date, `- YYYY-MM-DD`",
                h.line, h.label
            )),
        }
        if let Some((p, pv)) = previous {
            if v >= pv {
                out.push(format!(
                    "line {}: `{}` is not older than `{}` above it",
                    h.line, h.label, p.label
                ));
            } else if h.date > p.date {
                out.push(format!(
                    "line {}: `{}` is dated after `{}` above it",
                    h.line, h.label, p.label
                ));
            }
        }
        if !tags.contains(&h.label) && h.label != version {
            out.push(format!(
                "line {}: `{}` is neither a tag nor the workspace version",
                h.line, h.label
            ));
        }
        previous = Some((h, v));
    }

    let has = |label: &str| headings.iter().any(|h| h.label == label);
    for tag in tags.iter().filter(|t| parse_version(t).is_some()) {
        if !has(tag) {
            out.push(format!("tag `{tag}` has no `## [{tag}]` entry"));
        }
    }
    if !has(version) {
        out.push(format!(
            "the workspace version `{version}` has no `## [{version}]` entry"
        ));
    }
    out
}

/// No tags is a failure, because a shallow checkout fetches none.
fn check_changelog() -> ExitCode {
    let root = repo_root();
    let read =
        |name: &str| std::fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"));
    let problems = (|| -> Result<Vec<String>, String> {
        let changelog = read("CHANGELOG.md")?;
        let version = workspace_version(&read("Cargo.toml")?)
            .ok_or_else(|| "Cargo.toml: no `version` under `[workspace.package]`".to_string())?;
        let tags: Vec<String> = String::from_utf8_lossy(&git(&root, &["tag", "--list"])?)
            .lines()
            .map(str::to_string)
            .collect();
        if tags.is_empty() {
            return Err("git lists no tags, so there is nothing to check against".to_string());
        }
        Ok(changelog_problems(&changelog, &tags, &version))
    })();
    match problems {
        Ok(p) if p.is_empty() => {
            println!("check-changelog: every version tag and the workspace version have an entry");
            ExitCode::SUCCESS
        }
        Ok(p) => {
            for problem in &p {
                eprintln!("CHANGELOG.md: {problem}");
            }
            eprintln!("\ncheck-changelog: {} problem(s).", p.len());
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("xtask check-changelog: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `.git` is skipped as a file too: in a worktree it holds an absolute path.
fn collect_all(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        let name = entry.file_name();
        if ty.is_dir() {
            if SKIP_DIRS.iter().any(|s| *s == name) {
                continue;
            }
            collect_all(&path, out)?;
        } else if ty.is_file() && name != ".DS_Store" && name != ".git" {
            out.push(path);
        }
    }
    Ok(())
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            let name = entry.file_name();
            if SKIP_DIRS.iter().any(|s| *s == name) {
                continue;
            }
            collect_rs(&path, out)?;
        } else if ty.is_file() && path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// nps
// ---------------------------------------------------------------------------
//
// The harness scales each clock by `scale_nps / measured`, so a stale `scale_nps`
// plays every game at the wrong clock unnoticed; this measures it when a test is
// created. A tool, not a gate: the figure depends on the machine as much as the code.
//
// It copies OpenBench's shape (`Client/bench.py` `run_benchmark`,
// `Client/worker.py` `determine_scale_factor`):
//
//   * a round is `concurrency` copies at once, and its figure is their mean;
//   * min, max and spread show contention, which the mean hides;
//   * one round per binary, as `sets=1`;
//   * dev before base, so under `BASE` the divisor is the second, warmed figure,
//     and the flat tail of the pairs is the figure to enter.

struct Round {
    mean_nps: u64,
    min_nps: u64,
    max_nps: u64,
    nodes: u64,
}

struct NpsArgs {
    binary: PathBuf,
    concurrency: usize,
    pairs: usize,
    reference: Option<u64>,
}

fn parse_nps_args(args: &[String]) -> Result<NpsArgs, String> {
    let mut out = NpsArgs {
        binary: repo_root().join("target/release/cadence"),
        concurrency: DEFAULT_CONCURRENCY,
        pairs: DEFAULT_PAIRS,
        reference: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{a} needs a value"))
                .map(String::as_str)
        };
        match a.as_str() {
            "--binary" => out.binary = PathBuf::from(value()?),
            "--concurrency" => {
                out.concurrency = value()?
                    .parse()
                    .map_err(|_| "--concurrency needs a number".to_string())?;
            }
            "--pairs" => {
                out.pairs = value()?
                    .parse()
                    .map_err(|_| "--pairs needs a number".to_string())?;
            }
            "--reference" => {
                let raw = value()?.replace([',', '_'], "");
                out.reference = Some(
                    raw.parse()
                        .map_err(|_| "--reference needs a number".to_string())?,
                );
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if out.concurrency == 0 || out.pairs == 0 {
        return Err("--concurrency and --pairs must be at least 1".to_string());
    }
    Ok(out)
}

fn run_round(binary: &Path, copies: usize) -> Result<Round, String> {
    let children: Vec<_> = (0..copies)
        .map(|_| {
            Command::new(binary)
                .arg("bench")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("could not run {}: {e}", binary.display()))
        })
        .collect::<Result<_, _>>()?;

    let mut nps = Vec::with_capacity(copies);
    let mut nodes = Vec::with_capacity(copies);
    for child in children {
        let out = child
            .wait_with_output()
            .map_err(|e| format!("bench did not finish: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let last = text
            .lines()
            .last()
            .ok_or_else(|| "bench printed nothing".to_string())?;
        // `<nodes> nodes <nps> nps`, the line CI and OpenBench both parse.
        let f: Vec<&str> = last.split_whitespace().collect();
        if f.len() != 4 || f[1] != "nodes" || f[3] != "nps" {
            return Err(format!(
                "last line is not `<nodes> nodes <nps> nps`: {last:?}"
            ));
        }
        nodes.push(
            f[0].parse::<u64>()
                .map_err(|_| format!("bad node count in {last:?}"))?,
        );
        nps.push(
            f[2].parse::<u64>()
                .map_err(|_| format!("bad nps in {last:?}"))?,
        );
    }
    if nodes.windows(2).any(|w| w[0] != w[1]) {
        return Err(format!(
            "the copies of one round disagreed on the node count ({nodes:?}); \
             the harness refuses a workload for this with `Non-Deterministic Benches`"
        ));
    }
    let mean_nps = nps.iter().sum::<u64>() / copies as u64;
    let min_nps = *nps.iter().min().expect("copies is at least one");
    let max_nps = *nps.iter().max().expect("copies is at least one");
    Ok(Round {
        mean_nps,
        min_nps,
        max_nps,
        nodes: nodes[0],
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "nps spread is displayed, and decides nothing here"
)]
fn round_spread_percent(round: &Round) -> f64 {
    if round.mean_nps == 0 {
        return 0.0;
    }
    100.0 * (round.max_nps - round.min_nps) as f64 / round.mean_nps as f64
}

fn print_preamble(args: &NpsArgs) {
    println!(
        "{} copies at once, {} pairs, the shape the OpenBench worker measures in:",
        args.concurrency, args.pairs
    );
    println!("OpenBench uses the mean within one concurrent round.");
    println!(
        "Across pairs, the flat warm tail supplies scale_nps; the aggregate mean is descriptive."
    );
    println!("Min, max and spread within a round expose contention for the concurrency choice.");
    println!("One round per binary, dev first and base second; under `scale_method = BASE`");
    println!("each candidate is the second round's mean.\n");
    println!(
        "{:>5}  {:<13}  {:>10}  {:>10}  {:>10}  {:>7}   nodes",
        "pair", "round", "mean", "min", "max", "spread"
    );
}

#[expect(
    clippy::cast_precision_loss,
    reason = "nps is displayed, and decides nothing here"
)]
fn print_round_row(pair: usize, label: &str, round: &Round, spread: f64) {
    println!(
        "{pair:>5}  {label:<13}  {:>9.3} M  {:>9.3} M  {:>9.3} M  {spread:>6.1}%   {}",
        round.mean_nps as f64 / 1e6,
        round.min_nps as f64 / 1e6,
        round.max_nps as f64 / 1e6,
        round.nodes
    );
}

#[expect(
    clippy::cast_precision_loss,
    reason = "nps and time controls are displayed, and decide nothing here"
)]
fn nps(args: &[String]) -> ExitCode {
    let args = match parse_nps_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xtask nps: {e}");
            usage();
            return ExitCode::FAILURE;
        }
    };
    if !args.binary.is_file() {
        eprintln!("xtask nps: {} does not exist.", args.binary.display());
        eprintln!();
        eprintln!("The figure is a property of the release profile, so build it first:");
        eprintln!("    cargo build --release -p cadence-engine");
        eprintln!("or point this at the binary the scaling will divide by, which for a");
        eprintln!("test against an earlier version is that version's own build:");
        eprintln!("    cargo xtask nps --binary <path to that build>/cadence");
        return ExitCode::FAILURE;
    }

    print_preamble(&args);

    let mut divisors = Vec::with_capacity(args.pairs);
    let mut copy_spreads = Vec::with_capacity(args.pairs * 2);
    // Across rounds as well as within, because the closing line claims both.
    let mut nodes: Option<u64> = None;
    for pair in 1..=args.pairs {
        let round = || match run_round(&args.binary, args.concurrency) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("\nxtask nps: {e}");
                None
            }
        };
        let (Some(first), Some(second)) = (round(), round()) else {
            return ExitCode::FAILURE;
        };
        for seen in [first.nodes, second.nodes] {
            match nodes {
                None => nodes = Some(seen),
                Some(n) if n == seen => {}
                Some(n) => {
                    eprintln!(
                        "\nxtask nps: rounds disagreed on the node count ({n} then {seen}). \
                         The same binary must count the same nodes every time; this is a \
                         determinism fault, not a slow machine."
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
        divisors.push(second.mean_nps);
        let first_spread = round_spread_percent(&first);
        let second_spread = round_spread_percent(&second);
        copy_spreads.extend([first_spread, second_spread]);
        print_round_row(pair, "first", &first, first_spread);
        print_round_row(pair, "second (BASE)", &second, second_spread);
    }

    let mut sorted = divisors.clone();
    sorted.sort_unstable();
    let mean = divisors.iter().sum::<u64>() / divisors.len() as u64;
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 0 {
        u64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    };
    let (lo, hi) = (sorted[0], sorted[sorted.len() - 1]);

    let nodes = nodes.unwrap_or_default();
    println!(
        "\n{} nodes, in every copy of every round, from {}",
        nodes,
        args.binary.display()
    );
    println!(
        "BASE-round summary over {} pairs: mean {:.2} M, median {:.2} M, range {:.2}-{:.2} M",
        args.pairs,
        mean as f64 / 1e6,
        median as f64 / 1e6,
        lo as f64 / 1e6,
        hi as f64 / 1e6
    );
    let worst_copy_spread = copy_spreads.into_iter().fold(0.0_f64, f64::max);
    println!(
        "Concurrency: worst min-to-max spread within the {} rounds: {:.1}%",
        args.pairs * 2,
        worst_copy_spread
    );
    println!("\n  scale_nps selection:");
    println!(
        "  Enter the flat tail of the BASE rows: the last few pair means agreeing to about\n  \
         one per cent. Do not enter the aggregate mean above if the sequence is still\n  \
         falling; cold rows read high and would deliver a longer clock than nominal. If\n  \
         the tail has not flattened, run more pairs. Record the selected tail figure,\n  \
         binary and machine in the SPRT record."
    );

    if let Some(reference) = args.reference {
        report_against_reference(reference, mean, lo, hi);
    }
    ExitCode::SUCCESS
}

#[expect(
    clippy::cast_precision_loss,
    reason = "nps and time controls are displayed, and decide nothing here"
)]
fn report_against_reference(reference: u64, mean: u64, lo: u64, hi: u64) {
    println!("\nAgainst a reference of {reference}:");
    let factor = |m: u64| reference as f64 / m as f64;
    println!(
        "  factor {:.3} to {:.3} across the pairs measured",
        factor(hi),
        factor(lo)
    );
    let f = factor(mean);
    for (base, inc) in [(8.0_f64, 0.08_f64), (40.0, 0.4)] {
        println!(
            "  a nominal {base:>4.1}+{inc:<4.2} is delivered as {:>6.2}+{:.3}",
            base * f,
            inc * f
        );
    }
    if !(0.95..=1.05).contains(&f) {
        println!(
            "\n  That reference is {:.0}% away from what this binary measures on this\n  \
             machine, so a test carrying it plays at the clock above rather than the\n  \
             nominal one. Whether that matters depends on where the figure came from:\n  \
             a stored Test.scale_nps is what the worker really divided by, while the\n  \
             engine config's nps is only the pre-fill a browser offers.",
            (f - 1.0).abs() * 100.0
        );
    }
}

/// `core.hooksPath`, not copies, so an edited hook takes effect at once. Hooks are not cloned.
fn install_hooks() -> ExitCode {
    let root = repo_root();
    let hooks = root.join(".githooks");
    if !hooks.is_dir() {
        eprintln!("install-hooks: {} does not exist", hooks.display());
        return ExitCode::FAILURE;
    }

    let status = Command::new("git")
        .current_dir(&root)
        .args(["config", "core.hooksPath", ".githooks"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("install-hooks: core.hooksPath = .githooks");
            for hook in HOOKS {
                let p = hooks.join(hook);
                println!(
                    "  {} {}",
                    if p.is_file() { "ok  " } else { "MISSING" },
                    hook
                );
            }
            ExitCode::SUCCESS
        }
        Ok(s) => {
            eprintln!("install-hooks: git config exited with {s}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("install-hooks: could not run git: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_check_names_what_it_did_not_examine() {
        let lists: [&[&str]; 4] = [
            HEADERS_NOT_EXAMINED,
            BOUNDARY_NOT_EXAMINED,
            &[STAGED_NOT_EXAMINED],
            CHANGELOG_NOT_EXAMINED,
        ];
        for spots in lists {
            assert!(!spots.is_empty(), "a check with nothing listed");
            for spot in spots {
                assert!(
                    !spot.trim().is_empty() && spot.is_ascii(),
                    "an empty or non-ASCII spot: {spot:?}"
                );
            }
        }
    }

    /// `cargo test --workspace` does not reach this crate; CI runs these from its own manifest.
    const NOT_ALLOWED: &str = "core/src/lib.rs";

    /// In a worktree `.git` is a file holding an absolute path.
    #[test]
    fn a_whole_tree_read_of_no_file_is_refused_and_an_empty_commit_is_not() {
        assert!(read_nothing(Source::WorkingTree, 0));
        assert!(!read_nothing(Source::WorkingTree, 1));
        assert!(!read_nothing(Source::Index, 0));
    }

    #[test]
    fn a_worktrees_dot_git_file_is_not_collected() {
        let dir = std::env::temp_dir().join(format!("cadence-xtask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory to walk");
        std::fs::write(
            dir.join(".git"),
            "gitdir: /Users/someone/git/clone/.git/worktrees/w\n",
        )
        .expect("the worktree marker");
        std::fs::write(dir.join("kept.rs"), "// a file the walk keeps\n").expect("a kept file");

        let mut found = Vec::new();
        collect_all(&dir, &mut found).expect("the walk");
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            names.iter().any(|n| n == "kept.rs"),
            "walked nothing: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == ".git"),
            "a worktree's .git file was collected and the boundary scan would read it: {names:?}"
        );
    }

    #[test]
    fn the_exempt_list_is_a_list_of_non_ascii_characters_once_each() {
        for c in ALLOWED_NON_ASCII {
            assert!(!c.is_ascii(), "{c} is ASCII and does not belong here");
            assert_eq!(
                ALLOWED_NON_ASCII.iter().filter(|o| *o == c).count(),
                1,
                "{c} is listed twice"
            );
        }
    }

    /// A character on both lists would make its paths dead text.
    #[test]
    fn a_character_is_allowed_everywhere_or_in_named_places_and_not_both() {
        for entry in ALLOWED_IN_PLACE {
            assert!(!entry.c.is_ascii(), "{} is ASCII", entry.c);
            assert!(
                !ALLOWED_NON_ASCII.contains(&entry.c),
                "{} is on both lists, so its paths decide nothing",
                entry.c
            );
            assert!(
                !entry.paths.is_empty(),
                "{} is allowed in no place, which is a ban written as an exception",
                entry.c
            );
            assert!(!entry.allowed_only.is_empty(), "{} says nowhere", entry.c);
            assert_eq!(
                ALLOWED_IN_PLACE.iter().filter(|o| o.c == entry.c).count(),
                1,
                "{} has two rows, and only the first would be read",
                entry.c
            );
        }
    }

    #[test]
    fn exempt_notation_passes_and_typography_does_not() {
        let notation: String = ALLOWED_NON_ASCII.iter().collect();
        assert!(stray_non_ascii(&notation, &[]).is_empty());
        // The usual arrivals, and Greek mu.
        for bad in [
            "a dash \u{2014}",
            "a \u{201c}quote\u{201d}",
            "an ellipsis\u{2026}",
            "12 \u{3bc}s",
        ] {
            assert!(!stray_non_ascii(bad, &[]).is_empty(), "{bad} passed");
        }
    }

    /// Over the paths that used to admit something, so it says something once a row exists.
    #[test]
    fn an_empty_table_admits_nothing_anywhere() {
        let retired = ["\u{a7}", "\u{2014}"];
        for path in [
            "docs/testing/perft.md",
            "core/tests/support/mod.rs",
            "engine/tests/support/mod.rs",
            "tests/fixtures/perft-corpus.txt",
            "core/src/movegen.rs",
            "README.md",
        ] {
            assert!(
                in_place_allowances(path).is_empty(),
                "{path} allows something"
            );
            for c in retired {
                assert_eq!(
                    stray_non_ascii(c, &in_place_allowances(path)),
                    vec![c.chars().next().expect("one char")],
                    "{c} passed in {path}"
                );
            }
        }
    }

    /// The refused name is invented: a real one would inventory what is withheld.
    #[test]
    fn root_documents_pass_and_other_capitals_md_do_not() {
        for name in ROOT_DOCUMENTS {
            let out = scan("README.md", &format!("See {name} for more."), &|_| true);
            assert!(out.is_empty(), "{name}: {out:?}");
        }
        let out = scan("README.md", "See NOTES_X.md for more.", &|_| true);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].1.contains("is not a document"), "{}", out[0].1);
    }

    /// The report's advice is what makes refusing the sign no loss.
    #[test]
    fn the_citation_sign_is_now_refused_everywhere_and_told_to_be_a_word() {
        for path in ["core/src/movegen.rs", "docs/testing/perft.md"] {
            let out = scan(path, "// \u{a7}4 has the DFRC arrays", &|_| true);
            assert_eq!(out.len(), 1, "{path}");
            assert!(
                out[0].1.contains("is not on the exempt list"),
                "{}",
                out[0].1
            );
            assert!(out[0].1.contains("section 4"), "{}", out[0].1);
        }
    }

    #[test]
    fn every_invisible_that_actually_arrives_is_named() {
        for (c, expected) in [
            ('\u{a0}', "no-break space"),
            ('\u{ad}', "soft hyphen"),
            ('\u{200b}', "zero-width space"),
            ('\u{200c}', "zero-width non-joiner"),
            ('\u{200d}', "zero-width joiner"),
            ('\u{2060}', "word joiner"),
            ('\u{feff}', "byte-order mark"),
            ('\u{202e}', "bidirectional override"),
            ('\u{2066}', "bidirectional override"),
            ('\u{2009}', "typographic space"),
            ('\u{2028}', "line separator"),
            ('\u{fffd}', "replacement character"),
        ] {
            let named = invisible(c).unwrap_or_else(|| panic!("U+{:04X} unnamed", c as u32));
            assert!(named.name.contains(expected), "U+{:04X}", c as u32);
            // Named, not quoted: an author cannot see the character itself.
            let shown = describe(c);
            assert!(shown.contains(expected) && !shown.contains('`'), "{shown}");
            assert!(shown.contains(&format!("U+{:04X}", c as u32)), "{shown}");
        }
    }

    #[test]
    fn an_invisible_character_is_told_to_go_and_a_visible_one_to_be_replaced() {
        // Standing in for nothing: delete it.
        for c in ['\u{200b}', '\u{200d}', '\u{ad}', '\u{2060}'] {
            assert!(replacement_for(c).starts_with("nothing: delete it"));
        }
        // Standing in for a space: write the space.
        assert!(replacement_for('\u{a0}').starts_with("an ordinary space"));
        // Visible, so an ASCII spelling is the right ask, and it is quoted.
        assert!(describe('\u{201c}').contains('`'));
        assert!(replacement_for('\u{201c}').contains("ASCII quote"));
        assert!(describe('\u{2014}').contains('`'));
    }

    /// An allowed character with no glyph would say nothing.
    #[test]
    fn nothing_on_either_allowed_list_is_invisible() {
        for c in ALLOWED_NON_ASCII {
            assert!(
                invisible(*c).is_none(),
                "U+{:04X} is exempt and invisible",
                *c as u32
            );
        }
        for entry in ALLOWED_IN_PLACE {
            assert!(
                invisible(entry.c).is_none(),
                "{} is allowed and invisible",
                entry.c
            );
        }
    }

    #[test]
    fn one_report_per_character_however_often_it_appears() {
        assert_eq!(stray_non_ascii("\u{201c}a\u{201c}b\u{201c}", &[]).len(), 1);
    }

    #[test]
    fn planning_labels_are_caught_in_the_forms_they_are_written_in() {
        for line in [
            "// Phase 3: add gives_check()",
            "fn phase1_perft_startpos() {}",
            "// item 8 owns the bound",
            "// gate 4",
            "// checkpoint-2",
            "// milestone_2",
            "// batch #7",
            "// tasks 9",
            "// Review 2026-08-25 finding 2.3",
        ] {
            assert!(planning_label(line, NOT_ALLOWED).is_some(), "{line} passed");
        }
    }

    #[test]
    fn record_citations_are_caught_in_the_forms_they_are_written_in() {
        for (line, cite) in [
            (
                "// Below the null move, which is ADR-0008's order",
                "ADR-0008",
            ),
            ("// as ADR 3 says", "ADR 3"),
            ("/// ADR0010 is the ruling", "ADR0010"),
            ("// the ADRs agree on this", "ADRs"),
            ("the reasoning is the one ADR-0002 asks for", "ADR-0002"),
        ] {
            assert_eq!(record_citation(line), Some(cite), "{line}");
        }
    }

    #[test]
    fn words_that_contain_the_letters_are_not_record_citations() {
        for line in [
            "// a quadratic term",
            "let adr = 1;",
            "// ADRESS is not a word this tree uses",
            "// MADR is not a citation either",
        ] {
            assert_eq!(record_citation(line), None, "{line}");
        }
    }

    #[test]
    fn finding_citations_are_caught_in_the_comments_they_are_written_in() {
        for (rel, line, cite) in [
            (
                "engine/tests/tune.rs",
                "/// that can be gated here, and F916 carries it.",
                "F916",
            ),
            ("engine/src/x.rs", "let a = 1; // see F942", "F942"),
            ("tools/x.py", "x = 1  # F910 has the instance", "F910"),
            (
                ".githooks/commit-msg",
                "# F951 is why this is resolved from git",
                "F951",
            ),
            ("README.md", "as F960 records", "F960"),
        ] {
            assert_eq!(finding_citation(line, rel), Some(cite), "{rel}: {line}");
        }
    }

    #[test]
    fn squares_lint_codes_and_code_are_not_finding_citations() {
        for (rel, line) in [
            // Squares are one digit.
            (
                "core/src/types.rs",
                "    A1 = 0, B1 = 1, C1 = 2, D1 = 3, E1 = 4, F1 = 5,",
            ),
            (
                "core/tests/a.rs",
                "assert_eq!(b.attackers_to(Square::F5, occ), x); // F5 is a square",
            ),
            // A lint code.
            (
                "openbench/overlay/CadenceSite/settings.py",
                "from OpenSite.settings import *  # noqa: F403",
            ),
            // Code, and a hex constant, are not comments.
            ("engine/src/x.rs", "let f = Square::F906;"),
            ("engine/src/x.rs", "// the mask is 0xF16"),
            // A file whose comments this check cannot find.
            ("engine/bench_positions.txt.json", "F916"),
        ] {
            assert_eq!(finding_citation(line, rel), None, "{rel}: {line}");
        }
    }

    #[test]
    fn a_planted_citation_fails_the_scan_beside_the_other_rules() {
        let out = scan(
            "engine/src/search/node.rs",
            "// Below the null move, which is ADR-0008's order.\n// F916 carries it.\n",
            &|_| true,
        );
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].1.contains("ADR-0008"), "{out:?}");
        assert!(out[1].1.contains("F916"), "{out:?}");
    }

    #[test]
    fn the_message_gate_may_spell_the_citation_rules_and_nothing_else() {
        let gate = scan(
            ".githooks/check-message-metadata",
            "# ADR and F916 are refused\n",
            &|_| true,
        );
        assert!(gate.is_empty(), "{gate:?}");
        let other = scan(
            ".githooks/commit-msg",
            "# ADR and F916 are refused\n",
            &|_| true,
        );
        assert_eq!(other.len(), 2, "{other:?}");
    }

    #[test]
    fn the_report_quotes_what_the_author_wrote() {
        assert_eq!(
            planning_label("// Phase 3: add x", NOT_ALLOWED),
            Some("Phase 3")
        );
        assert_eq!(
            planning_label("// tasks 9 remain", NOT_ALLOWED),
            Some("tasks 9")
        );
        assert_eq!(
            planning_label("fn phase1_x() {}", NOT_ALLOWED),
            Some("phase1")
        );
    }

    #[test]
    fn ordinary_prose_and_this_tree_s_own_vocabulary_pass() {
        for line in [
            // The word boundary: both of these end in `gate`.
            "// delegate 3 things",
            "// aggregate 5 rows",
            // `gate` is this tree's commonest test noun.
            "//! The gate for `features`: the train/play contract, stated as data.",
            "// the sign-off gate is recorded against these two values",
            // Why `step` is not searched at all.
            "// Step 2: the side to move is in the key",
            // A planning noun with no number: invisible, and stated as such.
            "// the next phase, once something reads it",
            "// the review said so",
            "// revisit when the TT lands",
        ] {
            assert_eq!(
                planning_label(line, NOT_ALLOWED),
                None,
                "{line} was flagged"
            );
        }
    }

    #[test]
    fn the_tapered_evaluation_owns_the_word_phase_in_its_own_two_files() {
        let line = r#"assert!(ending >= 200, "only {ending} positions at phase 0");"#;
        assert!(planning_label(line, NOT_ALLOWED).is_some());
        assert_eq!(planning_label(line, "engine/tests/eval.rs"), None);
        assert_eq!(planning_label(line, "engine/src/eval.rs"), None);
        // The exemption is per word, not per file: the file is still checked.
        assert!(planning_label("// item 8", "engine/src/eval.rs").is_some());
    }

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    const GOOD: &str = "\
# Changelog

## [Unreleased]

- Something not yet in a version.

## [0.2.0] - 2026-02-01

## [0.1.0] - 2026-01-01

[Unreleased]: https://example.invalid/compare/0.2.0...main
[0.2.0]: https://example.invalid/compare/0.1.0...0.2.0
[0.1.0]: https://example.invalid/tree/0.1.0
";

    #[test]
    fn a_changelog_with_every_tag_passes_and_unreleased_is_not_a_version() {
        assert_eq!(
            changelog_problems(GOOD, &tags(&["0.1.0", "0.2.0"]), "0.2.0"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_tag_with_no_entry_fails() {
        let out = changelog_problems(GOOD, &tags(&["0.1.0", "0.2.0", "0.2.1"]), "0.2.0");
        assert_eq!(
            out,
            vec!["tag `0.2.1` has no `## [0.2.1]` entry".to_string()]
        );
    }

    #[test]
    fn a_cut_fails_until_its_entry_exists_and_passes_before_its_tag() {
        let out = changelog_problems(GOOD, &tags(&["0.1.0", "0.2.0"]), "0.2.1");
        assert_eq!(
            out,
            vec!["the workspace version `0.2.1` has no `## [0.2.1]` entry".to_string()]
        );
        let cut = GOOD
            .replace("## [0.2.0]", "## [0.2.1] - 2026-03-01\n\n## [0.2.0]")
            .replace("[0.2.0]: ", "[0.2.1]: https://example.invalid/x\n[0.2.0]: ");
        assert_eq!(
            changelog_problems(&cut, &tags(&["0.1.0", "0.2.0"]), "0.2.1"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_entry_for_a_version_that_is_neither_tagged_nor_current_fails() {
        let out = changelog_problems(GOOD, &tags(&["0.1.0"]), "0.1.0");
        assert_eq!(
            out,
            vec!["line 7: `0.2.0` is neither a tag nor the workspace version".to_string()]
        );
    }

    #[test]
    fn order_dates_links_and_the_place_of_unreleased_are_checked() {
        let swapped = GOOD.replace("## [0.2.0] - 2026-02-01", "## [0.0.9] - 2026-02-01");
        assert!(
            changelog_problems(&swapped, &tags(&["0.1.0", "0.0.9"]), "0.1.0")
                .iter()
                .any(|p| p.contains("is not older than"))
        );
        let undated = GOOD.replace("## [0.1.0] - 2026-01-01", "## [0.1.0]");
        assert!(
            changelog_problems(&undated, &tags(&["0.1.0", "0.2.0"]), "0.2.0")
                .iter()
                .any(|p| p.contains("needs a date"))
        );
        let unlinked = GOOD.replace("[0.1.0]: https://example.invalid/tree/0.1.0\n", "");
        assert!(
            changelog_problems(&unlinked, &tags(&["0.1.0", "0.2.0"]), "0.2.0")
                .iter()
                .any(|p| p.contains("no link definition"))
        );
        let late = GOOD
            .replace("## [Unreleased]\n", "")
            .replace("[0.1.0]: https", "## [Unreleased]\n\n[0.1.0]: https");
        assert!(
            changelog_problems(&late, &tags(&["0.1.0", "0.2.0"]), "0.2.0")
                .iter()
                .any(|p| p.contains("must be the first heading"))
        );
    }

    #[test]
    fn the_workspace_version_is_read_from_its_own_section_only() {
        let manifest =
            "[package]\nversion = \"9.9.9\"\n\n[workspace.package]\nversion      = \"0.5.4\"\n";
        assert_eq!(workspace_version(manifest).as_deref(), Some("0.5.4"));
        assert_eq!(workspace_version("[package]\nversion = \"1.0.0\"\n"), None);
    }

    #[test]
    fn a_version_is_three_runs_of_digits() {
        assert_eq!(parse_version("0.4.10"), Some((0, 4, 10)));
        for bad in ["0.4", "0.4.1.2", "0.+4.1", "v0.4.1", "0..1", "Unreleased"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }
}
