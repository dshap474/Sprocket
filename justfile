test:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test --lib
    cargo test --test bootstrap
    cargo test --test observation
    cargo test --test checkpoint_flow
    cargo test --test promotion
    cargo test --test guards_runtime

test-all:
    just test
    cargo test --test e2e_cli -- --include-ignored
    cargo test --doc
