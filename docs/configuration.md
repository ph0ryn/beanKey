# 設定リファレンス

Nixでは`programs.beanKey.settings`に、TOMLと同じキー・構造で設定します。

LinuxとmacOSで同じ設定を使用します。導入先、runtime directory、学習directoryは各moduleとフロントエンドが決め、変換設定と分離します。

## 最小設定と既定値

Nixでは、moduleを読み込んで次の指定だけで動作します。

```nix
programs.beanKey.enable = true;
```

`settings`の既定値は`{ }`です。この場合は空のTOMLを生成し、daemonがすべての既定値を適用します。手書きのTOMLも空ファイルで有効です。各tableも省略でき、変更したい項目だけ記載できます。設定ファイル自体の欠損、リンク切れ、読み取りエラーは既定値への切り替えでは隠しません。

既定値と同じ値でも、Nixで明示した項目は出力します。`enable`と`useBeanKeyTheme`は導入側のoptionなので、TOMLへは書きません。任意項目を使わない場合は省略し、`null`は書かないでください。

設定の既定値・意味・検証は[daemonの設定実装](../crates/daemon/src/config.rs)が所有します。NixはTOMLへ変換できる型かを確認しますが、未知のキーや不正な設定値はdaemon起動時にエラーになります。設定はdaemon起動時に読み込みます。

### 配置

NixOSでは`beanKey.nixosModules.default`がFcitx5とbeanKeyアドオンを導入します。Home ManagerではLinuxとmacOSで`beanKey.homeModules.default`を読み込みます。どのmoduleも共通のdaemonと固定資産を使います。

- NixOS：`/etc/beankey/config.toml`から生成TOMLを参照します。
- LinuxのHome Manager：`xdg.configHome`配下の`beankey/config.toml`へ配置します。
- macOS：`~/Library/Application Support/beanKey/config.toml`を使います。Home Managerでは生成TOMLを参照します。単体installerは初回に設定ファイルを作成し、既存のファイルやリンクがある場合は変更しません。

Linuxのフロントエンドは`$XDG_CONFIG_HOME/beankey/config.toml`を優先し、環境変数が未指定・空・相対パスの場合は`$HOME/.config/beankey/config.toml`を使います。ユーザー設定がない場合だけ`/etc/beankey/config.toml`を使い、リンク切れやアクセスエラーではシステム設定へ切り替えません。Home Managerの`xdg.configHome`と実行環境の`XDG_CONFIG_HOME`を揃えてください。

LinuxのFcitx5はNixOSかHome Managerの一方で管理します。macOSの入力ソースへの追加はシステム設定で行います。Nixで管理している設定は生成物を直接編集せず、`programs.beanKey.settings`を変更してください。

### 固定資産

固定辞書、絵文字、GGUFモデル、英語・ギリシャ語辞書はdaemon実体の`../share/beankey`から読み込みます。これらのpathは設定しません。socketのruntime rootと学習directoryはフロントエンドが指定します。

## 設定例

設定例です。`settings`内の項目はすべて省略できます。

```nix
programs.beanKey = {
  enable = true;
  useBeanKeyTheme = true; # Linuxのみ。macOSではこの行を省略します。

  settings = {
    conversion = {
      input_style = "roman_to_kana";
      type_backslash = false;
      type_half_space = false;
      punctuation_style = "kuten_and_toten";
    };
    learning = {
      mode = "input_and_output";
      max_count = 65536;
    };
    zenz.inference_limit = 5;
  };
};
```

`settings`から生成されるTOMLは次と同じ内容です。

```toml
[conversion]
input_style = "roman_to_kana"
type_backslash = false
type_half_space = false
punctuation_style = "kuten_and_toten"

[learning]
mode = "input_and_output"
max_count = 65536

[zenz]
inference_limit = 5
```

## Nixの導入option

以下は`programs.beanKey`直下のoptionです。

| option | 型 | 既定値 | 説明 |
| --- | --- | --- | --- |
| `enable` | `bool` | `false` | OSに対応するbeanKeyフロントエンドを導入 |
| `useBeanKeyTheme` | `bool` | `false` | Linux専用。同梱のFcitx5 Classic UIテーマを適用 |
| `settings` | TOMLとして表現できるattribute set | `{ }` | daemonへ渡す設定。指定した項目だけ出力 |

