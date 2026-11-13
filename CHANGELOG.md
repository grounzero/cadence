# Changelog

All notable changes to Cadence, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

How the version numbers work: the middle number starts a new stretch of
work, and the last number goes up by one for each change that passed its
test, and for each fix to how the engine plays. A few versions say
otherwise, and they say why.

Each test played the new version against the one before it, at 8 seconds
plus 0.08 a move unless a longer time control is named. The test result
is how much stronger the new version scored, in Elo, give or take the
figure shown (95% of the time). These are gains against Cadence's own
previous version. They do not add up to a rating. Each test named here is
listed under its number in
[docs/testing/sprt-log.csv](docs/testing/sprt-log.csv), with its games and
result.

## [Unreleased]

## [0.6.0] - 2026-11-13

Everything since 0.5.23, published first as four release candidates, each
a pre-release: 0.6.0-rc.1 (2026-11-06), 0.6.0-rc.2 (2026-11-08),
0.6.0-rc.3 (2026-11-10) and 0.6.0-rc.4 (2026-11-13). It adds release
binaries and install scripts, scores a position where neither side has the
material to mate as a draw, and fixes how the engine answers the GUI, uses
the tablebases and ponders.

### Added

- Tests: proofs, checked by the Kani model checker on every change, that
  the score encoding, the transposition table's entries and buckets, the
  history and correction updates, the pruning margins and reductions, and
  the time budget stay within their bounds. A failed proof fails the build.
- Install scripts: `install.sh` for Linux, macOS, and Windows under Git
  Bash or WSL, and `install.ps1` for Windows PowerShell. Each installs a
  release's binary for the system it runs on once the binary's SHA-256 and
  bench match the release's, or builds the release from source where no
  binary fits, and can download the 3-4-5 piece Syzygy tables. The README
  says how to run them.
- Releases: each release builds seven binaries, Windows and Linux x86-64
  each in an AVX2 build and a generic one, Linux aarch64, macOS arm64 and
  macOS x86-64, and publishes them with `SHA256SUMS`, `bench.txt`, the two
  install scripts and a provenance attestation for each binary, made by the
  release workflow run at the release's tag; a manual run drafts and attests
  a release only when run at its tag.
- The test log: `docs/testing/sprt-log.csv` gives every SPRT, passed or
  failed, with its games and result.

### Changed

- Search: a position where neither side has the material to mate is scored
  as a draw. The bench rises from 24,819,899 to 34,686,410 nodes.
- Install scripts: they ask only before building from source, installing a
  compiler or Rust, replacing an install, changing the PATH or downloading
  the tablebases, and state the rest: the build they take, the folder and
  the binary's check. `--generic` and `--dir` are new names for
  `--build generic` and `--prefix`, which still work. The tablebase download
  shows its progress in megabytes and prints the folder for `SyzygyPath` on
  a line of its own, and after adding to the PATH they say how to run
  `cadence` before a new terminal opens. `--update` now differs from a plain
  run only in replacing a file at the install path that they did not
  install unless that is declined.
- Install scripts: without `--version` they take the latest stable release,
  or where none is published they name the newest pre-release, print the
  command that installs it and ask. A build from source names the tag it
  builds, and none is offered when nothing is published. A failed download
  is described in plain words, with the details in the log.
- Install scripts: a token in `GITHUB_TOKEN` is sent with their requests to
  GitHub's API, and a refusal for GitHub's hourly limit on unauthenticated
  requests is named as one, with what to do.
- Install scripts: where `gh` is installed and signed in, they check that
  each binary's build attestation was made by the release workflow run at
  the version's tag, and do not use a binary whose attestation fails; where
  `gh` cannot check it, they say so and go on with the checksum and the
  bench. A build from source first checks that
  the release's tag is the commit the release names, before any compiler or
  Rust is installed; where GitHub's API does not answer, they say so and
  build without that check. The end of a run names the checks the binary
  passed, with no SHA-256 for a build from source.
- Install scripts: they compare the installed version with the release. The
  same version is reported as up to date and the run stops, an older one is
  upgraded unless that is declined, and a newer one is kept unless a
  downgrade is asked for, as `--yes` with `--version` asks for one. A plain
  run installs into the folder the last install went to, `--dir`'s included.
  An install by an earlier installer, which recorded only its path, is known
  by its SHA-256 among the release's binaries; otherwise they ask to replace
  it, yes by default only on a run taking the newest release over another
  release's binary. Another build, or a build from source, asked for
  at the same version replaces the one installed. A file at the install path
  that they did not install is never run, and a second run changes nothing.
