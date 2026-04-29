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
    cargo build -p cheatsheet --bins
    cargo build -p vibeshellctl --bins
    cargo build -p vibewm --bins

# vibewm dev — boots the wlroots-style compositor in a winit window.
# Pass a "-- <cmd>" suffix to spawn a client into it on startup.
# Examples:
#   just run-vibewm
#   just run-vibewm -- weston-terminal
#   just run-vibewm -- vibeshell-panel
run-vibewm *args:
    cargo run -p vibewm -- {{args}}

test:
    cargo test --workspace

smoke-test:
    ./scripts/smoke-test

demo:
    ./scripts/demo

ci: fmt-check clippy check test smoke-binaries

run-ctl *args:
    cargo run -p vibeshellctl -- {{args}}
