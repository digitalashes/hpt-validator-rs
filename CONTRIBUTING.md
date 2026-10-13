# Contributing

Open an issue before a large change. A pull request passes:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Default mode returns exactly what upstream returns, so expected results come from
upstream, never by hand: add an input to `tests/cases` and to `oracle/cases.mjs`, then run
`oracle/run.sh` (see [How parity is tested](README.md#how-parity-is-tested)). Only
`tests/extended.json`, the Extended additions upstream cannot produce, is edited by hand.
