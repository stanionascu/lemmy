#!/bin/bash
set -e

CWD="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

cd $CWD/../

cargo clippy --workspace --fix --allow-staged --allow-dirty --tests --all-targets --all-features -- \
  -D warnings -D deprecated -D clippy::perf -D clippy::complexity \
  -D clippy::style -D clippy::correctness -D clippy::suspicious \
  -D clippy::dbg_macro -D clippy::inefficient_to_string \
  -D clippy::items-after-statements -D clippy::implicit_clone \
  -D clippy::wildcard_imports -D clippy::cast_lossless \
  -D clippy::manual_string_new -D clippy::redundant_closure_for_method_calls \
  -D clippy::unused_self \
  -A clippy::uninlined_format_args

cargo +nightly fmt
