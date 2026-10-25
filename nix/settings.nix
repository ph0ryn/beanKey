{
  cfg,
  lib,
  pkgs,
}:
let
  toml = pkgs.formats.toml { };
in
{
  configFile = toml.generate "beankey-config.toml" cfg.settings;
  options = {
    enable = lib.mkEnableOption "beanKey kana-kanji conversion";

    useBeanKeyTheme = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether to apply the beanKey Fcitx5 Classic UI theme on Linux. Ignored on macOS.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = {
        conversion.type_backslash = true;
        zenz.inference_limit = 4;
      };
      description = "Daemon settings using the same keys and structure as config.toml. Omitted settings use daemon defaults; the daemon validates their values.";
    };
  };
}
