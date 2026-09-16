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
    BEAN_KEY_TEST_DICTIONARY = "${dictionary}/share/bean-key/dictionary";
    BEAN_KEY_TEST_EMOJI_DICTIONARY = "${emoji}/share/bean-key/emoji/emoji_all_E17.0.txt";
    BEAN_KEY_TEST_EN_US_DICTIONARY = "${pkgs.hunspellDicts.en_US}/share/hunspell/en_US";
    BEAN_KEY_TEST_EL_GR_DICTIONARY = "${pkgs.hunspellDicts.el_GR}/share/hunspell/el_GR";
    BEAN_KEY_TEST_MODEL = "${model}/share/bean-key/model/ggml-model-Q5_K_M.gguf";
    BEAN_KEY_TEST_LLAMA_BACKEND = "${pkgs.llama-cpp}/bin";
    BEAN_KEY_TEST_ZENZ_TOKENIZER = "${tokenizer}/share/bean-key/tokenizer/tokenizer.json";
  };
}
