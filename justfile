set shell := ["bash", "-uc"]

# Run the app.
run:
    cargo run -p archroom-app

# Run the CLI (`just cli -- catalog check /tmp/test.arcat`).
cli *ARGS:
    cargo run -p archroom-cli -- {{ARGS}}

# Run every test in the workspace.
test:
    cargo test --workspace --all-targets

# Format check, clippy (deny warnings) and the crate dependency-rule check.
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo xtask check-deps

# Auto-fix formatting.
fmt:
    cargo fmt --all

# License and advisory check (needs `cargo install cargo-deny`).
deny:
    cargo deny check

bench:
    cargo bench --workspace

# Download the golden-image raw set (plan §13). TODO(M3): the fixture list
# and download script land with the golden-image test suite.
fixtures:
    @echo "TODO(M3): tests/fixtures download script — plan §13"
    @exit 1

# Re-bless golden-image references after an intentional rendering change
# (plan §13): rewrites tests/golden/*.png; review the diff before committing.
bless:
    ARCHROOM_BLESS=1 cargo test -p archroom-engine --test golden

# Everything CI runs.
ci: lint test
