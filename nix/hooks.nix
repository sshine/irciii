# Git hooks via hk-nix. The generated hk.pkl is symlinked into the repo root by
# the devshell startup hook (see devshell.nix); hk.pkl is gitignored.
{ inputs, ... }:
{
  imports = [ inputs.hk-nix.flakeModules.default ];
  perSystem =
    {
      config,
      pkgs,
      lib,
      ...
    }:
    let
      # Flags to reproduce the committed README.md from the irciii crate docs.
      # The webern/cargo-readme fork understands workspace inheritance, so no
      # Cargo.toml materialization workaround is needed.
      readmeArgs = "--project-root crates/irciii --no-title --no-license --no-badges";

      # Reference treefmt by absolute store path: the `nix flake check` hk-check
      # sandbox runs the pre-commit hook without the devshell PATH, so a bare
      # `treefmt` is not found there (the deadnix/conventional steps do the same).
      treefmt = lib.getExe config.treefmt.build.wrapper;
    in
    {
      hk-nix.settings.hooks = {
        "pre-commit" = {
          fix = true;
          stash = "git";
          steps.treefmt = {
            check = "${treefmt} --fail-on-change --no-cache {{files}}";
            fix = "${treefmt} {{files}}";
          };
        };

        "pre-push".steps = {
          deadnix = {
            glob = "*.nix";
            check = "${lib.getExe pkgs.deadnix} --fail {{files}}";
          };
          clippy = {
            check = "cargo clippy --all-targets --all-features -- -D warnings";
          };
          readme = {
            check = "cargo readme ${readmeArgs} | diff - README.md";
            fix = "cargo readme ${readmeArgs} -o README.md";
          };
          lock-check = {
            check = "cargo metadata --locked --format-version 1 > /dev/null";
          };
        };

        # Conventional commits; policy knobs: --require-scope, --types feat,fix,...
        "commit-msg".steps.conventional = {
          check = "${lib.getExe config.packages.conventional-commit-check} {{commit_msg_file}}";
        };
      };
    };
}
