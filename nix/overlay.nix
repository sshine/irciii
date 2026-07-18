{ inputs, ... }:
{
  flake.overlays.default = final: _prev: {
    irciii = inputs.self.packages.${final.stdenv.hostPlatform.system}.default;
  };
}
