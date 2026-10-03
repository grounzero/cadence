# Changelog

All notable changes to Cadence, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

How the version numbers work: the middle number starts a new stretch of
work, and the last number goes up by one for each change that passed its
test. A few versions say otherwise, and they say why.

Each test played the new version against the one before it, at 8 seconds
plus 0.08 a move unless a longer time control is named. The test result
is how much stronger the new version scored, in Elo, give or take the
figure shown (95% of the time). These are gains against Cadence's own
previous version. They do not add up to a rating. The test records are
private, so the results are reported here rather than linked.

## [Unreleased]

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

[Unreleased]: https://github.com/grounzero/cadence/compare/0.5.5...main
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
