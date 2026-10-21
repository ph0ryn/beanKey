{ pkgs }:

{
  model =
    let
      source = pkgs.fetchurl {
        url = "https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf/resolve/c67e03e07d215c869f591b274c1631170d3e11fe/ggml-model-Q5_K_M.gguf";
        hash = "sha256-KcIj1MIzJ7gP0T67WrJVUFekYxeZfV2jkVhP++8NtnM=";
      };
    in
    pkgs.runCommand "beankey-zenz-v3.2-small-gguf"
      {
        meta.license = pkgs.lib.licenses.asl20;
        outputHash = "sha256-i+n59zvclsiKaN6dcVILHwnFCHiXONuXO1RQ6tisiBg=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/model"
        cp ${source} \
          "$out/share/beankey/model/ggml-model-Q5_K_M.gguf"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';
}
