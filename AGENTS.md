# AGENTS.md

## プロジェクトの範囲

- beanKey は、NixOSとmacOSで動作するかな漢字変換器を実装するプロジェクトである。
- Fcitx5とmacOSは横並びのフロントエンドとし、公式azooKey Desktopを基準に、操作と使用感をほぼ互換にする。OSによる軽微な差は許容するが、一時的な制約は記録して将来の互換対象に含める。片方を従属させず、共通の操作感のために両方を変更できる。
- 製品コードを Swift に依存させない。
- Linuxの候補表示はFcitx5標準UIに委ねる。macOSはInputMethodKitとObjective-C++を使い、候補・注釈・予測だけの非アクティブパネルを同梱Fcitx5テーマに合わせて実装する。独自設定GUIは含めない。
- Rust で変換本体と daemon を実装し、Fcitx5 addon は薄い C++20 の層とする。
- Fcitx5 addon と daemon は Unix domain socket 上の Protobuf で通信する。
- daemon の起動は fcitx5-mozc の方式に合わせ、addon が接続時に必要であれば daemon を直接起動する。systemd service や socket activation は利用しない。
- ニューラル推論には、`flake.lock` で固定された nixpkgs の `pkgs.llama-cpp` を利用する。
- 英語・ギリシャ語補完には、同じnixpkgsの`pkgs.hunspell`、`pkgs.hunspellDicts.en_US`および`pkgs.hunspellDicts.el_GR`を利用する。
- 原作が利用する llama.cpp `b4846` と同じ source revision は要求せず、原作との挙動差は固定した `pkgs.llama-cpp` を使って検証する。
- 対応するGGUFモデルは、[Miwa-Keita/zenz-v3.2-small-gguf](https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf) の `ggml-model-Q5_K_M.gguf` だけとする。
- flakeはモデルrepositoryのcommit `c67e03e07d215c869f591b274c1631170d3e11fe`とhash `sha256-KcIj1MIzJ7gP0T67WrJVUFekYxeZfV2jkVhP++8NtnM=`を持つ`pkgs.fetchurl` derivationを定義する。NixOS moduleはそのNix store pathをdaemonへ渡し、モデルを差し替えるoptionは公開しない。daemon自身もモデルをダウンロードしない。
- 利用者向け設定はすべて`programs.beanKey`配下へ集約する。手書きのdaemon設定、設定用環境変数または別namespaceを公開しない。
- 固定した原作スナップショットで確認できるかな漢字変換機能を1機能ずつ実装し、最終的にはすべてを対象にする。MVPやv1などの別成果範囲は設けない。
- 現段階では各機能がFcitx5とmacOS上で正常に動作することを検証し、原作との厳密な出力・内部trace比較は後続作業とする。
- Fcitx5 APIの利用、キーのconsumed判定、プリエディット、候補および確定結果の反映は、[fcitx5-mozc](https://github.com/fcitx/mozc/tree/3f8dea4bdf72c6af200ecdbe3d456871fb1d5e03/src/unix/fcitx5)の構成に準拠する。日本語入力の状態遷移と表示意味は固定した公式azooKey Desktopを基準にし、Mozc固有protocolは流用しない。
- Mozcのsource、library、daemonまたは`pkgs.mozc`へ依存しない。原作の生成済み絵文字辞書に含まれるMozc由来dataは、資産のattributionとしてのみ扱う。
- 他の AzooKeyKanaKanjiConverter の Linux 向け移植は、設計や実装の参考にしない。
- 原作調査で確認できていない仕様は推測で決めず、未決事項として残す。

## 文書の責任

- `README.md` は、プロジェクトの概要、導入、基本操作、対応状況、原作との差および固定した主要依存を記載する利用者向け文書とする。
- `docs/configuration.md` は、`programs.beanKey`の利用者向け設定の正本とする。
- `docs/architecture.md` は、コンポーネント、依存方向、プロセス境界およびNixOS統合の正本とする。
- このファイルは、実装時に守る責務とリポジトリ境界を定義する。利用方法、対応状況または確定した設計を変更する場合は、対応する文書とこのファイルを同時に更新する。

## リポジトリ構成

実装領域は次の構成とする。

```text
.
├── Cargo.toml
├── crates/
│   ├── converter/   # 辞書読込とかな漢字変換本体
│   ├── llama/       # llama.cpp との FFI 境界
│   └── daemon/      # IPC セッションと各コンポーネントの接続
├── fcitx5/          # 薄い C++20 addon
│   ├── CMakeLists.txt
│   └── src/
├── proto/
│   └── beankey.proto
├── nix/             # NixOS moduleとpackage定義が必要になった時点で追加
├── flake.nix
└── flake.lock
```

空ディレクトリを維持するための `.gitkeep` は追加しない。各ディレクトリは、その責務に属する最初の実装と同時に作成する。

## 責務と依存境界

### `crates/converter`

- 辞書形式の読込、候補生成、経路探索、Zenzai prompt構築およびprefix制約付き再探索などの変換ロジックを所有する。
- Hunspell C APIの最小binding、辞書encoding変換、英語・ギリシャ語候補合成を所有し、C pointerをcrate外へ露出させない。handleは生成した専用worker thread内に固定する。
- Fcitx5、IPC、daemon のライフサイクル、llama.cpp の C API に依存させない。
- OS 固有処理を持ち込まない。

### `crates/llama`

- `pkgs.llama-cpp`のC APIのうち必要な部分に対するRust bindingと、モデルのロード、tokenize、decodeおよびlogit取得を所有する。
- llama.cpp 固有の型やポインタを crate の外へ露出させない。
- 変換アルゴリズムや IPC を実装しない。
- llama.cpp の取得とビルドは Nix に任せ、`pkgs.llama-cpp`のshared libraryへlinkする。この crate で sourceをvendor、再包装、静的linkまたは独自ビルドしない。

### `crates/daemon`

- Unix domain socket、Protobuf、IPC セッション、モデルパスを含む設定および各コンポーネントの接続を所有する。
- `converter` と `llama` を組み合わせるアプリケーション境界とする。
- Desktop準拠の入力中、preview中、候補選択中の状態遷移をセッションごとに所有する。
- `converter` の変換要求と `llama` の推論結果を接続し、Zenzaiの評価とprefix制約付き再探索のループを進行する。
- 辞書探索や候補順位付けのロジックを持たない。
- addon から直接起動できる単一のserver executableを提供する。
- フロントエンドが明示したruntime rootの`beankey/daemon.sock`でユーザー単位に待ち受け、接続peerのUIDを検証する。Linuxは`SO_PEERCRED`、macOSは`getpeereid`を使う。
- runtime directory内のlockをdaemonの生存中保持し、lock取得後にだけ同一UIDのstale socketを除去する。
- 同一セッションの要求を順番に処理し、異なるセッションは独立させる。共有llama.cpp contextへの推論は直列化する。

### `fcitx5`

- Fcitx5 の入力コンテキスト、キーイベントの意味的操作への正規化、標準 UI への候補提示および daemon との通信を所有する。
- かな漢字変換やニューラル推論を実装しない。
- Rust 実装へ直接リンクせず、`proto/beankey.proto` で定義した IPC 境界を利用する。
- daemonへ接続できない場合はserver executableを起動し、接続を再試行する。
- fcitx5-mozcと同様に、daemon応答がキーをconsumedとした場合だけFcitx5のeventをacceptし、結果を標準のプリエディット、候補UIおよびcommit APIへ反映する。
- daemonへの接続または要求が失敗した場合はセッションとUIをresetし、未処理のキーをFcitx5へ返す。キーをaddon内でbufferまたは再送しない。

### `proto`

- `ipc/cpp`は両フロントエンドの共有C++ transportと生成Protobuf型を所有する。
- `macos`はInputMethodKit、キー正規化、UTF-16変換、marked text、確定、候補パネル、daemon起動を所有する。Rustへの直接linkや変換ロジックを持たない。
- Rust と C++ の間で共有する通信仕様の唯一の正本とする。
- 生成コードはコミットせず、各ビルドで生成する。
- 内部実装の都合だけで通信仕様を拡張しない。
- Fcitx5固有のkey symbolをwireへ流さず、意味的操作、入力状態、候補window、注釈付き候補および独立した予測を表現する。
- wire formatはvarint length-delimited Protobufとし、各envelopeにprotocol version、request ID、session IDおよびpayloadを持たせる。
- 1メッセージの上限は1 MiBとし、超過、未知のversion、不正なpayloadは接続単位の明示的なprotocol errorとして扱う。

### `nix`

- 辞書と絵文字辞書は`nix/assets.nix`の`pkgs.fetchFromGitHub`でcommitとhashを固定して取得し、Git submoduleやvendorしたコピーを持たない。
- 製品は取得した生成済み辞書を直接packageし、辞書生成器や別形式への変換を含めない。更新は互換性を確認したうえでrevisionとhashを明示的に変更する。
- 開発環境とpackageのテストは、同じ辞書packageのNix store pathをテスト専用の`BEANKEY_TEST_DICTIONARY`と`BEANKEY_TEST_EMOJI_DICTIONARY`で渡す。これらをdaemonの利用者向け設定には使用しない。
- NixOS moduleで`programs.beanKey.enable`を公開する。
- 今後追加する利用者向けoptionも`programs.beanKey`配下に置く。NixOS moduleとmacOS用Home Manager moduleは、option定義と内部TOML生成を共有する。
- モデルはflakeが固定した`pkgs.fetchurl { url; hash; }` derivationとしてNixOS moduleから参照し、利用者向けoptionにしない。
- NixOS moduleはFcitx5 addon、daemon、辞書および固定モデルを導入し、`programs.beanKey`から内部用のdaemon設定を生成する。
- NixOS moduleはHunspellと固定nixpkgsの英語・ギリシャ語辞書を導入し、そのNix store pathを内部用daemon設定へ書く。辞書pathを利用者向けoptionにしない。
- 直接packageする辞書、モデル、tokenizerおよび絵文字には、資産ごとのlicense本文、取得元、固定revision、attributionおよび変更有無を添付する。通常依存の`pkgs.llama-cpp`、`pkgs.hunspell`および`pkgs.hunspellDicts`をこの資産台帳へ重複登録しない。
- flakeは`packages.<system>.daemon`、`packages.<system>.fcitx5-addon`、`packages.<system>.model`および`nixosModules.default`を公開する。
- NixOS moduleは内部設定をTOMLとして生成し、`/etc/beankey/config.toml`からNix store上の生成物を参照させる。addonはdaemonを`--config /etc/beankey/config.toml`付きで起動する。
- package境界は [fcitx5-mozc](https://github.com/NixOS/nixpkgs/blob/8c91a71d13451abc40eb9dae8910f972f979852f/pkgs/by-name/fc/fcitx5-mozc/package.nix#L36-L45) と [mozc](https://github.com/NixOS/nixpkgs/blob/8c91a71d13451abc40eb9dae8910f972f979852f/pkgs/by-name/mo/mozc/package.nix#L69-L105) を基準とする。
- daemonとFcitx5 addonを別のpackageとして定義し、addon packageからdaemon packageを参照する。
- addonにはdaemon executableのNix store pathを埋め込み、runtimeの`PATH`検索に依存させない。
- addonのshared libraryと設定は、Fcitx5の標準ディレクトリへインストールする。
- `programs.beanKey`のNixOS moduleは提供するが、daemon起動専用のmoduleやsystemd unitは作成しない。
- アプリケーションロジック、プロジェクト文書、テストデータを置かない。
- llama.cpp は `pkgs.llama-cpp` を利用し、独立した flake input や独自 derivation を追加しない。
- Hunspellと英語・ギリシャ語辞書はnixpkgs packageを利用し、sourceや辞書をvendorしない。
- 実装が存在しない将来用ファイルは作成しない。

## 依存方向

- `daemon` は `converter` と `llama` を利用し、`proto/beankey.proto` から生成した型を IPC に利用する。
- `fcitx5` は `proto/beankey.proto` から生成した型を IPC に利用するが、Rust crate へ依存しない。
- `converter` は実行時に渡されたパスから固定辞書とHunspell辞書を読むが、`daemon`、`fcitx5`、Protobuf または llama.cpp の C API へ依存しない。
- `llama` は `daemon`、`fcitx5` または Protobuf へ依存しない。
- 上記と逆向きの依存が必要になった場合は、責務の置き場所を先に見直す。

## テストの配置

- Rust の単体テストと crate 単位の統合テストは、所有する crate 内へ置く。
- C++ addonのテストは`fcitx5`、共有IPCのテストは`ipc/cpp`、macOSフロントエンドのテストは`macos`内へ置く。
- daemon、IPC、addon など複数の境界を跨ぐテストが必要になった場合だけ、ルートに `tests/` を追加する。
- 辞書ファイルや依存パッケージの存在だけを確認する先行テストは追加せず、実際の利用境界で検証する。
- 各機能は、入力、候補生成、選択、確定、必要な永続化および異常時の状態復旧を外部から確認できる最小シナリオで検証する。
- 現段階のテストは正常動作を合格基準とする。固定原作との候補列、内部traceおよびlogitの厳密比較は後続の適合検証として分離する。

## 実装時の規則

- 新しい crate は、独立した責務と依存境界が実装上必要になってから追加する。
- 共通化を目的とした `common`、`shared`、`utils` crate は作らない。実際の重複と所有者を確認してから配置を決める。
- Protobuf や FFI の生成物、ビルド成果物、モデルファイルは Git にコミットしない。
- Markdown は、ユーザーから明示的に依頼された場合を除いて追加または変更しない。
