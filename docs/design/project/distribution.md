# nib の配布

> ステータス: 合意済み（2026-09-28）。仕組みは用意したが、リリースは 1.0.0 から出す。それまでは、ソースから入れる（README の「Installing」）。tap と bucket のリポジトリは作ってあり、リリースがあるまでは何もしない。mise の一覧への PR は、広く使われてから（下の「入れ方」）。

`nib` の実行ファイルを、mise、Homebrew、Scoop、cargo から入れられるようにする。プラグインの配布（[plugin-install.md](../api/plugin-install.md)）とは別の話。

## 土台: GitHub のリリース

`v*` のタグ（`v0.9.9` など）を push すると、ワークフロー（`.github/workflows/release.yml`）が各 OS 向けに `nib` をビルドし、GitHub のリリースに置く。ほかの配り方は、どれもこのリリースを指す。

| ターゲット | 置くもの |
|------------|----------|
| `aarch64-apple-darwin` | `nib-v0.9.9-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `nib-v0.9.9-x86_64-apple-darwin.tar.gz` |
| `x86_64-unknown-linux-gnu` | `nib-v0.9.9-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `nib-v0.9.9-aarch64-unknown-linux-gnu.tar.gz` |
| `x86_64-pc-windows-msvc` | `nib-v0.9.9-x86_64-pc-windows-msvc.zip` |

- アーカイブの中身は `nib`（Windows は `nib.exe`）と `LICENSE-*`、`README.md`。標準プラグインは実行ファイルに埋め込んである。
- それぞれに `.sha256` を並べて置く。
- 名前に Rust のターゲットをそのまま使うのは、mise や cargo-binstall が OS と CPU を名前から読み取るため。
- Linux は、古い glibc でも動くように、なるべく古い Ubuntu の runner でビルドする。
- タグと workspace の `version` が違えば、ビルドの前に止める。
- 手で動かす（`workflow_dispatch`）と、すべてのターゲットのビルドとアーカイブ作りまでを行い、リリースも公開もしない。タグを打つ前に試すため。

## 入れ方

| 先 | 入れ方 | 置き場 |
|----|--------|--------|
| mise | `mise use github:nib-editor/nib`（一覧に載ったら `mise use nib`） | リリースをそのまま使う |
| Homebrew | `brew install nib-editor/tap/nib` | tap のリポジトリ `nib-editor/homebrew-tap` の formula |
| Scoop | `scoop bucket add nib-editor https://github.com/nib-editor/scoop-bucket` のあと `scoop install nib` | bucket のリポジトリ `nib-editor/scoop-bucket` の manifest |
| cargo | `cargo install nib-editor`、または `cargo binstall nib-editor` | crates.io の `nib-editor` と `nib-editor-core` |

- 名前だけで入れられる公式の一覧（mise の registry、homebrew-core、Scoop の main / extras）には、審査がある。mise の一覧は、新しく載せるものに「すでに広く使われている（GitHub のスターがふつうは数千）」ことを求め、版を表示させて照らし合わせる確認（`mise test-tool`）もする（`nib --version` は足してある）。条件を満たしたときに PR を出す。homebrew-core と Scoop の公式の bucket は、知名度の条件を満たしてから申請する。それまでは、tap と bucket を一度登録すれば名前で入る。
- tap と bucket の中身は、それぞれのリポジトリのワークフローが、nib の最新のリリースを見て毎日更新する（手でも動かせる）。nib のリポジトリからほかのリポジトリへ書き込むための鍵を持たずに済む。Scoop の manifest は、Scoop の `checkver` / `autoupdate` の書き方に従う。
- Homebrew の formula は、ビルド済みの実行ファイルを入れる（tap なので、ソースからのビルドにしなくてよい）。

## crates.io

- パッケージの名前は `nib-editor`（`nib` は別の人のクレートが使っている）。入る実行ファイルは `nib`。コアも `nib-editor-core` として公開する（`nib-core` だと、別の人の `nib` の一部に見えるため）。ライブラリの名前は `nib_core` のままで、コードは変わらない。
- crates.io からソースでビルドするときは、標準プラグインを作る手順（`cargo xtask build-plugins`）が走らない。公開するクレートに、ビルド済みの標準プラグイン（WASM、合わせて約 5 MB）を同梱する。
  - `cargo xtask package` が、`target/plugins/` の標準プラグインを `tui/plugins/` に写す（git には入れない）。`tui/build.rs` は、`target/plugins/` がなければ `tui/plugins/` から埋め込む。ライセンスのファイルも、両方のクレートの隣に写す。
- コアは WIT を `api/wit/` から読んでいたが、crates.io のクレートは自分の外を読めない。Go の SDK と同じく写しを `core/wit/` に置き、`api/wit/` と同じであることを CI で確かめる。
- 公開した版は消せない（使わない印を付けられるだけ）。最初の公開は持ち主が手で行い、以降はリリースのワークフローが crates.io の Trusted Publishing で公開する。
- `nib-editor` の `[package.metadata.binstall]` に、リリースのアーカイブの場所を書く。`cargo binstall nib-editor` はソースからビルドせずに、それを取る。

## 版

- 実行ファイルの版は workspace の `version`。リリースのタグはそれに `v` を付けたもの。
- `nib-editor` が使う `nib-editor-core` の版は、workspace の `version` と同じにそろえる（`[workspace.dependencies]` に書く）。
