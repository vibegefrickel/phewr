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

# Clippy with all features and with none, and a cargo check for each single feature; every warning is an error, the lint table is in Cargo.toml.
lint:
    cargo clippy --release --all-targets -- -D warnings
    cargo clippy --release --all-targets --no-default-features -- -D warnings
    for feature in turbo heic video sips; do RUSTFLAGS="-D warnings" cargo check --all-targets --no-default-features --features "$feature" || exit 1; done

# Apply the lint and formatting fixes clippy and rustfmt can do on their own.
fix:
    cargo clippy --release --all-targets --fix --allow-dirty --allow-staged
    cargo fmt

# Tests through nextest (one process per test, better output); falls back to `cargo test`.
test *ARGS:
    if command -v cargo-nextest >/dev/null; then cargo nextest run --release {{ARGS}}; else cargo test --release {{ARGS}}; fi

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

# What `cargo update` would change.
outdated:
    cargo update --dry-run --verbose 2>&1 | grep -E '^\s*(Updating|Unchanged)' || true

# The full gate.
check: fmt-check lint test doc deny

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
# Release (Codeberg, curl installer, Homebrew tap). No CI: everything runs here. `just release`
# needs CODEBERG_TOKEN and the sibling checkout ../homebrew-tap.
# Postponed: no release before Q1 2027 (AGENTS.md, "Release").

owner     := "webgefrickel"
repo      := "phewr"
bin       := "phewr"
version   := `PATH="/opt/homebrew/opt/rustup/bin:$PATH" cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'`
tag       := "v" + version
asset     := bin + "-macos-arm64.tar.gz"
api       := "https://codeberg.org/api/v1/repos/" + owner + "/" + repo
tap_dir   := "../homebrew-tap"
stage     := "dist/stage"

# The archive is a prefix (bin/) with LICENSE, README.md and its checksum. The binary is Apple
# Silicon only, ad-hoc signed, in the `dist` profile. It links Homebrew's libheif. ffmpeg, ffprobe
# and mpv are programs on PATH at run time.
# Ad-hoc signed arm64 binary, packed as a prefix archive with its checksum.
dist:
    cargo build --profile dist --target aarch64-apple-darwin
    rm -rf {{stage}} && mkdir -p {{stage}}/bin
    cp target/aarch64-apple-darwin/dist/{{bin}} {{stage}}/bin/{{bin}}
    codesign -s - -f {{stage}}/bin/{{bin}}
    {{stage}}/bin/{{bin}} --version | grep -qx '{{bin}} {{version}}'
    cp LICENSE README.md {{stage}}/
    tar -czf dist/{{asset}} -C {{stage}} bin LICENSE README.md
    shasum -a 256 dist/{{asset}} | cut -d' ' -f1 > dist/{{asset}}.sha256
    @echo "dist/{{asset}} ($(lipo -archs {{stage}}/bin/{{bin}})) sha256 $(cat dist/{{asset}}.sha256)"

# Fill both templates into dist/ without touching any other repo (to inspect before a release).
render: dist
    #!/bin/sh
    set -eu
    sha=$(cat dist/{{asset}}.sha256)
    for t in {{bin}}.rb install.sh; do
      sed -e "s/@VERSION@/{{version}}/g" -e "s/@SHA256@/$sha/g" packaging/$t.in > dist/$t
    done
    sh -n dist/install.sh && ruby -c dist/{{bin}}.rb >/dev/null && echo "dist/{{bin}}.rb, dist/install.sh ok"

# Refuses a dirty tree or an existing tag.
# Tag, create the Codeberg release, upload the archive and install.sh.
publish: render
    #!/bin/sh
    set -eu
    : "${CODEBERG_TOKEN:?export CODEBERG_TOKEN first (Codeberg > Settings > Applications)}"
    [ -z "$(git status --porcelain)" ] || { echo "commit everything first"; exit 1; }
    ! git rev-parse -q --verify "refs/tags/{{tag}}" >/dev/null || { echo "{{tag}} exists"; exit 1; }
    git tag {{tag}} && git push origin {{tag}}
    id=$(curl -fsS -X POST "{{api}}/releases" \
      -H "Authorization: token $CODEBERG_TOKEN" \
      -H "Content-Type: application/json" \
      -d '{"tag_name":"{{tag}}","name":"{{tag}}"}' | jq -er .id)
    for file in {{asset}} install.sh; do
      curl -fsS -X POST "{{api}}/releases/$id/assets?name=$file" \
        -H "Authorization: token $CODEBERG_TOKEN" \
        -F "attachment=@dist/$file" >/dev/null
    done
    echo "Published {{tag}}"

# The Homebrew formula in the tap repo, filled from packaging/phewr.rb.in.
tap:
    #!/bin/sh
    set -eu
    [ -d "{{tap_dir}}/.git" ] || { echo "clone homebrew-tap next to this repo first"; exit 1; }
    mkdir -p {{tap_dir}}/Formula
    cp dist/{{bin}}.rb {{tap_dir}}/Formula/{{bin}}.rb
    cd {{tap_dir}} && git add -A && git commit -m "{{bin}} {{version}}" && git push

# Full release: the gate, then the Codeberg release and the tap.
release: check publish tap
