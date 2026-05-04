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
    cargo build -p wm --bin mock-vibewm-control

# vibewm dev — boots the wlroots-style compositor in a winit window.
# Pass a "-- <cmd>" suffix to spawn a client into it on startup.
# Examples:
#   just run-vibewm
#   just run-vibewm -- weston-terminal
#   just run-vibewm -- vibeshell-panel
run-vibewm *args:
    cargo run -p vibewm -- {{args}}

# Boot the full vibeshell session against vibewm (WM_BACKEND=wlroots).
# Mirror of run-session but for the wlroots-style compositor instead of sway.
run-vibeshell-session:
    ./scripts/start-vibeshell-session

test:
    cargo test --workspace

smoke-test:
    ./scripts/smoke-test

# Headless smoke test against the wlroots backend path (uses
# `mock-vibewm-control` instead of a real vibewm — no GPU needed).
smoke-test-wlroots:
    ./scripts/smoke-test-wlroots

demo:
    ./scripts/demo

ci: fmt-check clippy check test smoke-binaries smoke-test-wlroots

run-ctl *args:
    cargo run -p vibeshellctl -- {{args}}
