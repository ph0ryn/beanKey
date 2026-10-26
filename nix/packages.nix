{ assets, pkgs }:

let
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;
  product = pkgs.rustPlatform.buildRustPackage {
    pname = "beankey";
    inherit version;
    outputs = [
      "out"
      "daemon"
    ];
    src = pkgs.lib.cleanSourceWith {
      src = ../.;
      filter =
        path: _:
        let
          relative = pkgs.lib.removePrefix "${toString ../.}/" (toString path);
        in
        builtins.any (root: relative == root || pkgs.lib.hasPrefix "${root}/" relative) [
          ".cargo"
          "CMakeLists.txt"
          "Cargo.lock"
          "Cargo.toml"
          "LICENSE"
          "THIRD_PARTY_NOTICES.txt"
          "crates"
          "fcitx5"
          "macos"
          "ipc"
        ];
    };
    cargoLock = {
      lockFile = ../Cargo.lock;
      allowBuiltinFetchGit = true;
    };
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.ninja
      pkgs.pkg-config
      pkgs.protobuf
      pkgs.rustPlatform.bindgenHook
    ];
    buildInputs = [
      pkgs.protobuf
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.fcitx5 ];
    dontUseCmakeConfigure = true;
    configurePhase = ''
      runHook preConfigure
      cmake -S . -B build -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$out" \
        -DCMAKE_INSTALL_LIBDIR=lib \
        -DBEANKEY_MODEL=${assets.model}/share/beankey/model/ggml-model-Q5_K_M.gguf \
        ${
          if pkgs.stdenv.hostPlatform.isDarwin then
            "-DBEANKEY_NIX_STORE=${pkgs.nix}/bin/nix-store"
          else
            ''-DBEANKEY_DAEMON_PATH="$daemon/bin/beankey-daemon"''
        }
      runHook postConfigure
    '';
    buildPhase = ''
      runHook preBuild
      cmake --build build --parallel "$NIX_BUILD_CORES"
      runHook postBuild
    '';
    checkPhase = ''
      runHook preCheck
      ctest --test-dir build --output-on-failure
      runHook postCheck
    '';
    installPhase = ''
      runHook preInstall
      cmake --install build --prefix "$daemon" --component Daemon
      cmake --install build --prefix "$out" --component Frontend
      runHook postInstall
    '';
    meta = {
      description = "beanKey kana-kanji input method";
      license = pkgs.lib.licenses.mit;
      platforms = pkgs.lib.platforms.linux ++ [ "aarch64-darwin" ];
    };
  };
in
{
  daemon = product.daemon // {
    meta = product.meta // {
      description = "beanKey kana-kanji conversion daemon";
      mainProgram = "beankey-daemon";
    };
  };
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
  macos-input-method = product // {
    meta = product.meta // {
      mainProgram = "beankey-install";
    };
  };
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  fcitx5-addon = product;
}
