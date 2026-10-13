<!-- What changes and why. Link the issue it closes. -->

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and
      `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` pass
- [ ] Expected results come from the oracle (`oracle/run.sh`); only `tests/extended.json`
      is edited by hand
- [ ] Default output stays byte for byte equal to upstream's
