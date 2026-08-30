// SPDX-License-Identifier: GPL-3.0-or-later

//! Repository chores. Not a workspace member and not shipped.
//!
//! Run via the `cargo xtask <subcommand>` alias defined in
//! `.cargo/config.toml`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// The SPDX identifier every `.rs` file in the repository must open with.
///
/// One line, and it must be the first line of the file. The copyright notice
/// is deliberately not here: repeating it in every file means a name or a
/// year change is a repository-wide edit, and SPDX exists precisely so that
/// per-file licensing is a machine-readable tag rather than a wall of prose.
/// The notice itself lives in `LICENSE` and in `README.md`.
const HEADER: &str = "// SPDX-License-Identifier: GPL-3.0-or-later";

/// Directory names never descended into: version control and build
/// artefacts (`__pycache__` embeds the absolute paths of the machine that
/// compiled it, which check-boundary would otherwise flag).
const SKIP_DIRS: &[&str] = &[".git", "target", "__pycache__"];

/// The hooks `install-hooks` expects to find in `.githooks/`.
const HOOKS: &[&str] = &["commit-msg", "pre-commit"];

/// Concurrent copies to run at once: the `-T` an SPRT worker is started with.
/// The bench has to be measured at the worker's own concurrency because that
/// is the state the worker measures in, and the figure is
/// concurrency-dependent.
///
/// Six is the default because it is what the worker these figures are taken
/// for was started with. It is a property of a machine and not of this
/// repository, so a different one is measured with `--concurrency` rather
/// than by editing this.
const DEFAULT_CONCURRENCY: usize = 6;

/// Pairs of rounds to run. Each pair is one dev bench followed by one base
/// bench, which is what the worker does before every workload.
const DEFAULT_PAIRS: usize = 5;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("check-headers") => check_headers(),
        Some("check-boundary") => {
            let rest: Vec<String> = args.collect();
            match rest
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                [] => check_boundary(Source::WorkingTree),
                ["--staged"] => check_boundary(Source::Index),
                _ => {
                    eprintln!("xtask check-boundary: expected no argument or `--staged`");
                    ExitCode::FAILURE
                }
            }
        }
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

fn usage() {
    eprintln!("usage: cargo xtask <subcommand>");
    eprintln!();
    eprintln!("subcommands:");
    eprintln!("  check-headers   verify every .rs file carries the GPL notice");
    eprintln!("  check-boundary  verify references resolve here, and punctuation and vocabulary");
    eprintln!("                  --staged: read the index rather than the working tree");
    eprintln!("  install-hooks   point git at .githooks/ for this clone");
    eprintln!("  nps             measure the bench's speed the way the SPRT harness measures it");
    eprintln!();
    eprintln!(
        "usage: cargo xtask nps [--binary PATH] [--concurrency N] [--pairs N] [--reference N]"
    );
}

/// The repository root, resolved at compile time so the subcommand works
/// from any working directory.
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

