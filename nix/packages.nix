# Workspace packages via buildRustPackage.
#
# crate2nix (the walltime-rs template's choice) cannot handle the promptui-core
# git dependency: it vendors the crate out of its workspace, which breaks its
# `edition.workspace = true` inheritance. importCargoLock keeps the fetched git
# workspace intact, so buildRustPackage works; `allowBuiltinFetchGit` avoids
# having to maintain outputHashes for git dependencies.
{ ... }:
{
  perSystem =
    { pkgs, lib, ... }:
    rec {
      packages.default = pkgs.rustPlatform.buildRustPackage {
        pname = "irciii";
        version = "0.1.0";
        src = ../.;
        cargoLock = {
          lockFile = ../Cargo.lock;
          allowBuiltinFetchGit = true;
        };
        buildAndTestSubdir = "crates/irciii";
        meta.mainProgram = "irciii";
      };

      checks.irciii = packages.default;

      apps.default = {
        type = "app";
        program = lib.getExe packages.default;
      };
    };
}
