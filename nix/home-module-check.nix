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
    conversion = {
      typeHalfSpace = true;
      punctuationStyle = "period_and_comma";
    };
    zenz.personalization = {
      baseNgram = "/beankey-test/base-ngram";
      personalNgram = "/beankey-test/personal-ngram";
      alpha = 1.5;
    };
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
pkgs.runCommand "beankey-home-module"
  {
    nativeBuildInputs = [ pkgs.python3 ];
  }
  ''
    python - ${configFile} <<'PY'
    import sys
    import tomllib

    with open(sys.argv[1], "rb") as source:
        config = tomllib.load(source)
    assert config["conversion"]["type_half_space"] is True
    assert config["conversion"]["punctuation_style"] == "period_and_comma"
    assert config["zenz"]["personalization"] == {
        "base_ngram": "/beankey-test/base-ngram",
        "personal_ngram": "/beankey-test/personal-ngram",
        "alpha": 1.5,
    }
    PY
    ${pkgs.lib.optionalString isLinux ''
      grep -Fx 'Theme=beanKey' ${config.xdg.configFile.fcitx5.source}/conf/classicui.conf
    ''}
    ln -s ${configFile} "$out"
  ''
