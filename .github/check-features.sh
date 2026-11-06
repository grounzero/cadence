#!/usr/bin/env bash
# Every rustc command line in a verbose cargo build log must resolve to the target's baseline with
# the features named, and name no target-cpu. Usage: check-features.sh TARGET FEATURES LOG
set -eu
target=$1
features=$2
log=$3
base=$(rustc --print cfg --target "$target" -C "target-feature=$features" | grep target_feature | sort)
lines=$(grep -E '^ +Running `.*rustc(\.exe)? .*--crate-name ' "$log" || true)
for crate in cadence_core cadence_engine cadence; do
  if ! grep -q -- "--crate-name $crate " <<< "$lines"; then
    echo "::error::no rustc invocation for $crate in the release build log"
    exit 1
  fi
done
n=0
while read -r line; do
  name=$(echo "$line" | grep -oE -- '--crate-name [^ ]+')
  # A target-cpu brings in features no target-feature names, whatever those resolve to.
  cpu=$(echo "$line" | grep -oE "target-cpu=[^ '\`\"]+" || true)
  if [ -n "$cpu" ]; then
    echo "::error::$name was built with $cpu, which brings in features no flag names"
    exit 1
  fi
  set --
  for f in $(echo "$line" | grep -oE "target-feature=[^ '\`\"]+"); do set -- "$@" -C "$f"; done
  [ "$#" -gt 0 ] || { echo "::error::$name was built with no target feature"; exit 1; }
  got=$(rustc --print cfg --target "$target" "$@" | grep target_feature | sort)
  if [ "$got" != "$base" ]; then
    echo "::error::$name resolves to $(echo $got) and not the baseline with $features"
    exit 1
  fi
  n=$((n + 1))
done <<< "$lines"
echo "$n rustc invocations, every one at the baseline with $features: $(echo $base)"
