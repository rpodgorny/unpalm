build:
  cargo build --release

test:
  cargo nextest run

check:
  cargo check

clippy:
  cargo clippy

fmt:
  cargo fmt

audit:
  cargo audit
