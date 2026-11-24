// SPDX-License-Identifier: GPL-3.0-or-later

//! Each subcommand's `--help` text, kept here so the README can stay a summary.

/// The help for the subcommand `name`, or `None` for one this binary does not have.
#[must_use]
pub fn text(name: &str) -> Option<&'static str> {
    match name {
        "run" => Some(RUN),
        "validate" => Some(VALIDATE),
        "export" => Some(EXPORT),
        "book" => Some(BOOK),
        "overfit" => Some(OVERFIT),
        "eval" => Some(EVAL),
        _ => None,
    }
}

const RUN: &str = "\
usage: cadence-train run --data FILE... --manifest FILE... --holdout FILE --out DIR
           --id NAME [--superbatches N] [--batches N] [--result-share F]
           [--validation-positions N] [--hidden 256|512] [--lr-schedule cosine|step]
           [--result-share-end F] [--from CHECKPOINT] [--lr-start F]

Trains the network on the trainer's position records with bullet. It needs a
GPU and the cuda feature.

Each data file must hash to the training file a manifest names, written by
`cadence-tools convert` or `cadence-train book`, or the run stops before it
starts. Two files with the same records are refused, the holdout must be the
holdout split of a manifest a data file matched, and a run never shares its
directory with another's. The manifests it verified are listed by SHA-256 in
`verified-manifests` beside the checkpoints. After training, each saved
checkpoint is scored over the holdout's first 20,000 records by the trainer's
own forward pass, and its validation loss is printed beside the training loss
it logged.

The defaults are the net's recipe: 40 superbatches of 6,104 batches, the target
half result and half score, 256 units a perspective.

  --superbatches N           superbatches to train
  --batches N                batches in each superbatch
  --result-share F           the weight on the game result in the target, 0 to 1
  --result-share-end F       move that weight on a line to F by the last
                             superbatch; the run is recorded and scored at F
  --validation-positions N   holdout records each checkpoint is scored over
  --hidden 256|512           units a perspective; 512 clips the output weights
                             to 1.0 so that the engine can load it
  --lr-schedule cosine|step  cosine decay, or five equal drops to the same
                             final rate
  --from CHECKPOINT          start from a saved checkpoint's weights and
                             optimiser state, of the same width
  --lr-start F               start the learning rate at F in place of the
                             recipe's first rate; with --from and a result share
                             of 1 for a few superbatches, a final stage on the
                             game result alone";

const VALIDATE: &str = "\
usage: cadence-train validate --checkpoint DIR... --holdout FILE --manifest FILE...
           [--validation-positions N] [--result-share F] [--score-share F]

Prints the validation loss `run` prints, for checkpoints already saved. It needs
a GPU and the cuda feature. The checkpoints must come from runs at one result
share.

  --validation-positions N   holdout records each checkpoint is scored over
  --result-share F           the result share for a run that did not record one
  --score-share F            score the loss at this result share instead of the
                             run's, to compare nets trained at different blends";

const EXPORT: &str = "\
usage: cadence-train export --checkpoint DIR --manifest FILE --provenance NAME --out FILE
           [--scale-factor F]

Writes one checkpoint as the file EvalFile loads, naming the data's manifest and
the commit that trained it. The manifest must be one its run verified.

  --scale-factor F   multiply the net's evaluations by F. The output weights and
                     bias are taken from the checkpoint's unquantised weights,
                     multiplied and quantised once, and the first layer is left
                     as saved. The export is refused if those weights are not the
                     checkpoint's or the result does not load. The file does not
                     record the factor; `cadence-tools forward --raw FILE
                     --scale-factor F --unscaled FILE` checks it.";

const BOOK: &str = "\
usage: cadence-train book --in FILE --out FILE --seed HEX [--every N]

Turns a tuning set's text, one position a line as a FEN and White's result in
brackets, into the trainer's records, scored zero. Every Nth line (by default
the tenth) is held out to FILE.holdout, each part is shuffled from the seed, and
a manifest beside them names both; an export hashes it into the net's header.";

const OVERFIT: &str = "\
usage: cadence-train overfit --out DIR [--data FILE... --manifest FILE...]

The check before a full run: a small synthetic set must be memorised at the
stated result share, or the check fails. With --data and --manifest it also
reads real data's loss against a constant guess's.";

const EVAL: &str = "\
usage: cadence-train eval --checkpoint DIR --fens FILE

Prints the trainer's own score for each FEN in the file, in centipawns, from one
saved checkpoint and the same network `run` builds. It needs a GPU and the cuda
feature.";

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn every_subcommand_has_help_that_names_it() {
        for name in ["run", "validate", "export", "book", "overfit", "eval"] {
            let help = text(name).unwrap_or_else(|| panic!("no help for {name}"));
            assert!(
                help.starts_with(&format!("usage: cadence-train {name}")),
                "{name}"
            );
        }
        assert_eq!(text("unknown"), None);
    }
}