- Install scripts: a binary the system refuses to start, as on a managed
  machine whose policy blocks it, is reported in plain words with what
  installing needs, and the log is kept. One the system cannot start for
  another reason, such as a missing loader, is built from source instead.
  The installer offers its own version only where that release is
  published.
- Install scripts: `CADENCE_RELEASES_URL` must name this repository's
  releases unless `--test-urls` is given, and every run prints where the
  release files come from. A binary's bench and its answer to `uci` are
  stopped, with every process they started, after ten minutes and thirty
  seconds.
- Install scripts: `--tables` downloads the tablebases without asking. The
  table names published with their checksums are checked before any becomes
  a path, a table already there and matching its checksum is kept, and a
  download that stopped is offered again by the next run.
- Install scripts: on Windows, `install.sh` and `install.ps1` keep one
  install state, Windows paths in UTF-8 without a byte order mark, each
  reading what the other wrote, and either uninstalls what the other
  installed.
- The README fetches the install scripts from the latest release rather
  than from `main`, and says a release's binaries can be downloaded
  directly.
- Tools: `setup-worker.py` refuses a configuration inside the repository,
  and git ignores `worker.json` and `credentials.cadence`. The Makefile
  clears an inherited `RUSTFLAGS`, and with it `CARGO_ENCODED_RUSTFLAGS`
  and every `CARGO_TARGET_<triple>_RUSTFLAGS`, as the install scripts' builds
  do for the target they build.
- The README gives a table of strength estimates from our own gauntlets
  against rated engines, and `docs/calibration/strength.md` publishes each
  gauntlet's games and fit, and how to check them.
- Tools: the pre-commit hook rebuilds its boundary checker when any file
  the checker is built from is newer than it, not only its main source.
- The README is shorter, starts with the install scripts and ends with
  acknowledgements. Setting up an OpenBench worker and creating a test are
  explained by `python3 openbench/setup-worker.py --help` instead.
- UCI: under `UCI_LimitStrength`, the engine prints only the lines the GUI
  asked for, and reports the move it plays.
- UCI: `SyzygyPath` is taken exactly as sent, spaces included.
- The README documents every UCI option, the tools and the generic build.
- Search: its state is grouped into its counters, its move-ordering
  tables, the run's result and its clock, with no change to play: every
  bench position searches the same nodes as before.

### Fixed

- UCI: `position` sent during a search stops and joins that search first.
  The search could answer `bestmove` for the old position after the new
  one had been set.
- UCI: `setoption name Hash` sent during a search stops and joins that
  search before the table is resized. The search could end without its
  `bestmove`.
- Tools: `perft --divide` writes castling as the position does, the king
  taking its rook only where a castling king or rook stands off its
  standard square. A standard position listed `e1h1` and `e1a1`.
- Tools: `datagen extract` publishes its training and holdout files as a
  pair, so a failure between the two no longer leaves one of them
  published.
- Tools: `bench` runs its searches on the 16 MB stack the engine's search
  threads have, as the tests now do, rather than the system's default.
- Pondering: after a ponderhit, the iteration the search was running stops
  at the move's planned time until an iteration completes. It could run on
  to the limit of half the remaining clock and then play the ponder's last
  result.
- UCI: after a ponderhit, the reported `time` and `nps` count from the
  `go`, as `nodes` does. `nps` was overstated.
- Tablebases: at a won root, only the winning moves that reach a capture
  or a pawn move soonest are searched. The losing side is unchanged.
- Install scripts: under Git Bash on an Intel processor, `install.sh`
  chose the generic build where the AVX2 build runs. It now chooses as
  `install.ps1` does.
- Install scripts: the newest pre-release is the highest version, where it
  was the first one GitHub listed, and a second run no longer adds the PATH
  line to a shell profile that already holds it.
- Install scripts: the uninstall removes only the PATH line under the
  installer's marker, leaving the same line written by hand, and on Windows
  the user PATH is edited as the registry holds it, so a `%VARIABLE%` in it
  stays unexpanded. After a `--dir` install, a plain run no longer installs
  a second copy in the default folder, and the uninstall removes the binary
  wherever the install put it. The uninstall says it removed a PATH entry
  only when there was one, and removes a user PATH it leaves empty.
- Install scripts: `install.ps1` finds its folder on the session PATH as
  `install.sh` does, and says the tablebases are removed only once they are.
