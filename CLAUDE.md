# nib

WASM プラグインで全機能を構成するモーダルエディタ。標準キーマップ（Helix 風）もプラグインとして実装する。

## 構成

- `core/` — エディタ本体（crate: `nib-editor-core`、ライブラリ名 `nib_core`）。バッファ・選択・描画・プラグインホストだけを持つ
- `tui/` — ターミナルのフロントエンド（crate: `nib-editor`、実行ファイル `nib`。crates.io の `nib` は別の人のもの）。コアは端末に依存しない
- `api/` — プラグイン API の WIT 定義。コアと全 SDK の唯一の正
- `sdk/<lang>/` — 言語別プラグイン SDK
- `plugins/` — 標準プラグイン（`plugins/test/` はテスト用）。公開 API だけで書く（コア内部に依存しない）。wasm32-wasip2 専用の別ワークスペース
- `xtask/` — cargo だけでは書けないビルド手順（`cargo xtask build-plugins`）
- `docs/` — 設計ドキュメント
- `bench/` — 既存エディタと比べる性能計測（`python3 bench/latency.py FILE`）

関連するリポジトリ（`nib-editor` の下。手元では ghq で `~/dev/github.com/nib-editor/` に置く）:

- `nib-editor/plugins` — `nib plugin search` と名前での `nib plugin add` が読む一覧（`plugins.toml`）。登録は PR で受け、CI が形だけを確かめる。
- `nib-editor/plugin-example` — Go で書いたサンプル（`wordcount`）。`v*` のタグを push するとリリースに `.nib.tar.gz` を置く。Go SDK を新しく出したら、ここの `go.mod` も上げてビルドを確かめる。

機能をコアに入れるかプラグインにするか迷ったら、プラグイン側に倒す。コアに入れるのは「プラグインからは実現できない」か「複数のプラグインが共有する基盤で、各プラグインに持たせると重複や性能の問題が出る」もの（例: tree-sitter の解析基盤）だけ。

## コマンド

```sh
cargo xtask build-plugins   # plugins/ を wasm32-wasip2 向けにビルドし target/plugins/ に置く。コアの統合テストが使う
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

プラグインは別ワークスペース（`plugins/Cargo.toml`）なので、fmt、clippy、test は `--manifest-path plugins/Cargo.toml` を付けて別に回す（clippy は `--target wasm32-wasip2`、test はネイティブで動かす）。CI も同じ内容（test は Linux / macOS / Windows）。

`api/wit/` を変えたら `cargo xtask build-plugins` をやり直す。古いプラグインは読み込みで型が合わずに失敗する。コアは写しの `core/wit/` から読むので、そこへも写す（crates.io のクレートが自分の外を読めないため）。Go の SDK のために、`sdk/go/wit/deps/nib-plugin/` へ写して `sdk/go` で `go generate` もする（CI がどちらの写しも一致を確かめる）。Go のテスト用プラグイン（`plugins/test/go`）は TinyGo（と、TinyGo が使う binaryen の `wasm-opt`、`wasm-tools`）があればビルドされ、なければ飛ばされる。

`nib` の実行ファイルは、ビルド時に `target/plugins/` にある標準プラグイン（helix、nano などと `plugins/languages/` の言語）を埋め込む（一覧は `tui/standard-plugins.txt`）。言語を足したら、そこにも足す。crates.io に出すときは `cargo xtask package` で同梱の準備をする（docs/distribution.md）。コアのビルドには cmake が要る（tree-sitter が WASM の文法を読むのに使う wasmtime の C API のため）。wasmtime のバージョンは tree-sitter が使うものにそろえる。プラグインを変えたら `cargo xtask build-plugins` のあとで `nib` をビルドし直す。

`target/debug` は、版や依存を変えるたびに古い成果物が残って膨らむ（一度 165 GB になった）。大きくなっていたら `cargo clean --profile dev` で消す（release のビルドと `target/plugins/` は残る）。

## ライセンス

- リポジトリ全体は `MIT OR Apache-2.0`。
- Helix（MPL-2.0）からコードを流用したファイルは MPL-2.0 のまま残す。元の著作権表示を保持し、`THIRD_PARTY.md` に upstream のパスとコミットを記録する。
- `api/` と `sdk/` には MPL コードを入れない。プラグイン作者のライセンス選択を縛らないため。

## バージョン

- エディタ本体は workspace の `version` で管理する。
- **1.0.0 までは、変更のたびには上げない**（グローバルの「変更に応じて上げる」より優先する）。リリースしないので版は外から見えず、上げるたびに `target/debug` に古い成果物が 1 世代まるごと残る（cargo は版をビルドの識別に含める）。1.0.0 を出したあとは、変更に応じて細かく上げる。
- SDK はエディタとは別にバージョンを付ける。`api/` が変わらない限り SDK のバージョンは上げない。例外は、Go のモジュールのパスが変わったときのように、打ち直さないと SDK を取れなくなるとき（パッチだけ上げる。`nib-editor` に移したときの `sdk/go/v0.4.1`）。
- `api/wit/` を変えたら、同じコミットで WIT のパッケージのバージョン（`nib:plugin@X.Y.Z`）も SDK と同じ段だけ上げ、`メジャー.マイナー` が変わったら `core` の `API_VERSION` と全プラグインの `plugin.toml` の `api` も合わせる。プラグインの `api` が nib と違えば読み込まない。
- Go SDK は `sdk/go/` に独自の `go.mod` を置き、タグは `sdk/go/vX.Y.Z` 形式にする。
- Rust SDK は crates.io に出さず、タグ `sdk/rust/vX.Y.Z` で取らせる。SDK のバージョンを上げたら、そのタグを打ち、`nib plugin new` の雛形が指すバージョン（`tui/src/scaffold.rs` の `RUST_SDK_TAG` と `GO_SDK_VERSION`）も直す。`RUST_SDK_TAG` はテストが `sdk/rust/Cargo.toml` と比べる。

## 進め方

設計ドキュメント（`docs/`）を先に書き、それに沿って実装する。実装中に設計が変わったら、同じコミットで docs も更新する。

`docs/` は日本語で書く。README・CONTRIBUTING・コード内のコメントと識別子は英語。
