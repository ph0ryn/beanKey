{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.beanKey;
  packages = self.packages.${pkgs.stdenv.hostPlatform.system};
  settings = import ./settings.nix {
    inherit
      cfg
      lib
      pkgs
      packages
      ;
  };
in
{
  options.programs.beanKey = settings.options;
  config = lib.mkIf cfg.enable {
    inherit (settings) assertions;
    i18n.inputMethod = import ./fcitx5.nix {
      inherit cfg lib packages;
    };
    environment.systemPackages = [ packages.daemon ];
    environment.etc."beankey/config.toml".source = settings.configFile;
  };
}