/// `None` if the header is correct, otherwise a one-line description of what
/// is wrong with it.
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
// Every reference in this repository must resolve inside it. The tree
// builds, tests and explains itself from a clean clone, and a comment that
// cites a document that is not here hands the reader a pointer to nothing.
// The cleanup that removed the last such references touched 44 lines across
// 15 files, which is the proof that the habit of writing them is real and
// will write them again; a rule enforced by memory erodes, so this one runs
// beside check-headers, in CI and in `.githooks/pre-commit`.
//
// The rules are deliberately SHAPES, not names, so there is no list of
// outside documents to maintain here -- or to read:
//
//   * A path-shaped `docs/...` token must resolve to a file or directory
//     that exists in this repository. A citation of a document that is not
//     here fails, whatever it is called, and the rule maintains itself as
//     documents come and go.
//   * `NAME.md` where NAME is all capitals is the root-document convention,
//     and the only such document in this tree is `README.md`. Any other is
//     a reference to a document that is not here.
//   * No absolute `/Users/` path: a home path names a machine, not the
//     repository.
//   * No character outside ASCII except the ones on [`ALLOWED_NON_ASCII`],
//     which is the exhaustive notation list, or the ones on
//     [`ALLOWED_IN_PLACE`] inside the paths that row names. That table is
//     currently empty, so for now the rule is simply: outside the notation
//     list, ASCII.
//   * No planning label: a planning noun with a number attached. Name the
//     condition, not the phase.
//
// The last two were rules enforced by attention until 2026-08-25, and three
// punctuation violations reached a tree in one week, each caught by somebody
// noticing. What a person can catch by noticing, a person can miss by not.
//
// WHAT THESE TWO CANNOT CATCH, because a check whose limits are unstated is
// the failure this project keeps recording:
//
//   * The punctuation rule is a character test, so it cannot see a *use* of an
//     exempt character that is wrong: a `×` standing in for the word "by", a
//     `→` in prose that wanted "becomes". For a row of [`ALLOWED_IN_PLACE`] it
//     would check the file and not the sentence, so a character inside one of
//     that row's paths, doing there the wrong thing it is admitted for, would
//     still pass. It also cannot see the punctuation the rule is really about,
//     which is a hyphen substituted for whatever it replaced; that reads worse
//     and is pure ASCII. And the exempt list was
//     verified against the tree once, when the rule landed. This check keeps
//     the tree inside the list; it does not re-verify that the list is right.
//   * The vocabulary rule is a keyword search over a tree whose domain
//     vocabulary overlaps it, so it is scoped down to what it can assert
//     without lying. See [`PLANNING_WORDS`] for the words that are searched,
//     [`PLANNING_ALLOWED`] for the two files where a planning noun is a
//     domain term, and the note on both for the forms deliberately not
//     searched. In particular: a planning reference carrying no number
//     ("the next phase", "the review said") is invisible to it, and that is
//     the majority of ways to write one.

/// A character allowed only in named places: the paths where it is permitted,
/// and what it is doing there.
///
/// One row per character, so admitting another is a row rather than a new
/// mechanism. [`ALLOWED_IN_PLACE`] is the table.
struct InPlace {
    /// The character.
    c: char,
    /// Path prefixes where it is allowed. A prefix, so a directory works.
    paths: &'static [&'static str],
    /// What it is doing in those places, phrased to complete "allowed only
    /// in...". This is what the author reads when the check fires, so it says
    /// where the character may go rather than restating that it may not go
    /// here.
    allowed_only: &'static str,
}

/// The characters that are permitted somewhere and not everywhere.
///
/// **Empty, and deliberately still here.** Both rows it once held were retired
/// on 2026-08-25, in the same order they were argued: the corpus marked an
/// absent node count with a character that needed an exception across three
/// files, and that marker is now an empty cell; the citations were written with
/// a character that needed another, and they are now written "section 4". Each
/// removal made the punctuation rule shorter to state rather than longer.
///
/// The test a row has to pass is whether an ASCII spelling exists that is not
/// worse. Neither of those two could meet it once the question was put: an
/// empty table cell is not worse than a dash meaning "no value", and "section
/// 4" is not worse than a sign meaning "section". What is left, if a row is
/// ever added, is a character with no ASCII spelling at all and a use confined
/// to named files. Keeping the mechanism costs a line and an empty slice;
/// rebuilding it would cost the argument again.
const ALLOWED_IN_PLACE: &[InPlace] = &[];

/// The characters from [`ALLOWED_IN_PLACE`] that `rel` is one of the places
/// for.
fn in_place_allowances(rel: &str) -> Vec<char> {
    ALLOWED_IN_PLACE
        .iter()
        .filter(|entry| entry.paths.iter().any(|p| rel.starts_with(p)))
        .map(|entry| entry.c)
        .collect()
}

/// The entry for `c`, if it is a character allowed only in named places.
fn in_place_entry(c: char) -> Option<&'static InPlace> {
    ALLOWED_IN_PLACE.iter().find(|entry| entry.c == c)
}

/// Every non-ASCII character this repository permits anywhere, and the whole
/// list. The ones permitted only somewhere are [`ALLOWED_IN_PLACE`].
///
/// These are notation rather than punctuation: dimensions and products, error
/// bars, microseconds, the field separator in the layout diagrams and the CI
/// summary line, the mapping and implication arrows, the order and set
/// relations, the Greek letters the SPRT bounds and an evaluation delta use,
/// and the four box-drawing characters the directory trees are made of.
///
/// The list being exhaustive is what makes it a check rather than a
/// judgement: a character that is neither ASCII nor here is a violation, and
/// nobody has to decide. Two consequences worth knowing before adding to it.
/// `µ` is U+00B5, the micro sign, and the Greek mu U+03BC is a different
/// character that this list does not contain; one symbol with two spellings
/// is exactly what an exhaustive list is for. And the box-drawing set is the
/// four characters the trees actually use, so a tree drawn with `┌` or `┬`
/// fails until somebody decides those belong here too.
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

