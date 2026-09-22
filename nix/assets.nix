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
        outputHash = "sha256-9vAeYy1EAjxYtQ3MryaNwk0DAIAioWa6eOWok9QThUs=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/dictionary"
        mkdir -p "$out/share/licenses/beankey-dictionary"
        cp -r ${dictionarySource}/Dictionary/. \
          "$out/share/beankey/dictionary/"
        cp ${dictionarySource}/LICENSE \
          "$out/share/licenses/beankey-dictionary/LICENSE"
        cat > "$out/share/licenses/beankey-dictionary/ATTRIBUTION" <<'EOF'
        Asset: azooKey dictionary storage
        Source: azooKey/azooKey_dictionary_storage
        Fixed revision: ${dictionarySource.rev}
        License: Apache License 2.0
        Copyright 2024 Miwa / ensan
        Changes by beanKey: none; generated dictionary files are copied directly.
        Upstream NOTICE: none at the fixed revision.
        EOF
      '';

  emoji =
    let
      mozcLicense = pkgs.fetchurl {
        url = "https://raw.githubusercontent.com/google/mozc/4517e51d53063397222adb5512c7ad972b17c181/LICENSE";
        hash = "sha256-RM3ZI7keqRmSk6vswnYscMh9vx5YHAJ6lMQWNo0aZIw=";
      };
      unicodeLicense = pkgs.fetchurl {
        url = "https://www.unicode.org/license.txt";
        hash = "sha256-56k7AJVlz85VkZo4FDesTbiD6dohJvoouR0ScyvFPZY=";
      };
    in
    pkgs.runCommand "beankey-emoji-dictionary"
      {
        meta.license = with pkgs.lib.licenses; [
          bsd3
          unicode-30
          mit
        ];
        outputHash = "sha256-m2je4y4idA8nLeb9MjUtw6zwskJrliy62bng0q5Dp0A=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/emoji"
        mkdir -p "$out/share/licenses/beankey-emoji"
        cp ${emojiSource}/EmojiDictionary/emoji_all_E17.0.txt \
          "$out/share/beankey/emoji/emoji_all_E17.0.txt"
        cp ${emojiSource}/data/README.md \
          "$out/share/licenses/beankey-emoji/UPSTREAM-DATA.md"
        cp ${mozcLicense} "$out/share/licenses/beankey-emoji/BSD-3-Clause.txt"
        cp ${unicodeLicense} "$out/share/licenses/beankey-emoji/Unicode-License-V3.txt"
        cp ${beanKeyUpstream}/LICENSE \
          "$out/share/licenses/beankey-emoji/MIT.txt"
        cat > "$out/share/licenses/beankey-emoji/ATTRIBUTION" <<'EOF'
        Asset: generated azooKey emoji dictionary for Unicode Emoji 17.0
        Source: azooKey/azooKey_emoji_dictionary_storage
        Fixed revision: ${emojiSource.rev}
        Packaged file: EmojiDictionary/emoji_all_E17.0.txt
        Changes by beanKey: none; the generated dictionary is copied directly.

        Mozc-derived data:
        Source revision: google/mozc 4517e51d53063397222adb5512c7ad972b17c181
        Copyright 2010-2018, Google Inc. All rights reserved.
        License: BSD 3-Clause

        Unicode Emoji and CLDR-derived data:
        Copyright 2022 Unicode, Inc. for emoji data.
        Copyright 1991-2023 Unicode, Inc. for CLDR data.
        License: Unicode License V3

        azooKey additional emoji data:
        Copyright 2023 Miwa / Ensan
        License: MIT
        EOF
      '';

  model =
    let
      source = pkgs.fetchurl {
        url = "https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf/resolve/c67e03e07d215c869f591b274c1631170d3e11fe/ggml-model-Q5_K_M.gguf";
        hash = "sha256-KcIj1MIzJ7gP0T67WrJVUFekYxeZfV2jkVhP++8NtnM=";
      };
      license = pkgs.fetchurl {
        url = "https://www.apache.org/licenses/LICENSE-2.0.txt";
        hash = "sha256-z8d0m5b2O9McPEK1xHG/dWgUBT6EfBDz6wA0F7xSPTA=";
      };
    in
    pkgs.runCommand "beankey-zenz-v3.2-small-gguf"
      {
        meta.license = pkgs.lib.licenses.asl20;
        outputHash = "sha256-dOmsJLYpxy6RGZVc0Rfirz8He/CoLXG5J+F61lBSjSM=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/model"
        mkdir -p "$out/share/licenses/beankey-model"
        cp ${source} \
          "$out/share/beankey/model/ggml-model-Q5_K_M.gguf"
        cp ${license} \
          "$out/share/licenses/beankey-model/Apache-2.0.txt"
        cat > "$out/share/licenses/beankey-model/ATTRIBUTION" <<'EOF'
        Asset: zenz-v3.2-small GGUF model
        Source: Miwa-Keita/zenz-v3.2-small-gguf
        Fixed revision: c67e03e07d215c869f591b274c1631170d3e11fe
        File: ggml-model-Q5_K_M.gguf
        Source hash: sha256-KcIj1MIzJ7gP0T67WrJVUFekYxeZfV2jkVhP++8NtnM=
        License: Apache License 2.0, as declared by the fixed model card metadata.
        Changes by beanKey: none; the fetched bytes are copied directly.
        Upstream LICENSE and NOTICE: none at the fixed revision.
        EOF
      '';

  tokenizer =
    let
      license = pkgs.fetchurl {
        url = "https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt";
        hash = "sha256-KKlSnH0LtNxR9L9cEWo9Fu8kegUvdZFGZ2jd9WP9HPU=";
      };
    in
    pkgs.runCommand "beankey-zenz-tokenizer"
      {
        meta.license = pkgs.lib.licenses.cc-by-sa-40;
        outputHash = "sha256-6xZIBXHZzGL3waV7tJf6t8n540XRWCvRQZbNwDFLVQ8=";
        outputHashAlgo = "sha256";
        outputHashMode = "recursive";
      }
      ''
        mkdir -p "$out/share/beankey/tokenizer"
        mkdir -p "$out/share/licenses/beankey-tokenizer"
        cp -r ${beanKeyUpstream}/Sources/EfficientNGram/tokenizer/. \
          "$out/share/beankey/tokenizer/"
        cp ${license} "$out/share/licenses/beankey-tokenizer/CC-BY-SA-4.0.txt"
        cat > "$out/share/licenses/beankey-tokenizer/ATTRIBUTION" <<'EOF'
        Asset: EfficientNGram tokenizer data
        Source: azooKey/AzooKeyKanaKanjiConverter
        Fixed revision: 93766c46e31fa6a18b7ced49dab31337780f6f45
        Original distribution: ku-nlp/gpt2-small-japanese-char
        License: Creative Commons Attribution-ShareAlike 4.0 International
        Changes by beanKey: none; files are copied from the fixed azooKey revision.
        EOF
      '';
}
