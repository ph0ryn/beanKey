{
  model,
  pkgs,
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
    pkgs.rustPlatform.bindgenHook
  ];
in
{
  inherit packages;

  shell = pkgs.mkShell {
    inherit packages;
    buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.fcitx5 ];
    RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
    BEANKEY_TEST_MODEL = "${model}/share/beankey/model/ggml-model-Q5_K_M.gguf";
  };
}