/// The planning nouns that must not appear with a number attached.
///
/// Planning vocabulary is meaningless to a reader without the plan in hand,
/// including its author a year later, and a comment naming a numbered future
/// step is a comment that goes wrong the moment the plan changes, with
/// nothing to force its update. The rule is to name the condition instead.
///
/// **Scoped down, three times, and each cut is a thing this does not catch.**
/// `step` is not searched: `core/src/position.rs` uses "Step 2" for the
/// stride of a loop, which is the ordinary technical sense and the more
/// common one in engine code. `gate` is searched with a digit but not with a
/// letter, so gate letters ("gate A") pass: "gate" is one of the most common
/// words in this tree's test prose, and `gate` followed by a single capital
/// is indistinguishable from a sentence that happens to continue "gate A
/// stranger receives". And nothing here catches a planning reference without
/// a number, which is most of them.
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

/// Where a word in [`PLANNING_WORDS`] is a domain term rather than a plan
/// reference, as (path prefix, word) pairs.
///
/// The tapered evaluation's game phase is an integer on `0..=PHASE_MAX`, so
/// "phase 0" in these two files means the value and not the plan. The cost is
/// stated rather than hidden: a plan reference written in either file is not
/// caught, and those are the two files most likely to write the word.
const PLANNING_ALLOWED: &[(&str, &str)] = &[
    ("engine/src/eval.rs", "phase"),
    ("engine/tests/eval.rs", "phase"),
];

/// Whether `c` can be part of a path-shaped token.
const fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')
}

/// Every path-shaped token in `line` that starts with `docs/`, with
/// sentence punctuation trimmed from the end.
fn docs_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (start, _) in line.match_indices("docs/") {
        // Only the start of a path: `cadence/docs/...` is still rooted at
        // the repository, but `xyzdocs/` is a different name.
        if line[..start].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let rest = &line[start..];
        let end = rest.find(|c| !is_path_char(c)).unwrap_or(rest.len());
        out.push(rest[..end].trim_end_matches(['.', '/']));
    }
    out
}

