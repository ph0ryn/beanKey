{
  home-manager,
  nixpkgs,
  pkgs,
  self,
  system,
}:

let
  packages = self.packages.${system};
  modelPath = packages.model.outPath;
  sharedModelPath = builtins.all (other: self.packages.${other}.model.outPath == modelPath) (
    builtins.attrNames self.packages
  );
  moduleConfig =
    beanKey:
    (nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        self.nixosModules.default
        {
          programs.beanKey = beanKey;
          system.stateVersion = "26.05";
        }
      ];
    }).config;
  enabled = moduleConfig {
    enable = true;
    useBeanKeyTheme = true;
  };
  withoutTheme = moduleConfig {
    enable = true;
    settings = {
      conversion.live_conversion = false;
      learning.max_count = 65536;
    };
  };
in
assert pkgs.lib.assertMsg sharedModelPath
  "beanKey model must have the same store path on all supported systems";
{
  daemon = packages.daemon;
  home-module = import ./home-module-check.nix {
    inherit
      home-manager
      pkgs
      self
      system
      ;
  };

  daemon-bundled-llama =
    let
      daemon = packages.daemon;
      closure = pkgs.closureInfo { rootPaths = [ daemon ]; };
    in
    pkgs.runCommand "beankey-daemon-bundled-llama"
      {
        nativeBuildInputs = [
          pkgs.gnugrep
        ]
        ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.binutils ]
        ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.darwin.cctools ];
      }
      ''
        ${
          if pkgs.stdenv.hostPlatform.isDarwin then
            "otool -L '${daemon}/bin/beankey-daemon'"
          else
            "readelf --dynamic '${daemon}/bin/beankey-daemon'"
        } > linked-libraries
        if grep -Ei 'lib(llama|ggml)[^[:space:]]*\.(so|dylib)' linked-libraries; then
          echo 'daemon links an external llama/ggml shared library' >&2
          exit 1
        fi
        if grep -E '/[0-9a-z]{32}-(llama-cpp|ggml)(-[0-9]|/|$)' '${closure}/store-paths'; then
          echo 'daemon closure retains an external llama/ggml package' >&2
          exit 1
        fi
        test ! -e '${daemon}/share/beankey/llama-backend'
        touch "$out"
      '';
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
  macos-input-method = packages.macos-input-method;
  macos-config-separation =
    let
      inputMethod = packages.macos-input-method;
      binary = "${inputMethod}/Library/Input Methods/beanKey.app/Contents/MacOS/beanKey";
    in
    pkgs.runCommand "beankey-macos-config-separation"
      {
        nativeBuildInputs = [
          pkgs.bash
          pkgs.gnugrep
        ];
      }
      ''
        test ! -e '${inputMethod}/share/beankey/config.toml'
        test ! -e '${inputMethod}/share/beankey/llama-backend'
        for asset in dictionary emoji-dictionary model hunspell-english hunspell-greek; do
          test -L '${packages.daemon}/share/beankey/'"$asset"
          test -e '${packages.daemon}/share/beankey/'"$asset"
        done
        bash -n '${inputMethod}/bin/beankey-install'
        if grep -aqE '/nix/store/[a-z0-9]{32}-beankey-config.toml' '${binary}'; then
          echo 'macOS frontend embeds a generated configuration store path' >&2
          exit 1
        fi
        touch "$out"
      '';
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  fcitx5-addon = packages.fcitx5-addon;
  nixos-module =
    assert !(withoutTheme.environment.etc ? "xdg/fcitx5/conf/classicui.conf");
    assert builtins.elem packages.fcitx5-addon enabled.i18n.inputMethod.fcitx5.addons;
    assert builtins.elem packages.daemon enabled.environment.systemPackages;
    assert !(enabled.systemd.services ? beanKey);
    assert !(enabled.systemd.sockets ? beanKey);
    pkgs.runCommand "beankey-nixos-module" { nativeBuildInputs = [ pkgs.python3 ]; } ''
      classic_ui_config=${enabled.environment.etc."xdg/fcitx5/conf/classicui.conf".source}
      grep -Fx 'Theme=beanKey' "$classic_ui_config"
      test -f ${packages.fcitx5-addon}/share/fcitx5/themes/beanKey/theme.conf
      python - ${enabled.environment.etc."beankey/config.toml".source} ${
        withoutTheme.environment.etc."beankey/config.toml".source
      } <<'PY'
      import sys
      import tomllib

      with open(sys.argv[1], "rb") as source:
          assert tomllib.load(source) == {}
      with open(sys.argv[2], "rb") as source:
          assert tomllib.load(source) == {
              "conversion": {"live_conversion": False},
              "learning": {"max_count": 65536},
          }
      PY
      ln -s ${enabled.environment.etc."beankey/config.toml".source} "$out"
    '';
}
