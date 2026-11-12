# Strength

How strong Cadence is, measured by playing it against engines that have
ratings, with every game here so that anyone can check the figures. None of
them is an official rating: each is our own estimate, worked out against
other engines' published ratings.

| Version | Estimate | Games | Measured | Evidence |
| --- | --- | ---: | --- | --- |
| [0.6.0-rc.3](https://github.com/grounzero/cadence/releases/tag/0.6.0-rc.3) | 2796 ± 9 | 4,000 | 2026-11-11 to 2026-11-12 | [below](#060-rc3-about-2796) |
| [0.4.8](https://github.com/grounzero/cadence/tree/0.4.8) | 2289 ± 14 | 2,000 | 2026-09-28 | [below](#048-about-2289) |

Each "±" is the range the result stays in 95% of the time from luck in the
games alone. Versions after each one listed have not been measured this way.

## 0.6.0-rc.3: about 2796

An estimate of where Cadence would sit on the CCRL Blitz list: about 2796,
give or take 9 (95% of the time), from 4,000 games against seven engines on
that list, measured on
[0.6.0-rc.3](https://github.com/grounzero/cadence/releases/tag/0.6.0-rc.3).
An eighth engine, Reckless 0.4.0, was played and left out for the reason
below. With all eight the estimate is 2806.

### Conditions

- **The machine**: an Apple Silicon Mac, an M5 Max, using its six
  performance cores: six games at once, one on each, on mains power, kept
  awake, with nothing else running. Its thermal and performance warning
  levels were read every minute, and none of 1,672 readings showed a
  warning.
- **The time control**: the CCRL Blitz list plays 2 minutes plus 1 second
  a game on its reference machine, an Intel i7-4770K, where the reference
  benchmark CCRL's testers published on TalkChess in January 2022 takes
  2,054 ms. On this Mac the same benchmark, built from source for Apple
  Silicon and run as six copies at once like the games, took 861 ms, the
  median of 30 copies, read on
  2026-11-10 and again just before the games. So the time control was scaled
  by 861 / 2,054 = 0.419, to 50.28 seconds plus 0.419 seconds a move. The
  bench read 865 ms after the games, 0.46% more.
- **The openings**: 250 openings of eight moves, drawn with seed 600 from
  `8moves_v3.pgn`, a public-domain (CC0-1.0) book of eight-move openings,
  each played with both colours against every engine. Each game's PGN holds
  its opening moves.
- **The engines' settings**: one search thread, 256 MB of hash, pondering
  off, and the Syzygy tablebases up to five pieces for the engines that read
  them (Cadence, Halogen 8 and Supernova 2.4). Every other option at its
  default.
- **No adjudication**: every game was played to mate or to a draw by the
  rules.
- **The tools**: the match runner fastchess 1.8.2, and Ordo 1.2.6 for the
  fit, each engine fixed at its rating on the
  [CCRL Blitz list](https://computerchess.org.uk/404/rating_list_all.html)
  computed 2026-10-17.

### The engines

Each engine's rating is its CCRL Blitz rating on that list, with the list's
own margin and game count. "Implied" is the rating Cadence's score against
that one engine gives Cadence.

| Engine | CCRL Blitz | Games | Cadence scored | Implied |
| --- | --- | ---: | ---: | ---: |
| [Reckless 0.4.0](https://github.com/codedeliveryservice/Reckless/tree/v0.4.0) | 2932 ± 17 (943 games) | 500 | 42.4% | 2879, left out |
| [Weiawaga 5.0](https://github.com/Heiaha/Weiawaga/tree/v5.0) | 2872 ± 10 (2,845 games) | 500 | 40.2% | 2803 |
| [Clovis III](https://github.com/jonathanmcdermid/Clovis/tree/III) | 2828 ± 10 (2,928 games) | 500 | 44.8% | 2792 |
| [Halogen 8](https://github.com/KierenP/Halogen/tree/v8) | 2827 ± 17 (958 games) | 500 | 40.2% | 2758 |
| [Mess 0.3.0](https://github.com/raklaptudirm/mess/tree/v0.3.0) | 2773 ± 12 (2,329 games) | 500 | 57.0% | 2822 |
| [Velvet 1.2.0](https://github.com/mhonert/velvet-chess/tree/v1.2.0) | 2765 ± 13 (1,802 games) | 500 | 55.1% | 2801 |
| [Supernova 2.4](https://github.com/MichaeltheCoder7/Supernova/tree/v2.4) | 2679 ± 10 (3,580 games) | 500 | 66.1% | 2795 |
| [Blunder 8.5.5](https://github.com/algerbrex/blunder/tree/v8.5.5) | 2663 ± 11 (2,534 games) | 500 | 69.3% | 2804 |

**How each was built**: from source at the tag linked above, for Apple
Silicon, never run through Rosetta.

| Engine | Commit | Built with | SHA-256 of the binary |
| --- | --- | --- | --- |
| Reckless 0.4.0 | `9ed16b2` | `cargo build --release --locked`, Rust 1.97.1 | `2c81b529acc03170aa2548a753f49c2e34b13be237aec48b66599991371324ac` |
| Weiawaga 5.0 | `c03e4d6` | `cargo build --release`, Rust 1.97.1 | `7340671fbae85fbd733f8ed3424f1cee531cc772843b3e78a6514e026cda2e46` |
| Clovis III | `65cd44c` | `make CXX=clang++`, without `-fopenmp`, Apple clang 21 | `8789d9876bdac82f9e0b9e96abe836dd93f2e53d7e53180995b2ca1d6dbea399` |
| Halogen 8 | `5cdd26b` | `make CC=clang++`, without `-march=native`, Apple clang 21 | `5a3aeeab14dbe59b6d0eae1fc390701701689675d2905f51faf8b7a9d1dca4d7` |
| Mess 0.3.0 | `2e0c054` | `go build`, Go 1.27.3 | `8af274b80009b42fbf188b07d4edc16515328e962c3325b4c37bc189d9a3c9d7` |
| Velvet 1.2.0 | `6bb4a5f` | `cargo build --release`, Rust 1.97.1 | `5306966ba6c6ae02fe897bf8dd05bedfcaa78268ac0414e14da39471cc600465` |
| Supernova 2.4 | `e2de225` | `make CC=clang`, without `-static`, Apple clang 21 | `5caf1626288894a728f3e484f81f782eabf9d61045185f496f381845873e2e8d` |
| Blunder 8.5.5 | `89230a7` | `go build`, Go 1.27.3 | `f20f7063d95f8ed4406f7bdd0b495e7d85e6c95d99ffaf792eba4054847122ed` |
| The reference benchmark, version 10 | `b4c239b` | `make build ARCH=general-64 COMP=clang prefetch=yes`, Apple clang 21 | `761690cc9970a810c724decc0e78101ac6136a12ffed57692b842655adf263d0` |

Two engines needed a one-line change to build for Apple Silicon. Clovis III
includes an x86-only header it never uses, which clang refuses there, so the
include was deleted. The reference benchmark's Makefile names the processor
on its macOS line, so it was set to `arm64`. Cadence was the
[0.6.0-rc.3 release's](https://github.com/grounzero/cadence/releases/tag/0.6.0-rc.3)
own macOS binary, SHA-256
`0bcf77b6b9de00bcaaec17f06429f01aff471e2675e8437b48caa468708ea74d`.

**How the eight were chosen**: a first, smaller run of 100 games each
against four engines put Cadence near 2801. Then eight engines rated within
150 of that were taken, four above and four below, each a different
project. Each played ten games first, to check it ran cleanly at this time
control. One, Donna 4.1, lost three of its ten on time and was replaced by
Blunder 8.5.5.

### Why Reckless 0.4.0 is left out

Before the games, a limit was set on how far the engines may disagree: the
ratings the eight imply for Cadence may span at most 80 points. With
Reckless 0.4.0 they span 121, from 2758 to 2879, and without it 64, from
2758 to 2822. Reckless is not one end of a smooth spread: its own range,
2854 to 2903 (95% of the time), overlaps none of the other seven's. So the
estimate is fitted to the seven. With all eight it is 2806, give or take 9.

### Results

- 4,000 games, 500 against each engine. Cadence won 1,432, drew 1,287 and
  lost 1,281, a score of 51.9%: 54.8% with white and 49.0% with black.
- No engine crashed, disconnected or made an illegal move, and no game was
  lost on time by either side. No move ran more than half a second past
  its time budget.
- Every decisive game ended in mate. The draws were 880 by repetition, 200
  by the fifty-move rule, 189 by insufficient material and 18 by
  stalemate.

### Checking it

The games, the ratings used and Ordo's output are in this folder. From the
root of the repository, with Ordo 1.2.6:

```sh
ordo -Q -D -s 1000 \
  -m docs/calibration/anchors-2026-11-12.csv \
  -p docs/calibration/gauntlet-2026-11-12.pgn
```

With `anchors-2026-11-12-with-reckless.csv` in place of the first file,
the fit takes all eight engines. The "give or take" figure can change a
little between runs, because Ordo estimates it by simulation.

The games are the PGN the match runner wrote, with one change: four accented
letters in some opening names are spelled without their accents, because
this repository keeps its text ASCII. The moves, results, clocks and every
other tag are as written.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| [`gauntlet-2026-11-12.pgn`](gauntlet-2026-11-12.pgn) | 16,784,750 | `fbbb30e075af8a1f5157ad4a40e07948e6aa21b88155efc9dae05e2da82555db` |
| [`anchors-2026-11-12.csv`](anchors-2026-11-12.csv) | 219 | `d51a560f4a6eb815d9b86df238a4d8c055431b92a65510e6b95867688e9d3630` |
| [`anchors-2026-11-12-with-reckless.csv`](anchors-2026-11-12-with-reckless.csv) | 253 | `83243ae0423f7dba052baa2c9c5a128df166111097354f64e11fdcb7a02b4ba3` |
| [`gauntlet-2026-11-12-ordo.txt`](gauntlet-2026-11-12-ordo.txt) | 822 | `3d2244476d7f0ac6b5d2ffc521ea60f8a481093c21fa95c75a9670fda3f36473` |
| [`gauntlet-2026-11-12-ordo-with-reckless.txt`](gauntlet-2026-11-12-ordo-with-reckless.txt) | 822 | `b3d9d5f76202864e5785efcc3d3745ee9d2588c9843280ec2e279075211eec41` |

## 0.4.8: about 2289

An estimate of where Cadence 0.4.8 would sit on the CCRL Blitz list,
measured on 2026-09-28.

Cadence played 2,000 games at 2 minutes plus 1 second a move. It played 400
games against each of five engines that are on that list. The program Ordo
then worked out the rating that best fits those results (2289, give or take
14 from luck in the games).

Treat it as rough. Each of the five opponents, taken alone, gives a
different answer: anywhere from 2241 to 2318. The time control was not
scaled to the Mac.

The games, the ratings used and Ordo's output are in this folder. From the
root of the repository, with Ordo 1.2.6:

```sh
ordo -Q -D -s 2000 \
  -m docs/calibration/anchors-2026-09-05.csv \
  -p docs/calibration/gauntlet-2026-09-28.pgn
```

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| [`gauntlet-2026-09-28.pgn`](gauntlet-2026-09-28.pgn) | 7,306,342 | `e02d73679026e2b9185f9f6b36745fd3bbb960d0ef86cf30b228965b2d78ee25` |
| [`anchors-2026-09-05.csv`](anchors-2026-09-05.csv) | 102 | `62ae77e079fe7fe235f43316114a448bdc1d91397b3536c779b5598acc1c3146` |
| [`gauntlet-2026-09-28-ordo.txt`](gauntlet-2026-09-28-ordo.txt) | 516 | `2c429fe6b8bb0d4d5d82aaa5712f355ed9903101b9da25743339a6686c010c63` |

## On Lichess: 2222 blitz

The bot [Mithandros](https://lichess.org/@/Mithandros)'s Lichess blitz
rating on 2026-10-03, from 1,257 rated games (give or take about 90).

Lichess ratings are their own scale, built from games against people and
other bots. They are not CCRL ratings. The bot has also played several
versions of Cadence over that time, with settings a rating list would turn
off: it thinks on the opponent's time and uses an opening book. So this
number does not belong to any one version.

## What these figures are not

- **Not official ratings.** Each estimate is ours, worked out against other
  engines' published ratings.
- **Not measured on the list's machine.** The games ran on a Mac, not on the
  computer the list uses, so a minute of thinking is not the same amount of
  work. From 0.6.0-rc.3 the time control is scaled to the Mac by a
  benchmark, which brings the work close to the list's but not exactly to
  it.
- **Not a measure of later versions.** Gains against Cadence's own earlier
  versions do not translate into rating points, so a version is only
  estimated when it is measured this way.
- **Not the whole uncertainty.** Each "give or take" covers luck in the
  games. Which engines are chosen also moves the figure, which is why the
  engines' agreement is checked against a limit set before the games.