- Search: when the tablebases cut a node, the transposition table keeps
  that node's move.
- Tablebases: a root the tables rank reports the tables' verdict as its
  score, a root move is ranked under the fifty-move counter as the
  tablebase library ranks it, each table file is opened once and one that
  fails is named, and stop and the clock are checked between root probes.
- UCI: the clock starts when `go` is read, and every `go` is answered,
  with `bestmove 0000` after a `position` command that failed. A clock
  value that is not a whole number reads as zero, a `go` limit too large
  to hold reads as the largest the engine holds, the count in `go mate` is
  clamped, a root with no legal move reports a score line, and an input
  line is limited to a mebibyte.
- Positions: a FEN with impossible material is refused, castling letters
  with no rook to castle with are dropped, and the move counters are
  clamped.
- Search: a reduction stays below the depth it reduces, a finished line is
  kept, and under a node limit with helper threads the main thread
  finishes its first iteration. `perft` refuses a depth deeper than the
  search allows.
- Version: a build calls itself by the release's version only at that
  version's annotated tag, on an unmodified tree.
- Tools: `bench`, `perft`, `spsa` and `magics` exit with a failure when
  they cannot write their output, `texel` checks its arguments and its
  weights, and `datagen` caps its workers, keeps its games in order and
  refuses a record whose moves do not end as it says.
- Build: the generic Windows build keeps SSE3.

Test 76, for the transposition table fix: -0.27 (between -2.75 and
+2.20), a self-play estimate against the build before it at the short
control, both sides given the tables up to five pieces. This was a
non-regression test, which asks only whether the fix costs strength. It
was stopped at 24,106 games before it decided, and the fix was taken on a
result that shows no cost. It is not a rating.

Test 77, for keeping the fastest wins at a won tablebase root: +0.36, give
or take 1.63, a self-play estimate against the build before it at the
short control, both sides given the tables up to five pieces. This was a
non-regression test, which asks only whether the change costs strength,
and it passed at 54,626 games. It is not a rating.

Test 80, for scoring a position with no mating material as a draw: +2.73
(between -0.46 and +5.93), a self-play estimate against the build before it
at the short control. This was a non-regression test, which asks only
whether the change costs strength, and it passed at 15,760 games. It is not
a rating.

## [0.5.23] - 2026-11-03

### Added

- Search: Syzygy endgame tablebases, through the `SyzygyPath` and
  `SyzygyProbeLimit` options. The search probes the win-draw-loss tables
  after a capture or a pawn move, and the root ranks its moves by the
  distance-to-zeroing tables, within the fifty-move rule. The path is
  empty by default, which leaves probing off.

### Changed

- Speed: the search runs about 11% faster on the machine the tests are
  played on, and finds the same moves at the same node counts at every
  bench position. The pawn structure's score is cached per search, an
  exchange question stops once its answer is decided, a losing capture's
  exchange is read once, and release builds leave out the search's
  diagnostic counters. Not tested in games.
- Build: the Rust toolchain is pinned to 1.99.1.

Test 75, for the tablebases: +13.53 (between +7.00 and +20.07), a
self-play estimate against 0.5.22 with the speed changes above, both
sides given the tables up to five pieces, which only 0.5.23 reads, at the
short control, from a sequential test stopped at its first crossing, so
it leans high. It is not a rating.

## [0.5.22] - 2026-10-30

### Changed

- Search: at a node the search expects to fail high, a late move that
  would already be searched shallower is now searched one ply shallower
  still. A move that would be searched at full depth keeps it.

Test 68: +5.95 (between +1.64 and +10.26), a self-play estimate against
0.5.21 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.21] - 2026-10-29

### Changed

- Search: a capture that loses material once the exchange on its square
  is counted, tried late at a node searched with a null window, is now
  searched one ply shallower wherever a quiet move in its place would be.
  A capture made in check or giving check keeps its depth.

Test 66: +7.50 (between +2.46 and +12.55), a self-play estimate against
0.5.20 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.20] - 2026-10-28

### Changed

- Search: a node searched with a null window, off the line the search
  reports, is now searched two plies shallower when the transposition
  table holds no move for it and at least four plies are left. A node in
  check keeps its depth.

Test 65: +37.16 (between +24.67 and +49.75), a self-play estimate against
0.5.19 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.19] - 2026-10-26

### Changed

- Search: quiet moves are now ordered by how well each has done in reply
  to the opponent's last move and after the side's own move before it, as
  well as by its own history. Captures, the killer moves and the order of
  the groups of moves are unchanged.

