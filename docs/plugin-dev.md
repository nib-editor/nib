# プラグインの開発

> ステータス: 合意済み（2026-09-27）

M5「AI で作れる土台」の設計。プラグインを作り始める、試す、直す、配る、の繰り返しを、人にも AI にも短く回せるようにする（[vision.md](vision.md) の「位置づけ」）。

AI がプラグインを作るときに要るのは、次の 3 つ。

1. **始め方**: 動く雛形。ビルドとテストの手順が、雛形の中に書いてある。
2. **確かめ方**: 端末を開かずに、キーを送って結果を確かめる手段。AI は画面を見られないので、これがないと「書いたが動くか分からない」で止まる。
3. **API の説明**: WIT の型だけでは分からない決まり（入力スタック、再入しないこと、権限、時間の上限など）を、1 か所で読めること。

## 流れ

```sh
nib plugin new wordcount          # 雛形を作る（Rust。--go で Go）
cd wordcount
nib plugin build                  # plugin.wasm を作る
nib plugin test                   # tests/*.toml を端末なしで実行する
nib --plugin .                    # 実際に動かして見る
git tag v0.1.0 && git push --tags # リリースを作る（雛形のワークフローが .nib.tar.gz を置く）
```

## `nib plugin new NAME [--go] [DIR]`

`DIR`（省略すると `./NAME`）に雛形を作る。`NAME` はマニフェストの `name` の決まり（英小文字、数字、`-`）に従う。すでにあるディレクトリには書かない。

Rust（既定）:

```
wordcount/
├── plugin.toml
├── Cargo.toml          # nib-plugin を git のタグで取る。crate-type = "cdylib"
├── src/lib.rs          # コマンドを 1 つ登録し、呼ばれたらステータスに出すだけのプラグイン
├── tests/wordcount.toml
├── README.md           # ビルド、テスト、試し方、リリース
├── AGENTS.md           # AI 向け。どこを読み、何で確かめ、何を守るか
├── .gitignore          # target/、plugin.wasm、*.nib.tar.gz
└── .github/workflows/release.yml
```

