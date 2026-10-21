{ pkgs }:

let
  dictionarySource = pkgs.fetchFromGitHub {
    owner = "azooKey";
    repo = "azooKey_dictionary_storage";
    rev = "4d418525b090cf49c219819d05a7e3cc2a4346eb";
    hash = "sha256-CsJ9kODrCOtMnMd/5I4A52enoSOyJlioC1i6azOd1Pk=";
  };
  emojiSource = pkgs.fetchFromGitHub {
    owner = "azooKey";
    repo = "azooKey_emoji_dictionary_storage";
    rev = "67b822603391b01238d7b80b8b61b63f966cf357";
    hash = "sha256-5Ebm94/W/21lrjMprdwbd6cfaavTaXZGH3WxV9OLt5M=";
  };
  beanKeyUpstream = pkgs.fetchFromGitHub {
    owner = "azooKey";
    repo = "AzooKeyKanaKanjiConverter";
    rev = "93766c46e31fa6a18b7ced49dab31337780f6f45";
    hash = "sha256-euGFHvSc0MfXpXg+0GdwgLpCbV9NmM8B/e/wHsKrXDE=";
  };
in
{
  dictionary =
    pkgs.runCommand "beankey-dictionary"
      {
        meta.license = pkgs.lib.licenses.asl20;
        outputHash = "sha256-KzQM9fCOVuXVNO/Gka9lWBgoQgoCgz+xb2f4EcqAiOc=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/dictionary"
        cp -r ${dictionarySource}/Dictionary/. \
          "$out/share/beankey/dictionary/"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';

  emoji =
    pkgs.runCommand "beankey-emoji-dictionary"
      {
        meta.license = with pkgs.lib.licenses; [
          bsd3
          unicode-30
          mit
        ];
        outputHash = "sha256-CiWASDsiYwfpkQ69n67nnhpvcym14Zy7VnDQbUdRYGY=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/emoji"
        cp ${emojiSource}/EmojiDictionary/emoji_all_E17.0.txt \
          "$out/share/beankey/emoji/emoji_all_E17.0.txt"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';

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
        outputHash = "sha256-7UFG6XARP6tGKq7x8vcS/+qWNSzApNBSMeH5nVRsZhI=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/model"
        cp ${source} \
          "$out/share/beankey/model/ggml-model-Q5_K_M.gguf"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';

  tokenizer =
    pkgs.runCommand "beankey-zenz-tokenizer"
      {
        meta.license = pkgs.lib.licenses.cc-by-sa-40;
        outputHash = "sha256-pHYoszHk+D3CHZY6U3MNSFhKvIqQ3px9nfx97yiv9rU=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/tokenizer"
        cp -r ${beanKeyUpstream}/Sources/EfficientNGram/tokenizer/. \
          "$out/share/beankey/tokenizer/"
        install -Dm644 ${../THIRD_PARTY_NOTICES.txt} "$out/share/licenses/beankey/THIRD_PARTY_NOTICES.txt"
      '';
}