/// Every `NAME.md` token in `line` whose NAME is entirely capitals, digits
/// and underscores: the root-document naming convention.
fn caps_md_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (at, _) in line.match_indices(".md") {
        // `.md` must end the token.
        if line[at + 3..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        let name_start = line[..at]
            .rfind(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .map_or(0, |i| i + c_len(&line[..at], i));
        let name = &line[name_start..at];
        // At least two characters of all-caps name, at a word boundary.
        let bounded =
            name_start == 0 || !line[..name_start].ends_with(|c: char| is_path_char(c) && c != '/');
        if name.len() >= 2 && bounded {
            out.push(&line[name_start..at + 3]);
        }
    }
    out
}

/// The byte length of the character starting at `i` in `s`.
fn c_len(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(1, char::len_utf8)
}

/// A character with no glyph to print: what to call it, and what to do with
/// it.
///
/// Reporting one of these the way a visible character is reported prints a
/// name that is not there, `` ` ` ``, and asks the author to find an ASCII
/// spelling for something that is not standing in for anything. So these are
/// named rather than quoted, and told to go rather than to be replaced.
struct Invisible {
    /// What to call it in the report, in place of the character itself.
    name: &'static str,
    /// What to do with it.
    advice: &'static str,
}

/// Delete it. The advice for a character that is not standing in for
/// anything: it has no ASCII spelling because it has no reading.
const DELETE: &str = "nothing: delete it. It is invisible and stands in for nothing, \
                      so there is no ASCII spelling to find";

/// The advice for a character that *is* standing in for a space.
const PLAIN_SPACE: &str = "an ordinary space. This one is invisible and is not one, \
                           which is why it survived being read";

/// Whether `c` has no visible glyph, and what to say about it.
///
/// The ranges rather than a Unicode property, because `char` in std carries no
/// category API and a dependency for this would be a dependency in a
/// repository chore. What the list has to cover is what actually arrives:
/// space characters that are not the space, the zero-width family, the bidi
/// controls that make a line read as something other than what it holds, the
/// byte-order mark, and the replacement character, which is not invisible but
/// means the bytes were not UTF-8.
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

/// How a character is shown in a report: named if it has no glyph, quoted if
/// it has one, and with its code point either way, which is the half that
/// survives a terminal.
fn describe(c: char) -> String {
    match invisible(c) {
        Some(i) => format!("{} (U+{:04X})", i.name, c as u32),
        None => format!("`{c}` (U+{:04X})", c as u32),
    }
}

/// What to write instead of `c`, for the characters that actually turn up.
///
/// A gate that reports only that it fired makes the author guess, and for a
/// dash the guess is a hyphen, which is the one answer the rule rules out. So
/// each arm names the replacement rather than the offence.
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

/// Every non-ASCII character in `line` that is on neither
/// [`ALLOWED_NON_ASCII`] nor `in_place`, once each.
///
/// `in_place` is what [`in_place_allowances`] returned for the file this line
/// is in, resolved once per file rather than once per character.
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

/// Whether `c` can separate a planning noun from its number: a space, the
/// punctuation a label is written with, or nothing at all (`phase1`).
///
/// A full stop is deliberately absent. "changes the phase. 3 of them" is a
/// sentence, not a label.
const fn is_label_gap(c: char) -> bool {
    matches!(c, ' ' | '\t' | '-' | '_' | ':' | '#')
}

/// The first planning label on `line`, if it has one: a word from
/// [`PLANNING_WORDS`] at a word boundary, an optional plural, an optional run
/// of separators, and then a digit. One report per line, as the other rules
/// here give.
///
/// The returned slice is cut from `line` rather than rebuilt, so the report
/// quotes what the author wrote. ASCII lowercasing preserves byte length, so
/// an offset into the lowered copy is the same offset into the original.
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

/// What a run reads: the files on disk, or the content `git commit` is about
/// to record.
///
/// The distinction is the whole point of running this before a commit. A check
/// that reads the working tree while the index holds something else passes
/// commits it should reject, when the fix is written and not staged, and
/// rejects commits it should not, when the offending line is in the tree and
/// not in the commit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    WorkingTree,
    Index,
}

/// `git` in the repository root, or `Err` with something a hook can print.
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

