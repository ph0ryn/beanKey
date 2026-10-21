{
  developmentPackages,
  home-manager,
  nixpkgs,
  pkgs,
  self,
  system,
}:

let
  assetNames = [
    "dictionary"
    "emoji"
    "model"
    "tokenizer"
  ];
  assetSystems = builtins.attrNames self.packages;
  referenceSystem = builtins.head assetSystems;
  sharedAssetPaths = builtins.all (
    assetName:
    builtins.all (
      assetSystem:
      self.packages.${assetSystem}.${assetName}.outPath
      == self.packages.${referenceSystem}.${assetName}.outPath
    ) assetSystems
  ) assetNames;
  moduleEvaluation = nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [
      self.nixosModules.default
      {
        programs.beanKey = {
          enable = true;
          useBeanKeyTheme = true;
          conversion = {
            typeBackslash = true;
            typeHalfSpace = true;
            optionDirectFullWidthInput = true;
            punctuationStyle = "period_and_comma";
          };
          zenz.personalization = {
            baseNgram = "/nix/store/beankey-base-ngram/lm";
            personalNgram = "/nix/store/beankey-personal-ngram/lm";
            alpha = 1.5;
          };
        };
        system.stateVersion = "26.05";
      }
    ];
  };
  moduleConfig = moduleEvaluation.config;
  moduleWithoutThemeEvaluation = nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [
      self.nixosModules.default
      {
        programs.beanKey.enable = true;
        system.stateVersion = "26.05";
      }
    ];
  };
  moduleWithoutThemeConfig = moduleWithoutThemeEvaluation.config;
  moduleConfigSource =
    assert !(moduleWithoutThemeConfig.environment.etc ? "xdg/fcitx5/conf/classicui.conf");
    assert builtins.elem self.packages.${system}.fcitx5-addon
      moduleConfig.i18n.inputMethod.fcitx5.addons;
    assert builtins.elem self.packages.${system}.daemon moduleConfig.environment.systemPackages;
    assert !(moduleConfig.systemd.services ? beanKey);
    assert !(moduleConfig.systemd.sockets ? beanKey);
    moduleConfig.environment.etc."beankey/config.toml".source;
  classicUIConfigSource = moduleConfig.environment.etc."xdg/fcitx5/conf/classicui.conf".source;
