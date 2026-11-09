# Task runner. `just` lists the recipes; `just check` is what a commit has to pass.
# Homebrew's rustup is keg-only, so the toolchain shims are put on PATH here.

set shell := ["zsh", "-euo", "pipefail", "-c"]
export PATH := "/opt/homebrew/opt/rustup/bin:" + env("PATH")

# turbojpeg-sys links Homebrew's libjpeg-turbo (static) instead of building it with cmake.
export TURBOJPEG_SOURCE      := env("TURBOJPEG_SOURCE", "explicit")
export TURBOJPEG_LIB_DIR     := env("TURBOJPEG_LIB_DIR", "/opt/homebrew/opt/jpeg-turbo/lib")
export TURBOJPEG_INCLUDE_DIR := env("TURBOJPEG_INCLUDE_DIR", "/opt/homebrew/opt/jpeg-turbo/include")

prefix := env("PREFIX", env("HOME") + "/.local")

# Everything a commit must pass: formatting, lints, tests, docs, dependency policy.
default: check

# Format in place.
fmt:
    cargo fmt

# Formatting as a check (no changes).
fmt-check:
    cargo fmt --check

# The features of the app without the native and the JPEG XL decoders of src/pictures/: a copy
# of the module must build and pass its tests without heic, jxl and turbo.
lean_features := "video,sips"

# Clippy with all features, with none and with the lean set, and a cargo check for each single feature; every warning is an error, the lint table is in Cargo.toml.
lint:
    cargo clippy --release --all-targets -- -D warnings
    cargo clippy --release --all-targets --no-default-features -- -D warnings
    cargo clippy --release --all-targets --no-default-features --features {{lean_features}} -- -D warnings
    for feature in turbo heic jxl video sips; do RUSTFLAGS="-D warnings" cargo check --all-targets --no-default-features --features "$feature" || exit 1; done

# Apply the lint and formatting fixes clippy and rustfmt can do on their own.
fix:
    cargo clippy --release --all-targets --fix --allow-dirty --allow-staged
    cargo fmt

# Tests through nextest (one process per test, better output); falls back to `cargo test`.
test *ARGS:
    if command -v cargo-nextest >/dev/null; then cargo nextest run --release {{ARGS}}; else cargo test --release {{ARGS}}; fi

# The tests without heic, jxl and turbo: the picture module as a copy without these decoders.
test-lean *ARGS:
    just test --no-default-features --features {{lean_features}} {{ARGS}}

# Also decode real files: PHEWR_TEST_JPEG, _HEIC, _AVIF, _JXL, _RAW, _PSD, _HDR_VIDEO and _SDR_VIDEO
# name one file each.
# A test without its variable skips with a note. FILTER selects tests, for example `just test-real heic`.
# The benches set the global color target of the process, so they run in `just bench` only.
test-real *FILTER:
    cargo test --release -- --include-ignored --show-output --skip bench:: {{FILTER}}

# The speed measurements of src/bench/ on a real folder (grid, fullscreen open, prefetch,
# Kitty payload). DIR holds the test images, for example a copy of an iPhone folder.
# PHEWR_BENCH_COLORSPACE=srgb measures with the sRGB target.
bench DIR=(env("HOME") / "Pictures/phewr-test/01-mixed-big") FILTER="bench::":
    PHEWR_TEST_DIR={{DIR}} cargo test --release {{FILTER}} -- --ignored --nocapture --test-threads=1 2>&1 | grep -E 'bench [a-z]|^error|panicked|test result' | sed -E 's/^test [^ ]+ \.\.\. //'

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
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

# Licenses, advisories, duplicate versions and sources, see deny.toml.
deny:
    cargo deny check

# The direct dependencies with a newer version (cargo-outdated from Homebrew).
outdated:
    cargo outdated --root-deps-only

# The full gate.
check: fmt-check lint test test-lean doc deny

# Release build with the default features (turbo, heic, video).
build:
    cargo build --release

# Build and start the gallery.
run *ARGS: build
    ./target/release/phewr {{ARGS}}

# Install into a prefix (default ~/.local): bin/phewr.
install: build
    install -d "{{prefix}}/bin"
    install -m 755 target/release/phewr "{{prefix}}/bin/phewr"

# Remove what `just install` put into the prefix.
uninstall:
    rm -f "{{prefix}}/bin/phewr"

# Remove the build output.
clean:
    cargo clean
    rm -rf dist

# Install the git pre-commit hook (fmt + clippy on every commit).
hooks:
    git config core.hooksPath .githooks
    @echo "pre-commit hook active; 'git config --unset core.hooksPath' turns it off"

# ---------------------------------------------------------------------------------------------
# The shared picture module (docs/picture-refactor.md of schnattr). src/pictures/ here is the
# master copy, and <target>/src/pictures/ in another project is a copy. Change the module only
# in the master copy, then run `just pictures-sync <target>`. The target has no default: name the
# root of the other project, for example ../schnattr or ../youtr. A sync into a project needs the
# go of the session of that project. Only the copy has SOURCE: one `<sha256>  <path>` line per
# file, sorted, so `shasum -a 256 -c SOURCE` in the copy reads it. The master copy must not have
# a SOURCE. Neither recipe changes the git state of either project.

# The sha256 lines of the .rs files below the current directory, in the format of SOURCE.
pictures_sums := "find . -type f -name '*.rs' | sed 's|^\\./||' | LC_ALL=C sort | xargs shasum -a 256"

