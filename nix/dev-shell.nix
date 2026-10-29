{
  model,
  input-ngram,
  pkgs,
}:

pkgs.mkShell {
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
    pkgs.rustPlatform.bindgenHook
  ];
  buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.fcitx5 ];
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
  BEANKEY_TEST_MODEL = "${model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
  BEANKEY_TEST_INPUT_NGRAM = "${input-ngram}/share/beankey/input-ngram";
}
