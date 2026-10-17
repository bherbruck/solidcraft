#!/usr/bin/env bash
# Self-hosted runner only: keep a job's Cargo build directory between runs, outside the
# checkout (actions/checkout's clean would delete it), and point CARGO_TARGET_DIR at it for the
# rest of the job. Usage: warm-target.sh NAME
# Each self-hosted runner has its own /home/runner, so two never share a directory.
set -euo pipefail
name=${1:?usage: warm-target.sh NAME}
dir=/home/runner/work/cargo-target/$name
# Start over when it has grown past 40 GB: old dependency versions pile up.
if [ -d "$dir" ] && [ "$(du -sm "$dir" | cut -f1)" -gt 40000 ]; then
  echo "warm-target: $dir is over 40 GB, starting a fresh build"
  rm -rf "$dir"
fi
mkdir -p "$dir"
echo "CARGO_TARGET_DIR=$dir" >> "$GITHUB_ENV"
echo "warm-target: $dir ($(du -sh "$dir" | cut -f1))"
