run-nested:
    ./scripts/run-in-nested-sway

run-session:
    ./scripts/start-sway-session

run-session-sway-only:
    VIBESHELL_SWAY_ONLY=1 ./scripts/start-sway-session

run-panel:
    cargo run -p panel

run-launcher:
    cargo run -p launcher

run-notifd:
    cargo run -p notifd

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

check:
    cargo check --workspace

smoke-binaries:
    cargo build -p panel --bins
    cargo build -p launcher --bins
    cargo build -p notifd --bins
    cargo build -p vibeshellctl --bins

smoke-test:
    ./scripts/smoke-test

ci: fmt-check clippy check smoke-binaries

run-ctl *args:
    cargo run -p vibeshellctl -- {{args}}
