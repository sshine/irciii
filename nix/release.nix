# Cross-compilation for the release artifacts.
#
# Releases ship statically linked musl binaries so they run on any Linux without
# matching a glibc version, which is the common failure mode for a distributed
# terminal binary.
#
# rustls pulls in aws-lc-sys, which compiles C with cmake, so a musl C toolchain
# is needed even for the host architecture: the cc crate's `CC_<target>` and the
# cargo linker variable both have to point at it.
{ ... }:
{
  perSystem =
    { pkgs, ... }:
    let
      targets = {
        "x86_64-unknown-linux-musl" = pkgs.pkgsCross.musl64.stdenv.cc;
        "aarch64-unknown-linux-musl" = pkgs.pkgsCross.aarch64-multiplatform-musl.stdenv.cc;
      };

      # cargo spells its target variables in SCREAMING_SNAKE_CASE, the cc crate in
      # the target triple's own case; both have to be set for a cross build.
      toolFor = target: tool: "${targets.${target}}/bin/${target}-${tool}";
      shout = target: builtins.replaceStrings [ "-" ] [ "_" ] (pkgs.lib.toUpper target);

      env = pkgs.lib.flatten (
        pkgs.lib.mapAttrsToList (target: _: [
          {
            name = "CARGO_TARGET_${shout target}_LINKER";
            value = toolFor target "cc";
          }
          {
            name = "CC_${builtins.replaceStrings [ "-" ] [ "_" ] target}";
            value = toolFor target "cc";
          }
          {
            name = "AR_${builtins.replaceStrings [ "-" ] [ "_" ] target}";
            value = toolFor target "ar";
          }
        ]) targets
      );
    in
    {
      devshells.default = {
        packages = builtins.attrValues targets ++ [ pkgs.cmake ];
        inherit env;
      };
    };
}