# Mirror the .rs files of the module into the copy (a file that the master copy no longer has
# goes, SOURCE stays), write SOURCE, then `cargo fmt --check` in both projects and clippy in the
# target. A file that did not change keeps its mtime. When the .rs files of the copy differ from
# its SOURCE, the copy has edits of its own: the sync refuses and lists them, and
# PICTURES_SYNC_FORCE=1 drops them. PICTURES_SYNC_SKIP_CARGO=1 skips the cargo steps, for a
# target that is not a crate.
pictures-sync target:
    #!/bin/sh
    set -eu
    target={{quote(target)}}
    master=src/pictures
    copy="$target/src/pictures"
    [ ! -e "$master/SOURCE" ] || { echo "pictures-sync: $master/SOURCE must not exist in the master copy" >&2; exit 1; }
    [ -d "$target/src" ] || { echo "pictures-sync: $target/src does not exist; give the root of the other project" >&2; exit 1; }
    if [ -f "$copy/SOURCE" ] && [ "${PICTURES_SYNC_FORCE:-}" != 1 ]; then
        edited=$( (cd "$copy" && {{pictures_sums}}) | sort - "$copy/SOURCE" | uniq -u | cut -c67- | LC_ALL=C sort -u)
        [ -z "$edited" ] || { printf '%s\n' "pictures-sync: these files of the copy differ from its SOURCE; port the edits to the master copy, or run with PICTURES_SYNC_FORCE=1 to drop them:" "$edited" | sed '2,$s/^/  /' >&2; exit 1; }
    fi
    mkdir -p "$copy"
    (cd "$master" && find . -type f -name '*.rs' | sed 's|^\./||' | LC_ALL=C sort) | while read -r file; do
        cmp -s "$master/$file" "$copy/$file" && continue
        mkdir -p "$(dirname "$copy/$file")"
        cp "$master/$file" "$copy/$file"
        echo "pictures-sync: copied $file"
    done
    (cd "$copy" && find . -type f ! -path ./SOURCE | sed 's|^\./||' | LC_ALL=C sort) | while read -r file; do
        [ -f "$master/$file" ] && [ "${file%.rs}" != "$file" ] && continue
        rm "$copy/$file"
        echo "pictures-sync: removed $file"
    done
    find "$copy" -mindepth 1 -type d -empty -delete
    (cd "$copy" && {{pictures_sums}}) > "$copy/SOURCE.new"
    if cmp -s "$copy/SOURCE.new" "$copy/SOURCE"; then rm "$copy/SOURCE.new"; else mv "$copy/SOURCE.new" "$copy/SOURCE" && echo "pictures-sync: wrote SOURCE"; fi
    if [ "${PICTURES_SYNC_SKIP_CARGO:-}" = 1 ]; then echo "pictures-sync: cargo steps skipped (PICTURES_SYNC_SKIP_CARGO=1)"; exit 0; fi
    name=$(basename "$(cd "$target" && pwd)")
    failed=""
    cargo fmt --check || failed="$failed\n  phewr: cargo fmt --check"
    (cd "$target" && cargo fmt --check) || failed="$failed\n  $name: cargo fmt --check"
    (cd "$target" && cargo clippy --all-targets -- -D warnings) || failed="$failed\n  $name: cargo clippy"
    [ -z "$failed" ] || { printf '%b\n' "pictures-sync: the copy is in place, but these steps failed:$failed" >&2; exit 1; }
    echo "pictures-sync: $copy matches the master copy"

# Compare the module with the copy: the .rs files of the master copy are all the files of the
# copy (SOURCE aside), with the same bytes, and SOURCE matches the files of the copy. The file set
# is the one that `pictures-sync` mirrors. Lists each difference and fails on one.
pictures-check target:
    #!/bin/sh
    set -eu
    target={{quote(target)}}
    master=src/pictures
    copy="$target/src/pictures"
    [ -d "$copy" ] || { echo "pictures-check: $copy does not exist; run 'just pictures-sync $target' first" >&2; exit 1; }
    scratch=$(mktemp -d)
    trap 'rm -rf "$scratch"' EXIT
    (cd "$master" && find . -type f -name '*.rs' | sed 's|^\./||' | LC_ALL=C sort) > "$scratch/master"
    (cd "$copy" && find . -type f ! -path ./SOURCE | sed 's|^\./||' | LC_ALL=C sort) > "$scratch/copy"
    {
        comm -23 "$scratch/master" "$scratch/copy" | sed 's|^|  only in the master copy: |'
        comm -13 "$scratch/master" "$scratch/copy" | sed 's|^|  only in the copy: |'
        comm -12 "$scratch/master" "$scratch/copy" | while read -r file; do
            cmp -s "$master/$file" "$copy/$file" || echo "  differs: $file"
        done
        [ ! -e "$master/SOURCE" ] || echo "  SOURCE exists in the master copy"
    } > "$scratch/differences"
    (cd "$copy" && {{pictures_sums}}) > "$scratch/sums"
    cmp -s "$scratch/sums" "$copy/SOURCE" 2>/dev/null || echo "  SOURCE does not match the files of the copy" >> "$scratch/differences"
    if [ -s "$scratch/differences" ]; then
        echo "pictures-check: $copy drifted from the master copy:" >&2
        cat "$scratch/differences" >&2
        echo "pictures-check: change the master copy, then run 'just pictures-sync $target'" >&2
        exit 1
    fi
    echo "pictures-check: $copy matches the master copy"
