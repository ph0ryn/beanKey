# beanKey for fcitx5 on NixOS

beanKeyは、[azooKey Desktop](https://github.com/azooKey/azooKey-Desktop)の日本語入力体験を、NixOS上のFcitx5で使うための日本語入力エンジンです。

ニューラルかな漢字変換システムには、[zenz-v3.2-small-gguf](https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf)を使用します。

> [!note]
> 現在は実験的な段階です。設定や内部データ形式は、今後予告なく変わる可能性があります。

## 主な機能

- ローマ字、AZIK、かな入力、カスタム入力テーブル
- 通常変換、ライブ変換、文節単位の確定、再変換
- Zenzaiによる文脈を考慮した候補評価
- 日本語、英語、ギリシャ語の入力中予測
- 学習、ユーザー辞書、入力訂正
- Fcitx5標準UIによるプリエディットと候補表示
- NixOS moduleによる宣言的な導入と設定

azooKey DesktopのAI変換、UI、独自候補ウィンドウは実装しません。

### azooKey Desktopとの違い

beanKeyは、AzooKeyKanaKanjiConverterのかな漢字変換機能と、azooKey Desktopの日本語入力操作を参照しています。ただし、azooKey Desktopそのものを移植するものではありません。

次の機能は対象外です。

- Control+SによるAI置換候補や続きの提案
- 選択テキストへ指示を与えるAI変換
- OpenAI APIやApple Foundation Modelsとの通信
- macOS固有の設定画面
- 独自の候補ウィンドウ
- Swift APIとのソース互換性またはABI互換性

候補表示にはFcitx5の標準UIを使用します。変換候補、内部trace、prompt、token、logitが固定原作と完全に一致するかの厳密な適合検証は、まだ行っていません。

## 動作環境

動作確認済みの環境は、`x86_64-linux`、NixOS 26.11、Fcitx5 5.1.21、X11です。
Wayland、`aarch64-linux`、NixOS以外のLinuxディストリビューションでは、まだ実環境で確認していません。
推論には、`flake.lock`でnixpkgsのllama.cpp `b10273`を使用します。

## インストール

NixOS flakeへbeanKeyを追加し、NixOS moduleを読み込みます。

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    beanKey = {
      url = "github:ph0ryn/beankey";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { beanKey, nixpkgs, ... }:
    {
      nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          beanKey.nixosModules.default
          {
            programs.beanKey.enable = true;
          }
        ];
      };
    };
}
```

Switch後にFcitx5を再起動し、Fcitx5の入力メソッド設定から`beanKey`を追加してください。

## 設定

設定例

```nix
programs.beanKey = {
  enable = true;
  useBeanKeyTheme = true;

  conversion = {
    inputStyle = "roman_to_kana";
    typeBackslash = true;
    typeHalfSpace = true;
    punctuationStyle = "kuten_and_toten";
  };

  learning = {
    mode = "input_and_output";
    maxCount = 65536;
  };

  zenz = {
    inferenceLimit = 4;
    profile = "学生";
    topic = "プログラミング";
    preference = "カタカナ優先";
  };
};
```

詳細は[configuration.md](docs/configuration.md)を参照してください。

## 基本操作

| キー | 動作 |
| --- | --- |
| `Space` / `↓` | 変換を開始、または次の候補へ移動 |
| `Shift+Space` / `↑` | 前の候補へ移動 |
| `Enter` | 入力または選択中の範囲を確定 |
| `Escape` | 候補選択を戻す。入力中の文字列は保持 |
| `Tab` | 表示中の予測を受け入れる |
| `1`から`9` | 表示中の候補を番号で選択 |
| `F6` | ひらがなへ変換 |
| `F7` | カタカナへ変換 |
| `F8` | 半角カナへ変換 |
| `F9` | 全角英数へ変換 |
| `F10` | 半角英数へ変換 |
| `Ctrl+Shift+U` | Unicodeコードポイント入力を開始 |
| `Ctrl+Backspace` / `Ctrl+Delete` | 選択中の候補を学習結果から忘却 |

## ライセンス

MIT

### 参照実装

beanKeyは、次の実装を設計と動作の基準として参照しています。

- [AzooKeyKanaKanjiConverter](https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/93766c46e31fa6a18b7ced49dab31337780f6f45): かな漢字変換とZenzai
- [azooKey Desktop](https://github.com/azooKey/azooKey-Desktop/tree/3ae5a4651c329d48fee9b9ec7ac1bcd60b940a12): 日本語入力の操作と表示
- [fcitx5-mozc](https://github.com/fcitx/mozc/tree/3f8dea4bdf72c6af200ecdbe3d456871fb1d5e03/src/unix/fcitx5): Fcitx5連携とデーモン起動

### 再配布資産

- [azooKey_dictionary_storage](https://github.com/ensan-hcl/azooKey_dictionary_storage/tree/4d418525b090cf49c219819d05a7e3cc2a4346eb): デフォルト辞書
- [azooKey_emoji_dictionary_storage](https://github.com/ensan-hcl/azooKey_emoji_dictionary_storage/tree/67b822603391b01238d7b80b8b61b63f966cf357): 絵文字辞書
- [gpt2-small-japanese-char](https://huggingface.co/ku-nlp/gpt2-small-japanese-char): tokenizer
- [zenz-v3.2-small-gguf](https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf/tree/c67e03e07d215c869f591b274c1631170d3e11fe): Zenzaiモデル
