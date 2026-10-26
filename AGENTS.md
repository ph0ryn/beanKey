# AGENTS.md

このファイルは、コードを読む・変更する前に知っておくべき目的、参照基準、制約を記す。作業時にコードで確認する処理手順、具体的な配置パス、公開名の一覧、アセットのrevision・hashは転記しない。

## プロジェクトの範囲

- beanKeyは、NixOSとmacOSで動作するかな漢字変換器である。
- Fcitx5とmacOSは対等なフロントエンドとし、公式azooKey Desktopと操作・使用感をほぼ互換にする。OSによる軽微な差は許容するが、一時的な制約は記録して将来の互換対象に含める。片方を従属させず、共通の操作感のために両方を変更できる。
- 固定した原作で確認できるかな漢字変換機能を1機能ずつ実装し、最終的にはすべてを対象にする。MVPやv1などの別成果範囲は設けない。
- 変換本体とdaemonはRust、Fcitx5 addonは薄いC++20の層、macOSはInputMethodKitとObjective-C++で実装する。製品コードをSwiftに依存させない。
- Linuxの候補表示はFcitx5標準UIに委ねる。macOSは候補・注釈・予測だけの非アクティブパネルを同梱Fcitx5テーマに合わせ、入力先からフォーカスを奪わない。独自設定GUIは含めない。

## 参照基準

