// SPDX-License-Identifier: GPL-3.0-or-later

//! Each subcommand's `--help` text, kept here so the README can stay a summary.

/// The help for `args`, the words after `cadence`, or `None` when they name no subcommand.
#[must_use]
pub fn text(args: &[String]) -> Option<&'static str> {
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some("bench"), _) => Some(BENCH),
        (Some("perft"), _) => Some(PERFT),
        (Some("spsa"), _) => Some(SPSA),
        (Some("texel"), _) => Some(TEXEL),
        (Some("datagen"), Some("play")) => Some(DATAGEN_PLAY),
        (Some("datagen"), Some("extract")) => Some(DATAGEN_EXTRACT),
        (Some("datagen"), Some("run")) => Some(DATAGEN_RUN),
        (Some("datagen"), Some("repair")) => Some(DATAGEN_REPAIR),
        (Some("datagen"), Some("verify")) => Some(DATAGEN_VERIFY),
        (Some("datagen"), _) => Some(DATAGEN),
        _ => None,
    }
}

const BENCH: &str = "\
usage: cadence bench

Searches a fixed set of positions to a fixed depth, on one thread with a 16 MB
table, and prints the total node count on its last line. The count depends only
on the code, so it must match bench.txt; a commit that changes it ends with a
`Bench: <n>` line.";

const PERFT: &str = "\
usage: cadence perft [--divide] [--threads N] <fen | startpos> <depth>

Counts the legal move tree from a position to a depth.

  --divide      print the count under each first move
  --threads N   split the first moves across N threads";

const SPSA: &str = "\
usage: cadence spsa

Prints an SPSA tuner's input block: one line for each search constant a tune
may move, with its default, range and step.";

const TEXEL: &str = "\
usage: cadence texel <data> [--holdout N | --holdout-file PATH] [--iterations N]
                    [--rate R] [--threads N] [--report N] [--k K | --phase-k]
                    [--min-weight W] [--min-weight-for PREFIX W]... [--ridge L]
                    [--tune PREFIX]...

Fits the handcrafted evaluation's weights to positions, one a line: a FEN, a
`|` and the game's result.

  --holdout N            hold out N positions to score the fit
  --holdout-file PATH    score the fit on this file instead
  --iterations N         gradient steps
  --rate R               step size
  --threads N            threads for the gradient
  --report N             print the losses every N steps
  --k K                  the score-to-result slope; --phase-k fits one for the
                         middlegame and one for the endgame
  --min-weight W         freeze a weight that rests on less than W of the
                         training set
  --min-weight-for P W   the same, for weights whose names start with P
  --ridge L              pull the weights toward the hand-written table by L
  --tune PREFIX          fit only the weights whose names start with PREFIX";

const DATAGEN: &str = "\
usage: cadence datagen <play | extract | run | repair | verify> ...

Self-play games and their positions. Run `cadence datagen <command> --help` for
each command. run, repair and verify are built only with the packed-datagen
feature.";

const DATAGEN_PLAY: &str = "\
usage: cadence datagen play --games N --seed S [--threads N] [--out PATH]

Plays games against itself and writes them to PATH. The same seed and game
count give the same file at any thread count. --threads is cut to the number of
games and to 256.";

const DATAGEN_EXTRACT: &str = "\
usage: cadence datagen extract <games> --train PATH --holdout PATH [--every N]

Cuts a games file into positions, sending every Nth game to the holdout set. It
refuses an input with repeated seeds or games, or an edited header.";

const DATAGEN_RUN: &str = "\
usage: cadence datagen run --out DIR --run-seed HEX|fresh [--threads N]
                           [--chunk-games N] [--standard-share P] [--host NAME]

Writes packed chunks of games into DIR until a STOP file appears there; a PAUSE
file pauses it. The games are played with the network built into the engine,
and every chunk's header names that network's SHA-256. Built only with the
packed-datagen feature.

  --run-seed HEX|fresh   the run's seed, or a fresh one that is recorded
  --threads N            games played at once
  --chunk-games N        games in each chunk
  --standard-share P     the percent of games from standard-chess openings,
                         0 to 100
  --host NAME            the machine name recorded with the run, by default
                         the hostname";

const DATAGEN_REPAIR: &str = "\
usage: cadence datagen repair CHUNK.tmp

Closes a chunk a killed run left behind, keeping its whole records. Built only
with the packed-datagen feature.";

const DATAGEN_VERIFY: &str = "\
usage: cadence datagen verify PATH... [--replay 1/N] [--threads N] [--manifest]

Checks chunks against their sidecars, this build's settings and network, and
the rules, and refuses a chunk that names another network. Built only with the
packed-datagen feature.

  --replay 1/N   regenerate every Nth game and compare it move for move
  --threads N    threads to use
  --manifest     write a directory's MANIFEST.tsv once every chunk in it passes";

#[cfg(test)]
mod tests {
    use super::text;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn every_subcommand_has_help_that_names_it() {
        for line in [
            "bench",
            "perft",
            "spsa",
            "texel",
            "datagen",
            "datagen play",
            "datagen extract",
            "datagen run",
            "datagen repair",
            "datagen verify",
        ] {
            let help = text(&words(line)).unwrap_or_else(|| panic!("no help for {line}"));
            assert!(help.starts_with(&format!("usage: cadence {line}")), "{line}");
        }
        assert_eq!(text(&words("unknown")), None);
        assert_eq!(text(&[]), None);
    }
}
