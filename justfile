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

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

check:
    cargo check --workspace

ci: fmt clippy check