Go（`--go`）は `Cargo.toml` と `src/` の代わりに `go.mod` と `main.go` を置く。中身は [nib-editor/plugin-example](https://github.com/nib-editor/plugin-example) と同じ形にする。

- 雛形のプラグインは、`nib plugin test` がそのまま通る状態で作る。最初の 1 回で「ビルドとテストが回る」ことを確かめられるようにするため。
- Rust の SDK は crates.io に出さず、git のタグ `sdk/rust/vX.Y.Z` で取る（Go の SDK の `sdk/go/vX.Y.Z` と同じ）。雛形は、その nib が対応する SDK のタグを書く。
- `AGENTS.md` は、AI のコーディング支援が読むファイル名の慣習に合わせる。中身は、API の説明（下の「API の文書」）と WIT の場所、`nib plugin build` と `nib plugin test` で確かめること、守ること（権限はマニフェストで宣言する、1 回の呼び出しは短く、など）。

## `nib plugin build [DIR]`

プラグインをビルドして、`DIR`（省略するとカレント）に `plugin.wasm` を置く。

- `Cargo.toml` があれば `cargo build --release --target wasm32-wasip2` を実行し、できた `.wasm` を写す。
- `go.mod` があれば TinyGo で作る（[plugin-api.md](plugin-api.md) の「SDK」のコマンド）。
- 道具がなければ、何を入れればよいかを言って失敗する（`rustup target add wasm32-wasip2`、TinyGo など）。

ビルドのコマンドを覚えなくてよいようにするための薄い包み。中で何を実行したかは表示する。

## `nib plugin test [DIR] [FILE]...`

`DIR`（省略するとカレント）のプラグインを読み込んだ nib を端末なしで動かし、`DIR/tests/*.toml`（または `FILE`）に書いたテストを実行する。

- 標準プラグインのうち、`lsp` 以外を読み込む（`lsp` は外部のプロセスを起動するため）。テストするプラグインと同じ名前の標準プラグインがあれば、差し替える。ユーザーの設定とインストールしたプラグインは使わない。どこで実行しても同じ結果になるようにするため。
- テストごとに新しいエディタを作る。前のテストの状態を引きずらない。
- 結果は 1 テスト 1 行で出し、失敗したものは期待と実際を並べる。1 つでも失敗すれば終了コードは 1。

### テストの書き方

```toml
# 読み込む標準プラグインを絞る（省略すると lsp 以外の全部）
with = ["helix"]

[[test]]
name = "counts the words of the buffer"
file = "notes.md"                 # バッファのファイル名。拡張子で言語が決まる。省略すると test.txt
text = "#[o|]#ne two three\n"     # バッファの中身と選択（下の「選択の書き方」）
keys = ":wordcount<ret>"          # 送るキー（下の「キーの書き方」）

[test.expect]
message = "3 words"               # メッセージ行
screen = ["3 words"]              # 画面のどこかの行に含まれていること

[[test]]
name = "answers the command"
text = "one two\n"
command = "wordcount.count"       # キーのあとに呼ぶコマンド
args = "{}"                       # コマンドの引数（JSON）。省略すると {}

[test.expect]
result = "2"                      # コマンドの戻り値（JSON）
```

`expect` に書けるもの（書いたものだけを確かめる）:

| キー | 確かめること |
|------|--------------|
| `text` | バッファの中身（選択の印なし） |
| `selections` | バッファの中身と選択（印つき） |
| `message` | メッセージ行（完全一致） |
| `screen` | 各文字列が、画面（80 × 24）のどこかの行に含まれること |
| `absent` | 各文字列が、画面のどの行にも含まれないこと |
| `result` | `command` の戻り値（JSON として比べる） |
| `error` | `command` が失敗すること、そのメッセージに含まれる文字列 |

キーを送るたびに、実際の nib と同じく、描画のあとの処理（イベントの配送、構文木の更新）を済ませてから次のキーを送る。

### 選択の書き方

Helix のテストと同じ印を使う。`#[` と `]#` で主選択、`#(` と `)#` でほかの選択を囲み、`|` がカーソル（`head`）の位置を表す。

```
#[h|]#ello        h の上のカーソル（anchor 0、head 1）
#[|hello]# world  hello を後ろ向きに選択（anchor 5、head 0）
#[a|]#b #(c|)#d   2 つのカーソル。主選択は a
```

`text` に印がなければ、カーソルは先頭の 1 文字の上に置く。

### キーの書き方

文字はそのまま、名前のあるキーと修飾つきのキーは `<` と `>` で囲む。中の書き方は設定のキー（[keymap.md](keymap.md) の「設定」）と同じ。

```
ihello<esc>       i、h、e、l、l、o、Esc
<C-w>v            Ctrl-w、v
<A-o><space>f     Alt-o、Space、f
<lt>              < そのもの
<A-gt>            Alt-> （< と > は、< > の中では lt と gt と書く）
```

### 待つこと、手順、設定

タイマー、外部プロセス、LSP のように、時間がたってから結果が出るものは `wait` で待つ。

```toml
[[test]]
name = "shows diagnostics from the server"
file = "main.rs"
text = "fn main() { error }\n"
# テストするプラグインの [settings]（plugins/<name>.toml に書くもの）
settings = { servers.rust.command = ["python3", "{dir}/fake_server.py"] }

[[test.step]]
command = "lsp.status"
wait = 5000                       # 最大 5 秒、expect が満たされるまでエディタを回す
[test.step.expect]
result = "rust ready"

[[test.step]]
keys = "<space>k"
wait = 2000
[test.step.expect]
screen = ["found error"]
```

- `wait`（ミリ秒）: キーやコマンドのあと、その時間まで実際にエディタを回す。タイマーを発火させ、外部プロセスの出力やファイルの一覧を渡し、描画のあとの処理を済ませる。`expect` がすべて満たされた時点で次に進み、時間が来ても満たされなければ失敗にする。`expect` がなければ、その時間だけ回す（タイマーを進めるのに使う）。
  - 時間は実際の時間。タイマーの 100 ms を待つテストは 100 ms かかる。
  - 同じ手順に `command` があり、`expect` に `result` か `error` があれば、満たされるまでコマンドを呼び直す（`lsp.status` のように、状態を問い合わせるコマンドのため）。呼ぶたびに何かを変えるコマンドでは、`wait` を別の手順に分ける。
- `[[test.step]]`: 手順を順に並べる。1 つの手順に `keys`、`paste`、`command`、`args`、`wait`、`expect` を書ける。キーを送り、`paste` の文字を端末に貼り付けたものとして 1 つの塊で届け、コマンドを呼び、待ってから確かめる。`step` を使わずに `[[test]]` に直接書いたものは、手順が 1 つのテストとして扱う（`text` と `file` と `settings` は `[[test]]` に書く）。
  - 手順の `expect` が満たされなければ、そこでテストを失敗にする。
- `settings`: テストするプラグインの `[settings]`。`init` に JSON で渡る。文字列の中の `{dir}` は、テストのファイルがあるディレクトリに置き換える（偽のサーバーのように、テストと一緒に置くファイルを指すため）。ファイルの先頭（`with` と同じ場所）に書くと、そのファイルのテスト全部の既定になる。
- `files`: 作業ディレクトリに置くファイル（`{ "src/main.rs" = "fn main() {}\n" }`）。
- テストごとに、作業ディレクトリを新しく作った一時ディレクトリにする。中にあるのは、テストのファイル（`file`）と `files` だけ。ファイルを並べたり読んだりするプラグイン（picker など）が、どこで実行しても同じ結果になるように。
- `expect.absent`: 各文字列が、画面のどの行にも含まれないこと（診断が消えたことなどを確かめる）。
- `lsp` は、`with` に書いたときだけ読み込む（テストするのが `lsp` 自身なら、いつも読み込む）。

偽のサーバーは、どの言語で書いてもよい。標準の `lsp` プラグインのテスト（`plugins/lsp/tests/`）は、Python で書いた偽のサーバーを使う。`python3` のない環境（Windows の CI など）では、このテストを飛ばす。

## `nib plugin pack` が入れるもの

ディレクトリの中身を全部入れると、Rust の雛形では `target/` や `src/` まで入ってしまうので、nib が読むものだけにする: `plugin.toml`、`plugin.wasm`、マニフェストの `[[languages]]` が指す文法とクエリ、`LICENSE*`。

## API の文書

プラグインを書く人（と AI）向けの説明を `sdk/README.md` に英語で書く。リポジトリの外の人が読むもので、README と同じ扱いにする（`docs/` は設計の記録で日本語）。

- WIT（`api/wit/plugin.wit`）が型と関数の唯一の正で、コメントも英語で書いてある。`sdk/README.md` は、WIT だけでは分からない決まりと、よくある作り方を書く。関数を一つずつ並べ直すことはしない。
- 載せること: プラグインの形（マニフェスト、`init` と 3 つの入口）、呼び出しの決まり（再入しない、時間の上限、止まったときの片付けと再起動）、入力スタックとキーの流れ、コマンドとイベント、権限、よくある作り方（キーマップの層、コマンドの登録、ステータスの表示、構文木のクエリ、タイマー）、ビルド・テスト・配布。
- `nib plugin new` の雛形の `AGENTS.md` と `README.md` は、ここを指す。

## 作る順番

1. キーの並びの記法と、選択の印の読み書き（コア。テスト以外でも使える）
2. `nib plugin test`
3. `nib plugin pack` が入れるものを絞る
4. `nib plugin build`
5. `nib plugin new`（Rust と Go）と `sdk/README.md`
6. Rust の SDK のタグ `sdk/rust/v0.4.2` を打ち、雛形から実際にビルドとテストが通ることを確かめる