/// Paths out of a `-z` git listing. `-z` rather than plain output because
/// `core.quotePath` mangles anything outside ASCII, and this command is in the
/// business of finding characters outside ASCII.
fn nul_separated(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every path in the index.
fn index_paths(root: &Path) -> Result<Vec<String>, String> {
    Ok(nul_separated(&git(root, &["ls-files", "--cached", "-z"])?))
}

/// The staged additions, copies, modifications and renames: the content this
/// commit introduces, which is all a pre-commit run has to read. Everything
/// else was read by the run that admitted it, and by CI, which reads the whole
/// tree every time.
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

/// `(relative path, content)` for everything one run reads.
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

/// Every problem in one file's `text`, as `(line number, what and what to do)`.
///
/// Split out of [`check_boundary`] so that the function left behind is the
/// choice of what to read and the report, and this one is the rules. The
/// repository's own clippy gate refuses a function this long, which is the
/// lint that caught `nps` growing past it once already.
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
            if token != "README.md" {
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
    }
    out
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
    // The `docs/` existence rule resolves against whichever tree is being
    // read, so a citation of a file that is on disk and not in the commit
    // fails a staged run. That is the reference the commit would publish.
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
        // The file defining the rules necessarily spells some of them out.
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

/// Every regular file under `dir`, skipping `SKIP_DIRS` and `.DS_Store`.
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
        } else if ty.is_file() && name != ".DS_Store" {
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
// What the SPRT harness plays is not the time control the preset names. The
// worker measures the bench's speed on its own hardware and multiplies the
// nominal control by `scale_nps / measured`, where `scale_nps` is a number a
// human entered when the test was created. If that number is stale, every
// game of every test is played at the wrong clock and nothing says so. This
// happened after quiescence was added: the bench's speed halved while the
// entered figure did not.
//
// So the figure is not remembered, it is measured, here, at the moment a test
// is created and from the binary the scaling will divide by. This subcommand
// is the thing that measures it. It is deliberately a tool and not a gate: it
// has no opinion and no failing exit code, because the value is a function of
// the machine, its concurrency and its thermal state as much as of the code,
// and a gate over a number no other party can recompute would be a mechanism
// in appearance only.
//
// The measurement copies the harness rather than inventing its own shape
// (OpenBench `Client/bench.py` `run_benchmark`, `Client/worker.py`
// `determine_scale_factor` and `safe_run_benchmarks`):
//
//   * one round is `concurrency` copies of `cadence bench` run at once, and
//     the round's figure is the MEAN of their nps, not the best of them;
//   * the minimum, maximum and spread within that round are retained too:
//     they answer whether the copies contend with each other, which the mean
//     cannot show;
//   * the harness takes exactly one round per binary (`sets=1`), so a round
//     here is a round there;
//   * it benches dev first and base second, so under `scale_method = BASE`
//     each divisor candidate is the SECOND figure, taken on a machine the
//     first bench has already warmed. Pairs are reported because the machine
//     can keep warming across the run; the flat tail, not an aggregate over
//     cold and warm pairs, is the reference entered as `scale_nps`.
//
// It also re-checks for free what the harness checks: every copy of a round
// must report the same node count, or the harness refuses the workload with
// `Non-Deterministic Benches`.

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

/// One round: `copies` benches at once, retaining both divisor and contention data.
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

/// What the table means, printed once before it. Split out of [`nps`] so
/// that the function left behind is the measurement and its checks.
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

/// One row of the table: a round's mean, min, max, spread and node count.
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
    // Every round runs the same binary, so a disagreement *between* rounds is
    // as much a determinism violation as one within a round. `run_round`
    // checks within; this checks across, because the line printed at the end
    // claims both and a claim wider than its check is the fault this whole
    // subcommand exists because of.
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

/// What a given reference would deliver against what was just measured.
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

/// Point this clone's git at `.githooks/`.
///
/// Hooks are not cloned, so this is per-clone and has to be re-run on a fresh
/// checkout. `core.hooksPath` is used rather than copying files into
/// `.git/hooks/`, so that a hook edited in the repository takes effect
/// immediately instead of after someone remembers to re-install it.
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

    /// `cargo test --workspace` does not reach this crate, so these run from
    /// their own manifest, in CI, beside the fmt and clippy legs. A matcher
    /// whose tests nothing runs is the same shape as a rule nothing checks.
    const NOT_ALLOWED: &str = "core/src/lib.rs";

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

    /// The two lists answer different questions and a character on both would
    /// make its paths dead text: the everywhere list would already have let it
    /// through. Checked rather than remembered, because the two are written
    /// forty lines apart.
    ///
    /// Vacuous while [`ALLOWED_IN_PLACE`] is empty, and kept for the row that
    /// may come: the invariant is what a reader adding one needs told.
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
        // The three that reached a tree in one week, and the other spelling of mu.
        for bad in [
            "a dash \u{2014}",
            "a \u{201c}quote\u{201d}",
            "an ellipsis\u{2026}",
            "12 \u{3bc}s",
        ] {
            assert!(!stray_non_ascii(bad, &[]).is_empty(), "{bad} passed");
        }
    }

    /// The table is empty, so no path admits anything, and the two characters
    /// it used to admit are refused in the files that used to be their places.
    ///
    /// This is the test that starts saying something again the day a row is
    /// added: it drives [`in_place_allowances`] over the paths that mattered
    /// most recently rather than asserting on the empty slice directly.
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

    /// The citation sign was the last row of [`ALLOWED_IN_PLACE`] and is now
    /// refused everywhere, so what a contributor meets is the plain message and
    /// the advice to write the word. Asserted because the advice is the whole
    /// reason the removal is not a loss: the twenty-seven citations it used to
    /// abbreviate now say "section 4", and so does the report.
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

    /// The seven invisibles that actually arrive in pasted prose -- the space
    /// that does not break, the hyphen that does not print, the zero-width
    /// space, the two joiners and the non-joiner, and the byte-order mark --
    /// plus the bidi overrides that make a line read as something other than
    /// what it holds, and the replacement character that means the bytes were
    /// not UTF-8.
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

    /// An exempt character with no glyph would be a contradiction: the list
    /// admits characters for what they say, and one that shows nothing says
    /// nothing.
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
}
