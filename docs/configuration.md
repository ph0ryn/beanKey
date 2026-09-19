# 設定リファレンス

beanKeyの設定は、すべて`programs.beanKey`配下にあります。

NixOSとmacOSでは同じ変換・学習設定を使用します。OS固有の導入先、runtime directory、学習directoryは各moduleとフロントエンドが決め、変換の公開設定と分離します。

## 最小設定

```nix
programs.beanKey.enable = true;
```

NixOSでは`beanKey.nixosModules.default`がFcitx5とbeanKeyアドオンを導入します。macOSではHome Managerへ`beanKey.homeModules.default`を読み込むと、InputMethodKitアプリを`~/Library/Input Methods/beanKey.app`へ導入します。どちらも共通の変換デーモン、辞書、GGUFモデルを使用します。macOSの入力ソースへの追加はシステム設定で行います。

## 設定例

```nix
programs.beanKey = {
  enable = true;
  useBeanKeyTheme = true; # NixOSのみ。macOSではこの行を省略します。

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

## 基本設定

| option | 型 | 初期値 | 説明 |
| --- | --- | --- | --- |
| `enable` | `bool` | `false` | OSに対応するbeanKeyフロントエンドを導入 |
| `useBeanKeyTheme` | `bool` | `false` | NixOS専用。同梱のFcitx5 Classic UIテーマを適用 |

`useBeanKeyTheme`はClassic UIのテーマ、フォント、accent color設定に既定値を設定します。ほかのNixOS設定で明示した値がある場合は、そちらが優先されます。

macOSの候補パネルは常に同梱テーマの配色と余白を使用します。独自設定GUIやOS別の変換設定namespaceはありません。

## 変換

`programs.beanKey.conversion`で入力方式と候補生成を設定します。

| option | 型 | 初期値 | 説明 |
| --- | --- | --- | --- |
| `inputStyle` | enum | `"roman_to_kana"` | 入力方式 |
| `typeBackslash` | `bool` | `false` | 円記号キーで`¥`ではなく`\`を入力 |
| `typeHalfSpace` | `bool` | `false` | `Space`を半角、`Shift+Space`を全角スペースに設定 |
| `punctuationStyle` | enum | `"kuten_and_toten"` | コンマキーとピリオドキーの句読点 |
| `liveConversion` | `bool` | `true` | 入力中に最良の完全一致候補を表示 |
| `candidateCount` | 正の整数 | `10` | 一度に要求する変換候補数 |
| `japanesePrediction` | enum | `"disabled"` | 日本語予測の動作 |
| `foreignPrediction` | enum | `"disabled"` | 英語・ギリシャ語予測の動作 |
| `keyboardLanguage` | enum | `"japanese"` | 外国語補完に使うキーボード言語 |
| `typoCorrection` | enum | `"automatic"` | 辞書による入力訂正の動作 |
| `fullWidthRoman` | `bool` | `true` | 全角英数候補を生成 |
| `halfWidthKana` | `bool` | `false` | 半角カナ候補を生成 |
| `typography` | `bool` | `false` | タイポグラフィ候補を生成 |
| `optionDirectFullWidthInput` | `bool` | `false` | 未入力時の`Alt`または`Shift+Alt`で英数字や記号を直接全角入力 |
| `userDictionary` | `null`または絶対パス | `null` | JSONユーザー辞書 |
| `userDictionaryDirectory` | `null`または絶対パス | `null` | azooKey形式ユーザー辞書ディレクトリ |
| `customInputTable` | `null`または文字列 | `null` | 既定で使うカスタム入力テーブル名 |
| `customInputTables` | attribute set | `{ }` | 入力テーブル名とJSONファイルの対応 |

`inputStyle`に指定できる値は次のとおりです。

- `"direct"`
- `"roman_to_kana"`
- `"azik"`
- `"kana_jis"`
- `"kana_us"`
- `"custom"`

`keyboardLanguage`に指定できる値は`"none"`、`"japanese"`、`"english_us"`、`"greek"`です。

予測には`"automatic"`、`"manual"`、`"disabled"`、辞書入力訂正には`"enabled"`、`"automatic"`、`"disabled"`を指定できます。

### 句読点

| 値 | コンマキー | ピリオドキー |
| --- | --- | --- |
| `"kuten_and_toten"` | `、` | `。` |
| `"kuten_and_comma"` | `，` | `。` |
| `"period_and_toten"` | `、` | `．` |
| `"period_and_comma"` | `，` | `．` |

`Alt`を押している間は、設定した和文・欧文記号の組み合わせが反転します。

### カスタム入力テーブル

`inputStyle = "custom"`を使う場合は、`customInputTables`へJSONファイルを登録し、その名前を`customInputTable`へ指定します。

```nix
programs.beanKey.conversion = {
  inputStyle = "custom";
  customInputTable = "my-table";
  customInputTables.my-table = ./my-input-table.json;
};
```

`customInputTable`が`customInputTables`に存在しない場合は、NixOS moduleの評価が失敗します。

### ユーザー辞書

```nix
programs.beanKey.conversion = {
  userDictionary = "/home/me/.config/beankey/user.json";
  userDictionaryDirectory = "/home/me/.local/share/azookey/user-dictionary";
};
```

これらは実行時に読み込むため、絶対パスで指定します。可変なホームディレクトリ上のデータであり、Nix storeへ自動的に取り込まれるわけではありません。

## 学習

`programs.beanKey.learning`で候補選択の学習を設定します。

| option | 型 | 初期値 | 説明 |
| --- | --- | --- | --- |
| `mode` | enum | `"input_and_output"` | 学習データの読み書き方法 |
| `maxCount` | 0以上の整数 | `65536` | 永続化する学習レコード数の上限 |

`mode`に指定できる値は次のとおりです。

- `"input_and_output"`: 既存の学習データを読み込み、新しい学習結果も保存
- `"only_output"`: 既存の学習データを読み込むが、新しい学習結果は保存しない
- `"nothing"`: 学習データを読み書きしない

## Zenzai

`programs.beanKey.zenz`で、固定GGUFモデルによる候補評価を設定します。モデル自体を差し替えるoptionはありません。

| option | 型 | 初期値 | 説明 |
| --- | --- | --- | --- |
| `inferenceLimit` | 1から50の整数 | `5` | prefix修正を繰り返す回数の上限 |
| `profile` | `null`または文字列 | `null` | 利用者プロフィール条件 |
| `topic` | `null`または文字列 | `null` | 話題や分野の条件 |
| `preference` | `null`または文字列 | `null` | 表記の優先条件 |
| `style` | `null`または文字列 | `null` | 文体の条件 |
| `predictiveInput` | `bool` | `false` | Zenzaiの次入力予測を有効化 |
| `richCandidates` | `bool` | `false` | rich candidateを要求 |
| `personalization` | `null`またはsubmodule | `null` | EfficientNGramによる個人化 |
| `enableAlignmentSeparator` | `bool` | `true` | promptにアラインメント区切りを追加 |

### 個人化

```nix
programs.beanKey.zenz.personalization = {
  baseNgram = "/absolute/path/to/base/lm";
  personalNgram = "/absolute/path/to/personal/lm";
  alpha = 1.0;
};
```

`baseNgram`と`personalNgram`はEfficientNGramファイル群の絶対prefixです。`alpha`は0以上の補間強度で、初期値は`1.0`です。

## 言語モデル入力訂正

`programs.beanKey.lmTypo`は実験的な入力訂正機能です。

| option | 型 | 初期値 | 説明 |
| --- | --- | --- | --- |
| `enabled` | `bool` | `false` | LM入力訂正候補を有効化 |
| `languageModel` | enum | `"zenz"` | 使用する言語モデル |
| `ngram` | `null`またはsubmodule | `null` | EfficientNGram設定 |
| `candidateCount` | 正の整数 | `5` | 返す入力訂正候補数 |
| `beamSize` | 正の整数 | `32` | 探索時のビーム幅 |
| `topK` | 正の整数 | `64` | 各ステップで探索するtoken数の上限 |
| `maxSteps` | `null`または正の整数 | `null` | decode step数の上限。`null`では自動決定 |
| `substitutionCost` | 浮動小数点数 | `2.0` | 文字置換のchannel cost |
| `deletionCost` | 浮動小数点数 | `3.0` | 文字削除のchannel cost |
| `transpositionCost` | 浮動小数点数 | `2.0` | 隣接文字入れ替えのchannel cost |

`languageModel`には`"zenz"`または`"ngram"`を指定できます。`"ngram"`を選ぶ場合は`ngram`も必要です。

```nix
programs.beanKey.lmTypo = {
  enabled = true;
  languageModel = "ngram";
  ngram = {
    prefix = "/absolute/path/to/ngram/lm";
    n = 5;
    discount = 0.75;
  };
};
```

`prefix`はEfficientNGramファイル群の絶対prefixです。`n`は正の整数、`discount`は浮動小数点数です。
