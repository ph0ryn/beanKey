{
  cfg,
  lib,
  packages,
}:
{
  enable = true;
  type = "fcitx5";
  fcitx5 = {
    addons = [ packages.fcitx5-addon ];
    settings.addons = lib.optionalAttrs cfg.useBeanKeyTheme {
      classicui.globalSection = {
        Font = lib.mkDefault "Sans 13";
        Theme = lib.mkDefault "beanKey";
        UseAccentColor = lib.mkDefault "False";
      };
    };
  };
}
