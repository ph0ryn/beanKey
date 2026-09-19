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
  inherit (lib) mkIf optionalAttrs;
  inherit (settings) configFile;
in
{
  options.programs.beanKey = settings.options // {
    useBeanKeyTheme = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether to apply the beanKey Fcitx5 Classic UI theme.";
    };
  };
  config = mkIf cfg.enable {
    inherit (settings) assertions;
    i18n.inputMethod = {
      enable = true;
      type = "fcitx5";
      fcitx5 = {
        addons = [ packages.fcitx5-addon ];
        settings.addons = optionalAttrs cfg.useBeanKeyTheme {
          classicui.globalSection = {
            Font = lib.mkDefault "Sans 13";
            Theme = lib.mkDefault "beanKey";
            UseAccentColor = lib.mkDefault "False";
          };
        };
      };
    };
    environment.systemPackages = [ packages.daemon ];
    environment.etc."beankey/config.toml".source = configFile;
  };
}
