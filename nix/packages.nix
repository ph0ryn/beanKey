{
  assets,
  pkgs,
}:

let
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;
  llamaSysVersion = pkgs.lib.removePrefix "=" (builtins.fromTOML (
    builtins.readFile ../crates/llama/Cargo.toml
  )).dependencies.llama-cpp-sys-2.version;
  llamaSysLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/utilityai/llama-cpp-rs/0dd5814c9b061930965d144c1653cd4d05054873/LICENSE-MIT";
    hash = "sha256-djkSvQX0KVbb+pAL0aL4A3xwT1xk13jGI+xZUkVOJKo=";
  };

  sourceFor =
    roots:
    pkgs.lib.cleanSourceWith {
      src = ../.;
      filter =
        path: _:
        let
          relative = pkgs.lib.removePrefix "${toString ../.}/" (toString path);
        in
        builtins.any (root: relative == root || pkgs.lib.hasPrefix "${root}/" relative) roots;
    };

  daemon = pkgs.rustPlatform.buildRustPackage {
    pname = "beankey-daemon";
    inherit version;
    src = sourceFor [
      ".cargo"
      "Cargo.lock"
      "Cargo.toml"
      "LICENSE"
      "crates"
      "proto"
    ];
    cargoLock.lockFile = ../Cargo.lock;
    cargoBuildFlags = [
      "--package"
      "beankey-daemon"
    ];
    cargoTestFlags = [
      "--workspace"
      "--all-targets"
    ];
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.pkg-config
      pkgs.protobuf
      pkgs.rustPlatform.bindgenHook
    ];
    buildInputs = [ pkgs.marisa ];
    dontUseCmakeConfigure = true;
    BEANKEY_TEST_DICTIONARY = "${assets.dictionary}/share/beankey/dictionary";
    BEANKEY_TEST_EMOJI_DICTIONARY = "${assets.emoji}/share/beankey/emoji/emoji_all_E17.0.txt";
    BEANKEY_TEST_EN_US_DICTIONARY = "${pkgs.hunspellDicts.en_US}/share/hunspell/en_US";
    BEANKEY_TEST_EL_GR_DICTIONARY = "${pkgs.hunspellDicts.el_GR}/share/hunspell/el_GR";
    BEANKEY_TEST_MODEL = "${assets.model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
    BEANKEY_TEST_ZENZ_TOKENIZER = "${assets.tokenizer}/share/beankey/tokenizer/tokenizer.json";
    postInstall = ''
      install -Dm644 ${../LICENSE} "$out/share/licenses/beankey/LICENSE"
      install -Dm644 ${llamaSysLicense} "$out/share/licenses/beankey/llama-cpp-sys-2-MIT.txt"
      llama_source="$cargoDeps/llama-cpp-sys-2-${llamaSysVersion}/llama.cpp"
      install -Dm644 "$llama_source/LICENSE" "$out/share/licenses/beankey/llama.cpp-MIT.txt"
      cat > "$out/share/licenses/beankey/llama.cpp-NOTICES" <<'EOF'
      Bundled dependency: llama-cpp-sys-2 ${llamaSysVersion}
      Rust source: https://github.com/utilityai/llama-cpp-rs/tree/0dd5814c9b061930965d144c1653cd4d05054873
      llama.cpp source: https://github.com/ggml-org/llama.cpp/tree/d81235049384534c167caea52b85a694f6103d14
      The bundled source is built without beanKey patches.

      ggml/src/ggml-cpu/ops.cpp (YaRN):
      MIT licensed. Copyright (c) 2023 Jeffrey Quesnelle and Bowen Peng.
      Source: https://github.com/jquesnelle/yarn
      The MIT permission and warranty text is included in llama.cpp-MIT.txt.

      src/llama-vocab.cpp (BPE tokenizer):
      adapted from https://github.com/cmp-nct/ggllm.cpp [MIT License]
      EOF
    '';
    passthru = {
      hunspellEnglish = pkgs.hunspellDicts.en_US;
      hunspellGreek = pkgs.hunspellDicts.el_GR;
    };
    meta = {
      description = "beanKey kana-kanji conversion daemon";
      license = pkgs.lib.licenses.mit;
      mainProgram = "beankey-daemon";
      platforms = pkgs.lib.platforms.linux ++ [ "aarch64-darwin" ];
    };
  };

in
{
  inherit daemon;
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
  macos-input-method = pkgs.stdenv.mkDerivation {
    pname = "beankey-macos-input-method";
    inherit version;
    src = sourceFor [
      "Cargo.toml"
      "LICENSE"
      "macos"
      "ipc"
      "proto"
    ];
    cmakeDir = "../macos";
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.ninja
      pkgs.protobuf
    ];
    buildInputs = [ pkgs.protobuf ];
    cmakeFlags = [
      "-DBEANKEY_DAEMON_PATH=${daemon}/bin/beankey-daemon"
    ];
    doCheck = true;
    postInstall = ''
      mkdir -p "$out/share/beankey"
      ln -s ${assets.dictionary}/share/beankey/dictionary "$out/share/beankey/dictionary"
      ln -s ${assets.emoji}/share/beankey/emoji/emoji_all_E17.0.txt "$out/share/beankey/emoji-dictionary"
      ln -s ${assets.model}/share/beankey/model/ggml-model-Q5_K_M.gguf "$out/share/beankey/model"
      ln -s ${daemon.hunspellEnglish}/share/hunspell "$out/share/beankey/hunspell-english"
      ln -s ${daemon.hunspellGreek}/share/hunspell "$out/share/beankey/hunspell-greek"
      substitute ${../macos/install.sh.in} "$out/bin/beankey-install" \
        --subst-var-by BASH ${pkgs.bash} --subst-var-by BUNDLE "$out" \
        --subst-var-by NIX ${pkgs.nix}
      chmod +x "$out/bin/beankey-install"
      install -Dm644 ${../LICENSE} "$out/share/licenses/beankey/LICENSE"
    '';
    meta = {
      description = "beanKey InputMethodKit frontend";
      license = pkgs.lib.licenses.mit;
      platforms = [ "aarch64-darwin" ];
      mainProgram = "beankey-install";
    };
  };
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  fcitx5-addon = pkgs.stdenv.mkDerivation {
    pname = "fcitx5-beankey";
    inherit version;
    src = sourceFor [
      "Cargo.toml"
      "LICENSE"
      "fcitx5"
      "ipc"
      "proto"
    ];
    cmakeDir = "../fcitx5";
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.ninja
      pkgs.pkg-config
      pkgs.protobuf
    ];
    buildInputs = [
      pkgs.fcitx5
      pkgs.protobuf
    ];
    cmakeFlags = [
      "-DBEANKEY_DAEMON_PATH=${daemon}/bin/beankey-daemon"
      "-DBEANKEY_CONFIG_PATH=/etc/beankey/config.toml"
    ];
    doCheck = true;
    postInstall = ''
      install -Dm644 ${../LICENSE} "$out/share/licenses/beankey/LICENSE"
    '';
    meta = {
      description = "Fcitx5 input method addon for beanKey";
      license = pkgs.lib.licenses.mit;
      platforms = pkgs.lib.platforms.linux;
    };
  };
}