| 固定した参照元 | 基準にするもの |
| --- | --- |
| [AzooKeyKanaKanjiConverter](<https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/93766c46e31fa6a18b7ced49dab31337780f6f45>) | かな漢字変換とZenzai |
| [azooKey Desktop](<https://github.com/azooKey/azooKey-Desktop/tree/3ae5a4651c329d48fee9b9ec7ac1bcd60b940a12>) | 日本語入力の操作、状態遷移、表示 |
| [fcitx5-mozc](<https://github.com/fcitx/mozc/tree/3f8dea4bdf72c6af200ecdbe3d456871fb1d5e03/src/unix/fcitx5>) | Fcitx5 APIとの連携、daemon起動 |

- 固定版以降の採用済み差分は、[READMEの参照実装](<README.md#参照実装>)を参照する。
- 変換ライブラリの候補アクションをDesktopの操作仕様と混同しない。日本語入力の操作・表示はDesktopを基準にし、Mozc固有protocolは流用しない。
- Mozcのsource、library、daemon、packageへ依存しない。生成済み絵文字辞書に含まれるMozc由来データは、資産のattributionとして扱う。
- 他のAzooKeyKanaKanjiConverterのLinux向け移植は、設計や実装の参考にしない。
- 原作調査で確認できていない仕様は推測で決めず、未決事項として残す。

## 責務と依存境界

| 所有者 | 責務と制約 |
| --- | --- |
| [converter](<crates/converter/>) | OS非依存の変換ロジック、辞書、候補生成、学習、Zenzaiのpromptと再探索。daemon、フロントエンド、IPC、llama.cpp C APIへ依存しない。 |
| [llama](<crates/llama/>) | llama.cppのFFI境界とモデル操作。固有の型・ポインタを外へ露出せず、daemon・フロントエンド・IPCへ依存しない。変換アルゴリズムを持たない。 |
| [daemon](<crates/daemon/>) | IPCセッション、入力状態遷移、利用者設定、converterとllamaの接続・進行。辞書探索や候補順位付けはconverterに委ねる。 |
| [fcitx5](<fcitx5/>)・[macos](<macos/>) | OSの入力API、キーの意味的操作への正規化、UI、daemon起動。変換・推論を実装せず、Rust crateへ直接linkしない。 |
| [ipc](<ipc/>) | 通信仕様と両フロントエンドの共有C++ transport。OSのUIや変換状態遷移を持たない。 |
| [nix](<nix/>) | packageとNixOS・Home Manager統合。アプリケーションロジック、プロジェクト文書、テストデータを置かない。 |

- 上記と逆向きの依存が必要になった場合は、責務の置き場所を先に見直す。
- フロントエンドとdaemonはUnix domain socket上のProtobufで通信する。[beankey.proto](<ipc/beankey.proto>)を通信仕様の唯一の正本とし、内部実装の都合だけで拡張しない。Fcitx5固有のkey symbolではなく、意味的操作をやり取りする。

### 起動・障害・安全性

- daemonはフロントエンドが必要時に直接起動する。systemd serviceやsocket activationは利用しない。
- daemonとFcitx5 addonは別packageとし、addonからdaemonを参照する。daemon executableのNix store pathを固定し、runtimeの`PATH`検索には依存しない。
- 準備中や通信失敗時は未処理のキーを入力先へ返す。通信失敗時はセッションとUIをresetし、フロントエンド内でキーをbuffer・再送しない。Fcitx5では、daemon応答がconsumedとした場合だけキーを処理済みにする。
- daemonはユーザー単位で待ち受け、接続peerのUIDを検証する。process lockを生存中保持し、lock取得後にだけ同一UIDのstale socketを除去する。
- メッセージのサイズ上限、protocol version、payloadを検証し、不正な要求は接続単位の明示的なprotocol errorとして扱う。
- 同一セッションの要求順序とセッション間の独立性を保ち、共有llama.cpp contextへの推論は直列化する。
- macOS installerは入力ソースの有効化・選択を変更しない。

## 依存と配布の方針

- llama.cppの取得・静的ビルドは、versionを完全固定したCargo依存の`llama-cpp-sys-2`に任せる。OSやNix・通常のCargo buildで別sourceを選ばず、feature方針も揃える。原作と同じllama.cpp revisionであることは要求しない。
- llama.cppのsystem library、外部backend directory、独自C shim、sourceのvendor・再包装・独自derivationは追加しない。CPU backendを維持し、macOSではMetalを有効にする。host固有ISAの暗黙有効化やOpenMP、不要なGPU toolkitへの依存を避ける。
- 英語・ギリシャ語補完は`spellbook`と固定したHunspell形式辞書を使い、Hunspell本体へ依存しない。対話的な補完の遅延を抑えるため、n-gram候補生成は無効にする。
- 対応モデルは[zenz-v3.2-small-gguf](<https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf>)のQ5_K_Mのみとする。モデルはpackageに同梱し、差し替える設定やdaemon自身がダウンロードする仕組みは追加しない。
- 辞書・モデル・tokenizerは固定資産として管理する。生成済み辞書を直接packageし、辞書生成器や別形式への変換は含めない。更新時は互換性とlicenseを確認する。
- 直接配布する資産の取得元、固定revision・hash、license、attribution、変更有無は[THIRD_PARTY_NOTICES.txt](<THIRD_PARTY_NOTICES.txt>)で管理し、必要なlicense本文・noticeをpackageへ添付する。Cargo依存と同梱llama.cppを資産台帳へ重複登録しないが、静的linkした依存のlicense本文・noticeも配布する。

## 設定とOS統合

- 変換・学習設定は両OSで同じTOML schemaを使い、既定値と検証はdaemonに集約する。設定用環境変数やOS別schemaを追加しない。
- NixOS・Home Managerは導入optionとTOML生成を共有する。Nixはdaemonと同じキー・構造で明示された項目だけを書き出し、schema、名前変換、既定値、検証を重複定義しない。導入optionは変換設定と分離する。
- packageに同梱する固定資産のpathやsocket名をTOMLの設定項目にしない。n-gram入力訂正のtokenizerは設定で明示し、Nix側で補完しない。
- 利用者設定をフロントエンドのpackageへ埋め込まない。設定のリンク切れやアクセスエラーを、別設定への切り替えで隠さない。

## 実装と検証

- 新しいcrateは、独立した責務と依存境界が必要になってから追加する。共通化だけを目的とした`common`、`shared`、`utils` crateや、実装が存在しない将来用ファイルは作らない。
- Protobuf・FFIの生成物、ビルド成果物、モデルファイルはGitにコミットしない。
- テストは責務を所有するcrate・ディレクトリに置く。ルートの`tests/`は複数の境界を跨ぐテストに限る。
- 辞書・モデルのテストは製品と同じ固定資産を使う。ファイルや依存の存在だけを確認する先行テストではなく、実際の利用境界で検証する。
- 各機能は両OSで、入力、候補生成、選択、確定、必要な永続化、異常時の状態復旧を外部から確認できる最小シナリオで検証する。原作の候補列・内部trace・logitとの厳密比較は、通常の機能検証とは別の適合検証として扱う。
- 開発環境とビルド・テストの入口は、[READMEの開発時のビルド](<README.md#開発時のビルド>)を参照する。

## 文書の責任

- [README.md](<README.md>)：概要、導入、対応状況、原作との差、参照実装、開発時のビルド。
- [usage.md](<docs/usage.md>)：両OSの操作方法と状態ごとの動作の正本。
- [configuration.md](<docs/configuration.md>)：設定項目・既定値・配置の正本。
- [architecture.md](<docs/architecture.md>)：コンポーネント、依存方向、プロセス境界、OS統合の正本。
- 利用方法・対応状況・確定した設計は対応する文書で管理し、このファイルへ転記しない。このファイルは、開発の前提・制約が変わる場合に更新する。
- Markdownは、ユーザーから明示的に依頼された場合を除いて追加または変更しない。
