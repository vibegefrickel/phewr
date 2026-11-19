# Task runner. `just` lists the recipes. `just check` is the gate of a commit.
# Homebrew's rustup is keg-only, so the toolchain shims are put on PATH here.

set shell := ["zsh", "-euo", "pipefail", "-c"]
export PATH := "/opt/homebrew/opt/rustup/bin:" + env("PATH")

prefix := env("PREFIX", env("HOME") + "/.local")

# Everything a commit must pass: formatting, lints, tests, docs, dependency policy.
default: check

# Format in place.
fmt:
    cargo fmt

# Formatting as a check (no changes).
fmt-check:
    cargo fmt --check

# The features of the app without the native and the JPEG XL decoders of the picture module of
# tuilibr: phewr must build and pass its tests without heic, jxl and turbo.
lean_features := "video,sips"

# Clippy with the default features, with none and with the lean set. Then a cargo check for each
# single feature. Every warning is an error here through `CARGO_BUILD_WARNINGS=deny`, which
# changes no compiler flag, so rust-analyzer and the gate share their artifacts. The lint table is
# in Cargo.toml.
# `--all-targets` adds the dev dependency `tuilibr/test-support` to the binary too. The two runs
# without it make sure that the binary uses no test item of tuilibr (`just build`).
lint:
    CARGO_BUILD_WARNINGS=deny cargo clippy --release
    CARGO_BUILD_WARNINGS=deny cargo clippy --release --no-default-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --release --all-targets
    CARGO_BUILD_WARNINGS=deny cargo clippy --release --all-targets --no-default-features
    CARGO_BUILD_WARNINGS=deny cargo clippy --release --all-targets --no-default-features --features {{ lean_features }}
    for feature in turbo heic jxl video sips; do CARGO_BUILD_WARNINGS=deny cargo check --all-targets --no-default-features --features "$feature" || exit 1; done

# Apply the lint and formatting fixes clippy and rustfmt can do on their own.
fix:
    cargo clippy --release --all-targets --fix --allow-dirty --allow-staged
    cargo fmt

# Tests through nextest (one process per test, better output). Without nextest, `cargo test` runs.
# phewr is a binary crate, so it has no doc tests. Every warning is an error.
test *ARGS:
    if command -v cargo-nextest >/dev/null; then CARGO_BUILD_WARNINGS=deny cargo nextest run --release {{ ARGS }}; else CARGO_BUILD_WARNINGS=deny cargo test --release {{ ARGS }}; fi

# The tests without heic, jxl and turbo: phewr with the picture module without these decoders.
test-lean *ARGS:
    just test --no-default-features --features {{ lean_features }} {{ ARGS }}

# Also decode real files: PHEWR_TEST_HEIC, _JXL, _HDR_VIDEO, _SDR_VIDEO and _MOTION name one file
# each. PHEWR_TEST_LIVE_DIR names a folder. The tests of the picture module are in tuilibr and
# read TUILIBR_TEST_*.
# A test without its variable skips with a note. FILTER selects tests, for example `just test-real heic`.
# The benches set the global color target of the process, so they run in `just bench` only.
test-real *FILTER:
    cargo test --release -- --include-ignored --show-output --skip bench:: {{ FILTER }}

# The speed measurements of src/bench/ on a real folder (grid, fullscreen open, prefetch,
# Kitty payload). DIR holds the test images, for example a copy of an iPhone folder.
# PHEWR_BENCH_COLORSPACE=srgb measures with the sRGB target.
bench DIR=(env("HOME") / "Pictures/phewr-test/01-mixed-big") FILTER="bench::":
    PHEWR_TEST_DIR={{ DIR }} cargo test --release {{ FILTER }} -- --ignored --nocapture --test-threads=1 2>&1 | grep -E 'bench [a-z]|^error|panicked|test result' | sed -E 's/^test [^ ]+ \.\.\. //'

# The Live Photo benches only (src/bench/live.rs). They read PHEWR_TEST_LIVE_DIR, default
# ~/Pictures/phewr-test/12-live-photos, and print the load average of their minute.
bench-live:
    cargo test --release bench::live -- --ignored --nocapture --test-threads=1 2>&1 | grep -E 'bench [a-z]|^error|panicked|test result' | sed -E 's/^test [^ ]+ \.\.\. //'

# Line coverage summary per file (needs cargo-llvm-cov: `brew install cargo-llvm-cov`).
coverage:
    cargo llvm-cov --release --all-targets --summary-only

# Line coverage as an HTML report in target/llvm-cov/html, opened in the browser.
coverage-html:
    cargo llvm-cov --release --all-targets --html --open

# rustdoc over the whole crate, warnings fatal (broken links, bad code blocks).
doc:
    CARGO_BUILD_WARNINGS=deny cargo doc --no-deps --document-private-items

# Licenses, advisories, duplicate versions and sources, see deny.toml.
deny:
    cargo deny check

# The lock updates that are left, and each direct dependency with a newer version (cargo-outdated fails on the path dependency).
outdated:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo update --dry-run --verbose 2>&1 | grep -E 'Unchanged|Locking'
    found=0
    while read -r name version; do
        latest=$(cargo info "$name@${version#v}" 2>/dev/null | sed -n 's/^version:.*(latest \(.*\))/\1/p')
        if [ -n "$latest" ]; then
            echo "$name: locked ${version#v}, latest $latest"
            found=1
        fi
    done < <(cargo tree --depth 1 -e normal,build,dev --prefix none | tail -n +2 | grep -v -E '^(tuilibr|phewr) ' | grep -v '(\*)' | awk '{print $1, $2}' | sort -u)
    if [ "$found" = 0 ]; then echo "all root dependencies are current"; fi

# The full gate.
check: fmt-check lint test test-lean doc deny audit

# The checks outside cargo: the TOML format, the justfile format, the shell scripts of the fixtures,
# the spelling, the unused dev-dependencies. A missing tool fails the recipe with its brew line.
# Install the tools once:
# `brew install taplo shellcheck shfmt typos-cli cargo-shear cargo-insta`.
audit:
    #!/usr/bin/env zsh
    set -euo pipefail
    need() { command -v "$1" >/dev/null || { echo "audit: $1 is not installed, brew install $2"; exit 1; }; }
    need taplo taplo
    taplo fmt --check
    just --fmt --check --unstable
    need shellcheck shellcheck
    shellcheck -S warning tests/fixtures/make.sh tests/fixtures/*/make.sh
    need shfmt shfmt
    shfmt -d -i 2 tests/fixtures/make.sh tests/fixtures/*/make.sh
    need typos typos-cli
    typos
    need cargo-shear cargo-shear
    cargo shear

# Release build with the default features (turbo, heic, video).
build:
    cargo build --release

# Build and start the gallery.
run *ARGS: build
    ./target/release/phewr {{ ARGS }}

# Install into a prefix (default ~/.local): bin/phewr.
install: build
    install -d "{{ prefix }}/bin"
    install -m 755 target/release/phewr "{{ prefix }}/bin/phewr"

# Remove what `just install` put into the prefix.
uninstall:
    rm -f "{{ prefix }}/bin/phewr"

# Remove the build output.
clean:
    cargo clean
    rm -rf dist