Test 62: +10.68 (between +4.35 and +17.01), a self-play estimate against
0.5.18 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.18] - 2026-10-25

### Changed

- Time management: the time the engine aims to spend on a move now
  depends on how many depths its best move has stood. It aims for about
  twice the usual time while the move has just changed, and about three
  quarters of it once the move has stood for nine depths or more. The
  limit it never passes, and a fixed time per move, are unchanged.

Test 61: +9.30 (between +3.46 and +15.15), a self-play estimate against
0.5.17 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.17] - 2026-10-24

### Changed

- Search: late moves at a node searched with an open window, the line the
  search reports, are reduced one ply less, and a move whose reduction
  would be one ply is searched without one.

Test 60: +10.60 (between +4.25 and +16.95), a self-play estimate against
0.5.16 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.16] - 2026-10-23

### Fixed

- Search: when a quiet move causes a cutoff, the history now marks down
  only the quiet moves the node searched before it. Quiet moves that
  pruning skipped without searching are no longer marked down.

Test 58: +0.29 (between -1.62 and +2.20), a self-play estimate against
0.5.15 at the short control. This was a non-regression test, which asks
only whether the fix costs strength, and it passed: the result is
neutral, not a gain. It is not a rating.

## [0.5.15] - 2026-10-14

### Changed

- Search: a capture that loses material is now tried after every quiet
  move; a promotion that loses material keeps its earlier place.

Test 57: +2.92 (between +0.62 and +5.23), a self-play estimate against
0.5.14 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.14] - 2026-10-12

### Changed

- Search: late move pruning counts only quiet moves toward its limit, so
  captures and promotions searched ahead of them no longer use it up.

Test 56: +6.77 (between +1.98 and +11.57), a self-play estimate against
0.5.13 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.13] - 2026-10-11

### Added

- UCI: the `Move Overhead`, `Clear Hash` and `UCI_EngineAbout` options.
- UCI: `go searchmoves` searches only the moves it names, and `go mate N`
  stops once a mate in N or fewer is proved.

### Changed

- Search: late move pruning no longer gives quiet moves up at a node searched
  with an open window, the line the search reports.

Test 55: +11.00 (between +4.43 and +17.58), a self-play estimate against
0.5.12 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.12] - 2026-10-10

### Fixed

- Search: reverse futility and the null move could cut at a stalemate before
  generating moves, so a move stalemating a materially stronger opponent read
  as a fail-low. Each now asks for a legal move before it returns more than a
  draw, which costs 0.34% of speed against 0.5.11 with the same node count at
  every bench position. Not tested in games.

## [0.5.11] - 2026-10-09

### Changed

- Search: when the narrow window around the last score fails low, its upper
  edge moves to the middle of the window that failed before the lower edge
  widens.

Test 54: +9.53 (between +3.59 and +15.47), a self-play estimate against
0.5.10 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

## [0.5.10] - 2026-10-09

### Changed

- Quiescence search: the check for a position with no legal move now stops
  at the first legal move it finds, and is made only where the search would
  otherwise stop on its static evaluation.

Test 53: +11.42 (between +4.84 and +18.00), a self-play estimate against
0.5.9 at the short control, from a sequential test stopped at its first
crossing, so it leans high. It is not a rating.

### Fixed

- Thinking on the opponent's time, a search that had already finished when
  the opponent played the expected move never answered it, so the engine
  could lose on time with time on its clock.

## [0.5.9] - 2026-10-09

### Changed

- Search: each iteration from depth four searches a narrow window around the
  last completed score first, widening it on each failure.

Test 52: +33.39 (between +21.11 and +45.75), a self-play estimate against
0.5.8 with the fix below at the short control, from a sequential test stopped
at its first crossing, so it leans high. It is not a rating.

### Fixed

- Quiescence scored a position with no legal move as its static evaluation
  instead of a draw, so a stalemate at the horizon could be played into or
  avoided for the wrong reason.

## [0.5.8] - 2026-10-07

### Changed

- Evaluation: every weight refitted together against the same positions,
  then scaled so the pieces keep their usual values.

Test 51: +39.02 (between +25.34 and +52.83), a self-play estimate against
0.5.7 at the short control, not a rating.

### Fixed

- Setting up a position from FEN now refuses an en passant square that no
  double pawn push could have left, and a pawn on the first or eighth rank.
  Either was accepted before, and the first could leave the board
  inconsistent after an en passant capture, with no error in a release
  build.

## [0.5.7] - 2026-10-04

