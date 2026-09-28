{
  home-manager,
  pkgs,
  self,
  system,
}:
let
  inherit (pkgs.stdenv.hostPlatform) isLinux;
  packages = self.packages.${system};
  evaluate =
    beanKey:
    home-manager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        self.homeModules.default
        {
          home = {
            username = "beankey-test";
            homeDirectory = if isLinux then "/home/beankey-test" else "/Users/beankey-test";
            stateVersion = "25.11";
          };
          programs.beanKey = beanKey;
        }
      ];
    };
  enabled = evaluate {
    enable = true;
    useBeanKeyTheme = true;
    conversion.typeHalfSpace = true;
  };
  disabled = evaluate { enable = false; };
  config = enabled.config;
  configFile =
    if isLinux then
      config.xdg.configFile."beankey/config.toml".source
    else
      config.home.file."Library/Application Support/beanKey/config.toml".source;
in
assert builtins.all (value: value.assertion) config.assertions;
assert builtins.all (value: value.assertion) disabled.config.assertions;
assert !(disabled.config.xdg.configFile ? "beankey/config.toml");
assert !(disabled.config.home.file ? "Library/Application Support/beanKey/config.toml");
assert
  !isLinux
  || (
    config.i18n.inputMethod.enable
    && config.i18n.inputMethod.type == "fcitx5"
    && builtins.elem packages.fcitx5-addon config.i18n.inputMethod.fcitx5.addons
    && !(config.home.activation ? beanKeyInputMethod)
    && !(disabled.config.systemd.user.services ? fcitx5-daemon)
  );
assert
  isLinux
  || (
    builtins.elem packages.macos-input-method config.home.packages
    && config.home.activation ? beanKeyInputMethod
    && !config.i18n.inputMethod.enable
    && !(disabled.config.home.activation ? beanKeyInputMethod)
  );
pkgs.runCommand "beankey-home-module" { } (
  ''
    grep -F 'dictionary = "${packages.dictionary}/share/beankey/dictionary"' ${configFile}
    grep -F 'model = "${packages.model}/share/beankey/model/ggml-model-Q5_K_M.gguf"' ${configFile}
    grep -F 'type_half_space = true' ${configFile}
  ''
  + pkgs.lib.optionalString isLinux ''
    grep -F 'Theme=beanKey' ${config.xdg.configFile.fcitx5.source}/conf/classicui.conf
  ''
  + ''
    touch "$out"
  ''
)
