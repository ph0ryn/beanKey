{
  dictionary,
  emoji,
  model,
  pkgs,
  tokenizer,
}:

let
  packages = [
    pkgs.cargo
    pkgs.clang
    pkgs.clang-tools
    pkgs.clippy
    pkgs.cmake
    pkgs.git
    pkgs.ninja
    pkgs.pkg-config
    pkgs.protobuf
    pkgs.rust-analyzer
    pkgs.rustc
    pkgs.rustfmt
  ];
in
{
  inherit packages;

  shell = pkgs.mkShell {
    inherit packages;
    buildInputs = [
      pkgs.hunspell
      pkgs.llama-cpp
      pkgs.marisa
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.fcitx5 ];
    RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
    BEANKEY_TEST_DICTIONARY = "${dictionary}/share/beankey/dictionary";
    BEANKEY_TEST_EMOJI_DICTIONARY = "${emoji}/share/beankey/emoji/emoji_all_E17.0.txt";
    BEANKEY_TEST_EN_US_DICTIONARY = "${pkgs.hunspellDicts.en_US}/share/hunspell/en_US";
    BEANKEY_TEST_EL_GR_DICTIONARY = "${pkgs.hunspellDicts.el_GR}/share/hunspell/el_GR";
    BEANKEY_TEST_MODEL = "${model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
    BEANKEY_TEST_LLAMA_BACKEND = "${pkgs.llama-cpp}/bin";
    BEANKEY_TEST_ZENZ_TOKENIZER = "${tokenizer}/share/beankey/tokenizer/tokenizer.json";
  };
}