### Changed

- Quiescence search: when the side to move can stop searching because its
  position already looks good enough, it now reports the bound it was asked
  about rather than its own estimate of the position.

Test 47: +10.67 (between +4.19 and +17.15).

## [0.5.6] - 2026-10-04

### Changed

- Reverse futility: when the search skips a position that looks too good
  for the side to move to need searching, it now reports the bound it was
  asked about rather than its own estimate of the position.

Test 46: +17.55 (between +8.78 and +26.34).

## [0.5.5] - 2026-10-03

### Added

- King safety: the evaluation now scores attacks near each king and the
  pawns sheltering it. The king's square table was retuned alongside it.

Test 45: +28.46 (between +16.86 and +40.06).

### Changed

- The walk over piece attacks is separated from the mobility count. No
  change to how the engine plays.

## [0.5.4] - 2026-10-03

### Added

- Mobility: the evaluation now rewards pieces that have more squares to
  move to. The four piece tables were retuned alongside it.

Test 44: +50.67 (between +35.01 and +66.33).

## [0.5.3] - 2026-10-02

### Changed

- The piece values and the queen's square table were retuned on
  Cadence's own games.

Test 43: +19.68 (between +10.29 and +29.10).

## [0.5.2] - 2026-10-02

### Added

- Pawn structure: the evaluation now scores pawn weaknesses and strengths.
  The pawn square table was retuned alongside it.

Test 41: +34.53 (between +21.14 and +47.92). At 40 seconds plus 0.4,
test 42: +52.04 (between +39.53 and +64.55).

## [0.5.1] - 2026-09-30

### Changed

- The knight, bishop, rook and king square tables were retuned on
  Cadence's own games, replacing hand-written values.

Test 39: +110.11 (between +86.63 and +134.60). At 40 seconds plus 0.4,
test 40: +115.46 (between +97.29 and +134.26).

## [0.5.0] - 2026-09-22

Starts the evaluation work. It also carries two changes made after 0.4.10.

### Added

- ProbCut: the search cuts a position short when a quick, shallow search
  of a capture already shows the position is good enough.
- `UCI_LimitStrength` and `UCI_Elo`, for playing at a weaker level. Off by
  default.

ProbCut, test 38: +27.40 (give or take 11.26), 2,224 games.

## [0.4.10] - 2026-09-07

### Added

- More than one search thread, through the `Threads` option.

Not tested this way. The tests run one thread on each side, so they
cannot see this change.

## [0.4.9] - 2026-09-07

### Added

- Pondering: thinking on the opponent's time, through the `Ponder` option.

Not tested this way. The tests run with pondering off, so they cannot see
this change.

## [0.4.8] - 2026-09-04

### Added

- Correction history: the engine remembers how wrong its evaluation has
  been in similar pawn structures, and corrects for it.

Test 35: +10.42 (give or take 6.45), 6,340 games.

## [0.4.7] - 2026-09-01

### Added

- Late move pruning: when little search depth is left, quiet moves near
  the end of the list are skipped.

Test 32: +22.67 (give or take 10.15), 2,624 games.

## [0.4.6] - 2026-08-30

### Added

- Reverse futility pruning: a position that is already far ahead is cut
  short without searching its moves.

Test 29: +31.39 (give or take 12.03), 1,820 games.

## [0.4.5] - 2026-08-30

### Added

- Futility pruning: near the end of a search line, quiet moves that cannot
  catch up are skipped.

Test 28: +40.68 (give or take 14.06), 1,450 games.

## [0.4.4] - 2026-08-30

### Added

- The history heuristic: quiet moves that worked well elsewhere in the
  search are tried earlier.

Test 27: +80.66 (give or take 20.79), 798 games.

## [0.4.3] - 2026-08-30

### Added

- Late move reductions: moves late in the list are searched less deeply
  first, and fully only if they look promising.

Test 26: +61.65 (give or take 17.78), 1,008 games.

## [0.4.2] - 2026-08-29

### Added

- Null-move pruning: the engine passes a turn in its head, and if it is
  still doing well enough, it stops searching that line.

Test 24: +103.41 (give or take 22.83), 626 games.

## [0.4.1] - 2026-08-28

### Changed

- Time management: the engine no longer starts a new search depth that
  the clock would not let it finish.

Test 21: +14.24 (give or take 7.35), 3,930 games.

## [0.4.0] - 2026-08-28

Starts the search work. No change to how the engine plays: its benchmark
count is the same as 0.3.0's.

