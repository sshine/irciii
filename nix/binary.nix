# Prebuilt release binaries, for consumers who want irciii without a Rust toolchain
# entering their build closure.
#
# The pinned version deliberately trails Cargo.toml: it names the last tag whose
# artifacts are published, not whatever the working tree happens to claim. Bump it
# once a tag's Release workflow has finished; `just release-hashes <tag>` prints the
# values.
#
# The hashes are of the tarballs the release publishes, so they can be checked against
# the .sha256 assets rather than taken on faith. Systems absent from `hashes` do not get
# the package, which is what keeps it off Darwin, since the release builds only Linux.
{ ... }:
{
  perSystem =
    {
      system,
      pkgs,
      lib,
      ...
    }:
    let
      release = {
        version = "0.1.0";
        hashes = {
          "x86_64-linux" = "sha256-vepuQtixt+Nlc2aDcgKedPV0dTBawTUH2OYReUFTtPE=";
          "aarch64-linux" = "sha256-ZAbNjzK7t1XkY0D5SvsCFA/BjQMEBjWXFvbp2uutnp0=";
        };
      };

      triple = {
        "x86_64-linux" = "x86_64-unknown-linux-musl";
        "aarch64-linux" = "aarch64-unknown-linux-musl";
      };

      tag = "v${release.version}";

      tarball = pkgs.fetchurl {
        url = "https://github.com/sshine/irciii/releases/download/${tag}/irciii-${tag}-${triple.${system}}.tar.gz";
        hash = release.hashes.${system};
      };
    in
    lib.optionalAttrs (release.hashes ? ${system}) {
      # No wrapper: the client re-executes itself to spawn the daemon, via
      # current_exe rather than a PATH lookup, so nothing has to be on PATH.
      packages.irciii-bin =
        pkgs.runCommand "irciii-bin-${release.version}"
          {
            meta = {
              description = "A minimal irssi-like terminal IRC client (prebuilt)";
              mainProgram = "irciii";
              license = with lib.licenses; [
                mit
                asl20
              ];
              platforms = lib.attrNames release.hashes;
              sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
            };
          }
          ''
            tar -xzf ${tarball}
            install -Dm755 irciii $out/bin/irciii
          '';
    };
}
