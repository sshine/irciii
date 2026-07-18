# Workaround: cargo-readme cannot parse Cargo.toml files that use workspace inheritance (e.g. `version.workspace = true`).
# This module materializes those values into a temporary copy before invoking cargo-readme, then restores the original.

{ ... }:
{
  perSystem =
    { pkgs, ... }:
    {
      packages.cargo-readme-workspace = pkgs.writeShellScriptBin "cargo-readme-workspace" ''
        set -euo pipefail

        check=0
        if [ "$#" -gt 0 ] && [ "$1" = "--check" ]; then
          check=1
        fi

        root="$(git rev-parse --show-toplevel)"
        toml="$root/crates/irciii/Cargo.toml"
        cp "$toml" "$toml.bak"
        trap 'mv "$toml.bak" "$toml"' EXIT
        sed \
          -e 's/^version\.workspace = true/version = "0.1.0"/' \
          -e 's/^edition\.workspace = true/edition = "2024"/' \
          -e 's/^license\.workspace = true/license = "MIT OR Apache-2.0"/' \
          -e 's|^repository\.workspace = true|repository = "https://github.com/sshine/irciii"|' \
          "$toml.bak" > "$toml"
        tmp=$(mktemp)
        trap 'rm -f "$tmp"; mv "$toml.bak" "$toml"' EXIT
        (cd "$root/crates/irciii" && cargo readme --no-title --no-license --no-badges) > "$tmp"
        mv "$toml.bak" "$toml"
        trap - EXIT

        if ! diff -q "$tmp" "$root/README.md" >/dev/null 2>&1; then
          if [ "$check" -eq 1 ]; then
            diff "$tmp" "$root/README.md" || true
            rm -f "$tmp"
            echo "README.md is out of date. Run 'cargo-readme-workspace' and commit the result." >&2
            exit 1
          fi
          mv "$tmp" "$root/README.md"
          echo "README.md updated."
        else
          rm -f "$tmp"
          echo "README.md is up to date."
        fi
      '';
    };
}
