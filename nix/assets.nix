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
        outputHash = "sha256-5GRFteUIkKTRszdgpicNiwxda5CZFFPkj/x54U18Olg=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/model"
        cp ${source} \
          "$out/share/beankey/model/ggml-model-Q5_K_M.gguf"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';

  input-ngram =
    let
      source = pkgs.fetchurl {
        url = "https://huggingface.co/Miwa-Keita/input_n5_lm_v1/resolve/6153c4693ee202049b6cb834396e658562c147b3/input_n5_lm_v1.zip";
        hash = "sha256-Cq8yYUCpLVd7ICCQU0ZnK4zExH5jUWMordDxl1aKr3o=";
      };
    in
    pkgs.runCommand "beankey-input-n5-lm-v1"
      {
        nativeBuildInputs = [ pkgs.unzip ];
        meta.license = pkgs.lib.licenses.asl20;
        outputHash = "sha256-UphlDGfWDPajGgYyzCsUoS8G6EuLBXgGIAwihpMA8o8=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/input-ngram"
        unzip -j ${source} \
          input_n5_lm_v1/lm_c_abc.marisa \
          input_n5_lm_v1/lm_c_bc.marisa \
          input_n5_lm_v1/lm_r_xbx.marisa \
          input_n5_lm_v1/lm_u_abx.marisa \
          input_n5_lm_v1/lm_u_xbc.marisa \
          -d "$out/share/beankey/input-ngram"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';
}
