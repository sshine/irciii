# The webern/cargo-readme fork (git version) — supports Cargo workspaces better
# than the crates.io release.
{ inputs, ... }:
{
  perSystem =
    { pkgs, ... }:
    {
      packages.cargo-readme = pkgs.rustPlatform.buildRustPackage {
        pname = "cargo-readme";
        version = "git";
        src = inputs.cargo-readme-src;
        cargoLock.lockFile = "${inputs.cargo-readme-src}/Cargo.lock";
        # Upstream tests expect a full cargo project fixture layout; skip.
        doCheck = false;
      };
    };
}
