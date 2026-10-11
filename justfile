# Default recipe: list available commands
default:
    @just --list

# Format all code (Rust + Nix + Markdown)
fmt:
    treefmt

# Check formatting (Rust + Nix + Markdown)
fmt-check:
    treefmt --fail-on-change --no-cache

# Run clippy lints
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Run all tests
test:
    cargo test --all-features

# Run tests with verbose output
test-verbose:
    cargo test --all-features -- --nocapture

# Run network tests (connects to irc.libera.chat)
test-net:
    cargo test -p irciii-daemon -- --ignored

# Build release
build:
    cargo build --release --all-features

# Generate documentation
doc *args='':
    cargo doc --no-deps --all-features {{args}}

readme_args := "--project-root crates/irciii --no-title --no-license --no-badges"

# Regenerate README.md from the irciii crate docs
readme:
    cargo readme {{readme_args}} > README.md

# Check README.md is in sync with the crate docs
readme-check:
    cargo readme {{readme_args}} | diff - README.md

release_targets := "x86_64-unknown-linux-musl aarch64-unknown-linux-musl"

# Assert the release tag names the version cargo would build
check-version version:
    @pkgid="$(cargo pkgid -p irciii)"; crate="v${pkgid##*#}"; \
    if [ "$crate" != "{{version}}" ]; then \
        echo "tag {{version}} does not match crate version $crate" >&2; exit 1; \
    fi

# Build the static release artifacts and their checksums into dist/
dist version: (check-version version)
    rm -rf dist && mkdir -p dist
    for target in {{release_targets}}; do \
        cargo build --release --locked --target "$target" -p irciii; \
        name="irciii-{{version}}-$target"; \
        tar -czf "dist/$name.tar.gz" -C "target/$target/release" irciii; \
        ( cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256" ); \
    done
    @ls -1 dist

# Print the SRI hashes nix/binary.nix pins, read from the published release.
# Local builds are not bit-identical to the ones CI publishes, so hashing dist/
# would give values that never match what fetchurl downloads.
release-hashes version:
    @for target in {{release_targets}}; do \
        url="https://github.com/sshine/irciii/releases/download/{{version}}/irciii-{{version}}-$target.tar.gz"; \
        hash="$(nix store prefetch-file --json --hash-type sha256 "$url" | jq -r .hash)"; \
        printf '"%s" = "%s";\n' "${target%%-*}-linux" "$hash"; \
    done

# Run CI checks locally
ci: fmt-check lint test doc readme-check build
    @echo "All CI checks passed!"

# Run the client with hot-reloading of the irciii-ui dylib
dev:
    #!/usr/bin/env bash
    set -m
    cargo build -p irciii-ui
    cargo watch -w crates/irciii-ui -x 'build -p irciii-ui' &
    watcher=$!
    trap 'kill $watcher 2>/dev/null' EXIT
    cargo run -p irciii --features irciii-client/hot-reload

# Watch for changes and run tests
watch:
    cargo watch -x test

# Clean build artifacts
clean:
    cargo clean

# Review snapshot test changes
snap:
    cargo insta test --review
