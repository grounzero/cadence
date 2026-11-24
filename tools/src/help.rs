// SPDX-License-Identifier: GPL-3.0-or-later

//! Each subcommand's `--help` text, kept here so the README can stay a summary.

/// The help for the subcommand `name`, or `None` for one this binary does not have.
#[must_use]
pub fn text(name: &str) -> Option<&'static str> {
    match name {
        "magics" => Some(MAGICS),
        "convert" => Some(CONVERT),
        "forward" => Some(FORWARD),
        _ => None,
    }
}

const MAGICS: &str = "\
usage: cadence-tools magics [seed]

Searches for the sliding-attack magic numbers and prints them as Rust, from the
seed given or a fixed one.";

const CONVERT: &str = "\
usage: cadence-tools convert --manifest FILE [--manifest FILE]... --out DIR
           --shuffle-seed HEX|fresh [--every N] [--standard-share P]
           [--score-cap N] [--swing-cap N] [--dedup] [--max-copies N]
           [--report-sample N] [--adjudicate WIN,PLIES,DRAW,PLIES
           [--adjudicate-share P]] [--piece-keep FILE] [--contradiction-skip K]
           [--mate-score N]

Turns datasets of packed chunks into the trainer's position records, split by
game and shuffled, and publishes them under DIR as one directory named by its
contents. Each chunk is read once to choose the games, and a game repeating an
earlier one from another seed is kept once.

  --manifest FILE        a dataset's manifest; give one for each dataset
  --out DIR              where the unit is published
  --shuffle-seed HEX     the seed for the split, the shuffle and the filters'
                         random choices, or `fresh` for a recorded new one
  --every N              send every Nth game to the holdout (by default 10)
  --standard-share P     the percent of games taken from standard-chess openings
  --score-cap N          drop a position whose score is past N centipawns
  --swing-cap N          once a score reaches N centipawns, drop the rest of
                         that game's positions
  --dedup                keep each position once, within a game and across games
  --max-copies N         keep at most N training copies of a position's network
                         input, a position and its colour-flipped mirror
                         counting as one, meeting the games in an order the
                         shuffle seed fixes, and count the copies dropped;
                         the holdout keeps every copy
  --report-sample N      read the report's repeats, overlaps and copies from one
                         key in N and scale them, for a conversion too large to
                         hold every key; the records written are the same

The report counts the kept positions by the output bucket their piece count
picks and by the result for the side to move, with what the in-check, capture
and mate filters dropped beside them, and again by exact piece count.

Four filters are off unless asked for. Each is named in the unit's manifest and
draws its random choices from the shuffle seed:

  --adjudicate WIN,PLIES,DRAW,PLIES
      cut a game after the ply at which one side's score has stood at WIN
      centipawns or more for PLIES plies running, or every score has stood
      under DRAW for its own run of plies; keep the positions up to there with
      the game's own result, and count how often the rule would have named
      another result
  --adjudicate-share P
      apply --adjudicate in P percent of games (all of them without this)
  --piece-keep FILE
      keep the percent of positions that a table of `PIECES PERCENT` lines
      gives each piece count; the manifest names the table by its SHA-256
  --contradiction-skip K
      skip a position with a chance equal to the gap between its result and
      the win chance its score gives at a slope of K centipawns
  --mate-score N
      keep a mate-scored position with its score held at N, where it would
      otherwise be dropped";

const FORWARD: &str = "\
usage: cadence-tools forward --net FILE [--raw FILE [--against FILE]
           [--scale-factor F [--unscaled FILE]] [--correct-bias OUT]]
           [--positions N] [--seed HEX] [--fens FILE]

Scores random legal positions, at least 1,000, with a net file's quantised
evaluation and with an f32 one written from the same file. It passes when no
position differs by more than 5 centipawns and the mean by no more than 1.

  --net FILE           the net file under test
  --raw FILE           also compare the trainer's unquantised weights; a reading
                       for training time that decides nothing
  --against FILE       compare those weights with the trainer's own scores, as
                       `cadence-train eval` prints them; another such reading
  --scale-factor F     the net was exported with --scale-factor F: hold it to
                       the unquantised weights times F
  --unscaled FILE      the unscaled export of the same checkpoint; the scaled
                       net may differ from the weights by the unscaled net's own
                       error plus at most 5 centipawns, and 1 on average
  --correct-bias OUT   write the net to OUT with its output bias moved against
                       the offset common to every position, which rounding
                       the output weights leaves; needs --raw
  --positions N        how many positions to score
  --seed HEX           the seed for the positions
  --fens FILE          read the positions from FILE, one FEN a line, in place
                       of random ones";

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn every_subcommand_has_help_that_names_it() {
        for name in ["magics", "convert", "forward"] {
            let help = text(name).unwrap_or_else(|| panic!("no help for {name}"));
            assert!(
                help.starts_with(&format!("usage: cadence-tools {name}")),
                "{name}"
            );
        }
        assert_eq!(text("unknown"), None);
    }
}