## [0.3.0] - 2026-08-27

The first tagged version. An alpha-beta search with a transposition table,
move ordering, quiescence search, check extensions and principal variation
search. Each was tested on its own before versions counted tests.

## Tested and not kept, 0.4.0 to 0.4.8

These changes were tested and thrown away.

- A different way of sharing time across a game. Test 22: -29.90 (give or
  take 11.90).
- Sharing time by where the search spends its effort. Test 23: -4.10 (give
  or take 5.00).
- Delta pruning. Test 30: -0.99 (give or take 3.71).
- Late move reductions sized by the move's history. Test 31: -37.26 (give
  or take 13.98).
- Singular extensions. Test 33: -19.35 (give or take 10.33).
- Static exchange evaluation (SEE) on captures in the main search. Test 34:
  +2.81 (give or take 3.64). The result never became clear, so it was not
  kept.

[Unreleased]: https://github.com/grounzero/cadence/compare/0.6.0...main
[0.6.0]: https://github.com/grounzero/cadence/compare/0.5.23...0.6.0
[0.5.23]: https://github.com/grounzero/cadence/compare/0.5.22...0.5.23
[0.5.22]: https://github.com/grounzero/cadence/compare/0.5.21...0.5.22
[0.5.21]: https://github.com/grounzero/cadence/compare/0.5.20...0.5.21
[0.5.20]: https://github.com/grounzero/cadence/compare/0.5.19...0.5.20
[0.5.19]: https://github.com/grounzero/cadence/compare/0.5.18...0.5.19
[0.5.18]: https://github.com/grounzero/cadence/compare/0.5.17...0.5.18
[0.5.17]: https://github.com/grounzero/cadence/compare/0.5.16...0.5.17
[0.5.16]: https://github.com/grounzero/cadence/compare/0.5.15...0.5.16
[0.5.15]: https://github.com/grounzero/cadence/compare/0.5.14...0.5.15
[0.5.14]: https://github.com/grounzero/cadence/compare/0.5.13...0.5.14
[0.5.13]: https://github.com/grounzero/cadence/compare/0.5.12...0.5.13
[0.5.12]: https://github.com/grounzero/cadence/compare/0.5.11...0.5.12
[0.5.11]: https://github.com/grounzero/cadence/compare/0.5.10...0.5.11
[0.5.10]: https://github.com/grounzero/cadence/compare/0.5.9...0.5.10
[0.5.9]: https://github.com/grounzero/cadence/compare/0.5.8...0.5.9
[0.5.8]: https://github.com/grounzero/cadence/compare/0.5.7...0.5.8
[0.5.7]: https://github.com/grounzero/cadence/compare/0.5.6...0.5.7
[0.5.6]: https://github.com/grounzero/cadence/compare/0.5.5...0.5.6
[0.5.5]: https://github.com/grounzero/cadence/compare/0.5.4...0.5.5
[0.5.4]: https://github.com/grounzero/cadence/compare/0.5.3...0.5.4
[0.5.3]: https://github.com/grounzero/cadence/compare/0.5.2...0.5.3
[0.5.2]: https://github.com/grounzero/cadence/compare/0.5.1...0.5.2
[0.5.1]: https://github.com/grounzero/cadence/compare/0.5.0...0.5.1
[0.5.0]: https://github.com/grounzero/cadence/compare/0.4.10...0.5.0
[0.4.10]: https://github.com/grounzero/cadence/compare/0.4.9...0.4.10
[0.4.9]: https://github.com/grounzero/cadence/compare/0.4.8...0.4.9
[0.4.8]: https://github.com/grounzero/cadence/compare/0.4.7...0.4.8
[0.4.7]: https://github.com/grounzero/cadence/compare/0.4.6...0.4.7
[0.4.6]: https://github.com/grounzero/cadence/compare/0.4.5...0.4.6
[0.4.5]: https://github.com/grounzero/cadence/compare/0.4.4...0.4.5
[0.4.4]: https://github.com/grounzero/cadence/compare/0.4.3...0.4.4
[0.4.3]: https://github.com/grounzero/cadence/compare/0.4.2...0.4.3
[0.4.2]: https://github.com/grounzero/cadence/compare/0.4.1...0.4.2
[0.4.1]: https://github.com/grounzero/cadence/compare/0.4.0...0.4.1
[0.4.0]: https://github.com/grounzero/cadence/compare/0.3.0...0.4.0
[0.3.0]: https://github.com/grounzero/cadence/tree/0.3.0