`useBeanKeyTheme`はClassic UIのテーマ、フォント、accent color設定に既定値を設定します。他で明示した設定があれば、そちらが優先されます。macOSの候補パネルは常に同梱テーマの配色と余白を使います。独自設定GUIはありません。

以下の表はdaemonのTOMLキーと既定値です。Nixでは`programs.beanKey.settings`内に同じ構造で指定します。

## 変換

TOMLの`[conversion]`、Nixの`settings.conversion`で入力方式と候補生成を設定します。

| キー | 型 | 既定値 | 説明 |
| --- | --- | --- | --- |
| `input_style` | enum | `"roman_to_kana"` | 入力方式 |
| `type_backslash` | `bool` | `false` | 円記号キーで`¥`ではなく`\`を入力 |
| `type_half_space` | `bool` | `false` | `Space`を半角、`Shift+Space`を全角スペースに設定 |
| `punctuation_style` | enum | `"kuten_and_toten"` | コンマキーとピリオドキーの句読点 |
| `live_conversion` | `bool` | `true` | 入力中に最良の完全一致候補を表示 |
| `n_best` | 正の整数 | `10` | 一度に要求する変換候補数 |
| `japanese_prediction` | enum | `"disabled"` | 日本語予測の動作 |
| `foreign_prediction` | enum | `"disabled"` | 英語・ギリシャ語予測の動作 |
| `keyboard_language` | enum | `"japanese"` | 外国語補完に使うキーボード言語 |
| `typo_correction` | enum | `"automatic"` | 辞書による入力訂正の動作 |
| `full_width_roman` | `bool` | `true` | 全角英数候補を生成 |
| `half_width_kana` | `bool` | `false` | 半角カナ候補を生成 |
| `typography` | `bool` | `false` | タイポグラフィ候補を生成 |
| `option_direct_full_width_input` | `bool` | `false` | 未入力時の`Alt`または`Shift+Alt`で英数字や記号を直接全角入力 |
| `user_dictionary` | パス文字列（省略可） | 未指定 | JSONユーザー辞書 |
| `user_dictionary_directory` | パス文字列（省略可） | 未指定 | azooKey形式ユーザー辞書ディレクトリ |
| `custom_input_table` | 文字列（省略可） | 未指定 | 既定で使うカスタム入力テーブル名 |
| `custom_input_tables` | table | `{ }` | 入力テーブル名とJSONファイルの対応 |

`input_style`には`"direct"`、`"roman_to_kana"`、`"azik"`、`"kana_jis"`、`"kana_us"`、`"custom"`を指定できます。`keyboard_language`には`"none"`、`"japanese"`、`"english_us"`、`"greek"`を指定できます。

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

`input_style = "custom"`を使う場合は、`custom_input_tables`へJSONファイルを登録し、その名前を`custom_input_table`へ指定します。

```nix
programs.beanKey.settings.conversion = {
  input_style = "custom";
  custom_input_table = "my-table";
  custom_input_tables.my-table = "/home/me/.config/beankey/my-input-table.json";
};
```

`custom_input_table`が`custom_input_tables`に存在しない場合は、daemon起動時にエラーになります。Nixのpath値で指定するとファイルをNix storeへ取り込み、文字列なら実行時にそのパスを読み込みます。

### ユーザー辞書

```nix
programs.beanKey.settings.conversion = {
  user_dictionary = "/home/me/.config/beankey/user.json";
  user_dictionary_directory = "/home/me/.local/share/azookey/user-dictionary";
};
```

可変なホームディレクトリ上のファイルには、絶対パスの文字列を指定してください。文字列のパスをNix storeへ自動的に取り込むことはありません。

## 学習

TOMLの`[learning]`、Nixの`settings.learning`で候補選択の学習を設定します。

| キー | 型 | 既定値 | 説明 |
| --- | --- | --- | --- |
| `mode` | enum | `"input_and_output"` | 学習データの読み書き方法 |
| `max_count` | 0以上の整数 | `65536` | 永続化する学習レコード数の上限 |

`mode`に指定できる値は次のとおりです。

- `"input_and_output"`：既存の学習データを読み込み、新しい学習結果も保存
- `"only_output"`：既存の学習データを読み込むが、新しい学習結果は保存しない
- `"nothing"`：学習データを読み書きしない

## Zenzai

TOMLの`[zenz]`、Nixの`settings.zenz`で固定GGUFモデルによる候補評価を設定します。モデル自体を差し替える設定はありません。

| キー | 型 | 既定値 | 説明 |
| --- | --- | --- | --- |
| `inference_limit` | 0以上の整数 | `5` | prefix修正を繰り返す回数の上限。`0`では再探索しない |
| `profile` | 文字列（省略可） | 未指定 | 利用者プロフィール条件 |
| `topic` | 文字列（省略可） | 未指定 | 話題や分野の条件 |
| `preference` | 文字列（省略可） | 未指定 | 表記の優先条件 |
| `style` | 文字列（省略可） | 未指定 | 文体の条件 |
| `predictive_input` | `bool` | `false` | Zenzaiの次入力予測を有効化 |
| `rich_candidates` | `bool` | `false` | rich candidateを要求 |
| `personalization` | table（省略可） | 未指定 | EfficientNGramによる個人化 |
| `enable_alignment_separator` | `bool` | `true` | promptにアラインメント区切りを追加 |

### 個人化

```nix
programs.beanKey.settings.zenz.personalization = {
  base_ngram = "/absolute/path/to/base/lm";
  personal_ngram = "/absolute/path/to/personal/lm";
  alpha = 1.0;
};
```

`personalization`を指定する場合は、3項目とも必須です。`base_ngram`と`personal_ngram`はEfficientNGramファイル群のprefixです。`alpha`は有限かつ0以上の補間強度で、既定値はありません。

## 言語モデル入力訂正

TOMLの`[lm_typo]`、Nixの`settings.lm_typo`は実験的な入力訂正機能です。

| キー | 型 | 既定値 | 説明 |
| --- | --- | --- | --- |
| `enabled` | `bool` | `false` | LM入力訂正候補を有効化 |
| `language_model` | enum | `"zenz"` | 使用する言語モデル |
| `ngram` | table（省略可） | 未指定 | EfficientNGram設定 |
| `n_best` | 正の整数 | `5` | 返す入力訂正候補数 |
| `beam_size` | 正の整数 | `32` | 探索時のビーム幅 |
| `top_k` | 正の整数 | `64` | 各ステップで探索するtoken数の上限 |
| `max_steps` | 0以上の整数（省略可） | 未指定 | decode step数の上限。省略時は自動決定 |
| `substitution_cost` | 有限かつ0以上の数値 | `2.0` | 文字置換のchannel cost |
| `deletion_cost` | 有限かつ0以上の数値 | `3.0` | 文字削除のchannel cost |
| `transposition_cost` | 有限かつ0以上の数値 | `2.0` | 隣接文字入れ替えのchannel cost |

候補数・探索幅・costの値は`enabled = true`のときに検証します。`language_model`には`"zenz"`または`"ngram"`を指定できます。有効化して`"ngram"`を使う場合は`ngram`も必要です。

```nix
programs.beanKey.settings.lm_typo = {
  enabled = true;
  language_model = "ngram";
  ngram = {
    prefix = "/absolute/path/to/ngram/lm";
    tokenizer = "/absolute/path/to/tokenizer.json";
    n = 5;
    discount = 0.75;
  };
};
```

`ngram`を指定する場合は、4項目とも必須です。`prefix`はEfficientNGramファイル群のprefix、`tokenizer`は対応するtokenizer JSONへのパスです。`n`は2以上、`discount`は有限の数値を指定します。Nixによるtokenizerの自動補完はありません。

## 推論の固定profile

`[inference]`は通常、省略します。daemonは`context_size = 512`、`batch_size = 512`、`micro_batch_size = 64`、`flash_attention = true`を使い、これらと異なる値は拒否します。