in
{
  home-module = import ./home-module-check.nix {
    inherit
      home-manager
      pkgs
      self
      system
      ;
  };

  shared-asset-paths =
    assert pkgs.lib.assertMsg sharedAssetPaths
      "beanKey assets must have the same store paths on all supported systems";
    pkgs.runCommand "beankey-shared-asset-paths" { } ''
      touch "$out"
    '';

  default-package =
    let
      packages = self.packages.${system};
      components = builtins.removeAttrs packages [ "default" ];
    in
    pkgs.runCommand "beankey-default-package" { } ''
      ${pkgs.lib.concatStringsSep "\n" (
        pkgs.lib.mapAttrsToList (name: path: ''
          test "$(readlink '${packages.default}/${name}')" = '${path}'
        '') components
      )}
      touch "$out"
    '';

  third-party-notices =
    let
      components = builtins.removeAttrs self.packages.${system} [ "default" ];
      noticeFor =
        name:
        if name == "fcitx5-addon" then
          {
            source = ../fcitx5/THIRD_PARTY_NOTICES.txt;
            directory = "beankey/fcitx5";
          }
        else if name == "macos-input-method" then
          {
            source = ../macos/THIRD_PARTY_NOTICES.txt;
            directory = "beankey/macos";
          }
        else
          {
            source = ../THIRD_PARTY_NOTICES.txt;
            directory = "beankey";
          };
    in
    pkgs.runCommand "beankey-third-party-notices" { } ''
      ${pkgs.lib.concatStringsSep "\n" (
        pkgs.lib.mapAttrsToList (
          name: package:
          let
            notice = noticeFor name;
            directory = "${package}/share/licenses/${notice.directory}";
          in
          ''
            cmp ${notice.source} '${directory}/THIRD_PARTY_NOTICES.txt'
            ${pkgs.lib.optionalString (!(builtins.elem name assetNames)) ''
              cmp ${../LICENSE} '${directory}/LICENSE'
            ''}
          ''
        ) components
      )}
      ${pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
        resources='${
          self.packages.${system}.macos-input-method
        }/Library/Input Methods/beanKey.app/Contents/Resources'
        cmp ${../macos/THIRD_PARTY_NOTICES.txt} "$resources/THIRD_PARTY_NOTICES.txt"
        cmp ${../LICENSE} "$resources/LICENSE"
      ''}
      touch "$out"
    '';

  daemon-bundled-llama =
    let
      daemon = self.packages.${system}.daemon;
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

  macos-installer-upgrade =
    pkgs.runCommand "beankey-macos-installer-upgrade"
      {
        nativeBuildInputs = [
          pkgs.bash
          pkgs.gnugrep
        ];
      }
      ''
        for kind in standalone home-manager; do
          home="$TMPDIR/$kind"
          support="$home/Library/Application Support/beanKey"
          mkdir -p "$support"
          cat > "$TMPDIR/$kind-config.toml" <<EOF
        # Generated by beanKey standalone installer
        dictionary = "/existing/dictionary"
        llama_backend_directory = "$support/package/share/beankey/llama-backend"
        [learning]
        mode = "nothing"
        EOF
          if test "$kind" = home-manager; then
            ln -s "$TMPDIR/$kind-config.toml" "$support/config.toml"
          else
            cp "$TMPDIR/$kind-config.toml" "$support/config.toml"
          fi
          if HOME="$home" bash '${../macos/install.sh.in}' >stdout 2>stderr; then
            echo 'installer accepted an obsolete backend setting' >&2
            exit 1
          fi
          grep -qF 'llama_backend_directory' stderr
          grep -qF 'Home Manager' stderr
          grep -qF 'config.toml' stderr
          cmp "$TMPDIR/$kind-config.toml" "$support/config.toml"
          test ! -e "$support/package"
          test ! -e "$home/Library/Input Methods"
        done
        touch "$out"
      '';

  cargo-metadata =
    pkgs.runCommand "beankey-cargo-metadata"
      {
        nativeBuildInputs = [ pkgs.cargo ];
      }
      ''
        cp ${../Cargo.toml} Cargo.toml
        cp -R ${../crates} crates
        cargo metadata \
          --format-version 1 \
          --no-deps \
          --manifest-path Cargo.toml \
          >/dev/null
        touch "$out"
      '';

  development-tools =
    pkgs.runCommand "beankey-development-tools"
      {
        nativeBuildInputs = developmentPackages;
      }
      ''
        cargo --version
        clang --version
        clang-format --version
        printf '%s\n' \
          '#include <string_view>' \
          'static_assert(std::string_view{"beanKey"}.size() == 7);' \
          | clang++ -std=c++20 -x c++ -fsyntax-only -
        cmake --version
        ninja --version
        pkg-config --version
        protoc --version
        rustc --version
        touch "$out"
      '';
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
  macos-config-separation =
    let
      inputMethod = self.packages.${system}.macos-input-method;
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
          test -L '${inputMethod}/share/beankey/'"$asset"
          test -e '${inputMethod}/share/beankey/'"$asset"
        done
        bash -n '${inputMethod}/bin/beankey-install'
        grep -aqF 'Library/Application Support/beanKey' '${binary}'
        grep -aqF 'config.toml' '${binary}'
        grep -qF 'config.toml' '${inputMethod}/bin/beankey-install'
        grep -qF 'package_root/share/beankey/model' '${inputMethod}/bin/beankey-install'
        if grep -aqE '/nix/store/[a-z0-9]{32}-beankey-config.toml' '${binary}'; then
          echo 'macOS frontend embeds a generated configuration store path' >&2
          exit 1
        fi
        touch "$out"
      '';
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  nixos-module = pkgs.runCommand "beankey-nixos-module" { } ''
    config=${moduleConfigSource}
    grep -F 'dictionary = "${self.packages.${system}.dictionary}/share/beankey/dictionary"' "$config"
    grep -F 'model = "${self.packages.${system}.model}/share/beankey/model/ggml-model-Q5_K_M.gguf"' "$config"
    if grep -qF 'llama_backend_directory' "$config"; then
      echo 'generated config retains the removed backend setting' >&2
      exit 1
    fi
    grep -F 'runtime_socket = "beankey/daemon.sock"' "$config"
    grep -F 'keyboard_language = "japanese"' "$config"
    grep -F 'japanese_prediction = "disabled"' "$config"
    grep -F 'foreign_prediction = "disabled"' "$config"
    grep -F 'full_width_roman = true' "$config"
    grep -F 'half_width_kana = false' "$config"
    grep -F 'live_conversion = true' "$config"
    grep -F 'type_backslash = true' "$config"
    grep -F 'type_half_space = true' "$config"
    grep -F 'option_direct_full_width_input = true' "$config"
    grep -F 'punctuation_style = "period_and_comma"' "$config"
    grep -F 'mode = "input_and_output"' "$config"
    grep -F 'max_count = 65536' "$config"
    grep -F 'inference_limit = 5' "$config"
    grep -F 'enable_alignment_separator = true' "$config"
    grep -F 'base_ngram = "/nix/store/beankey-base-ngram/lm"' "$config"
    grep -F 'personal_ngram = "/nix/store/beankey-personal-ngram/lm"' "$config"
    grep -F 'alpha = 1.5' "$config"
    classic_ui_config=${classicUIConfigSource}
    grep -F 'Font=Sans 13' "$classic_ui_config"
    grep -F 'Theme=beanKey' "$classic_ui_config"
    grep -F 'UseAccentColor=False' "$classic_ui_config"
    test -f ${self.packages.${system}.fcitx5-addon}/share/fcitx5/themes/beanKey/theme.conf
    touch "$out"
  '';
}
