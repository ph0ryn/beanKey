{
  assets,
  pkgs,
}:

let
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;

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
      pkgs.pkg-config
      pkgs.protobuf
    ];
    buildInputs = [
      pkgs.hunspell
      pkgs.llama-cpp
      pkgs.marisa
    ];
    BEANKEY_TEST_DICTIONARY = "${assets.dictionary}/share/beankey/dictionary";
    BEANKEY_TEST_EMOJI_DICTIONARY = "${assets.emoji}/share/beankey/emoji/emoji_all_E17.0.txt";
    BEANKEY_TEST_EN_US_DICTIONARY = "${pkgs.hunspellDicts.en_US}/share/hunspell/en_US";
    BEANKEY_TEST_EL_GR_DICTIONARY = "${pkgs.hunspellDicts.el_GR}/share/hunspell/el_GR";
    BEANKEY_TEST_MODEL = "${assets.model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
    BEANKEY_TEST_LLAMA_BACKEND = "${pkgs.llama-cpp}/bin";
    BEANKEY_TEST_ZENZ_TOKENIZER = "${assets.tokenizer}/share/beankey/tokenizer/tokenizer.json";
    postInstall = ''
      install -Dm644 ${../LICENSE} "$out/share/licenses/beankey/LICENSE"
    '';
    passthru = {
      llamaCpp = pkgs.llama-cpp;
      hunspellEnglish = pkgs.hunspellDicts.en_US;
      hunspellGreek = pkgs.hunspellDicts.el_GR;
    };
    meta = {
      description = "beanKey kana-kanji conversion daemon";
      license = pkgs.lib.licenses.mit;
      mainProgram = "beankey-daemon";
      platforms = pkgs.lib.platforms.linux;
    };
  };
in
{
  inherit daemon;

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
