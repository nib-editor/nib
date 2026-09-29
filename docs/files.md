# ファイルを開く道具とディレクトリの一覧

> ステータス: 合意済み（2026-09-29）。nib 0.12.1 で files プラグインとして入れ、0.13.0 で picker にまとめ、netrw 風と dired 風を選べるようにした（2026-09-30）。

`nib <ディレクトリ>` で開けるようにし、ディレクトリの中身を眺めてたどる一覧（vim の netrw、Emacs の dired に当たるもの）を入れる。ファイルを開く道具は、どれも picker プラグインにまとめる。あわせて、設定のファイルを作りやすくする。

## picker のコマンド

| コマンド | キー（helix、vim） | すること |
|----------|--------------------|----------|
| `picker.files` | `Space f` | 下のファイルを名前で探す。.gitignore されたものと隠しファイルは除く |
| `picker.all-files` | `Space e` | 同じ箱で、隠しファイルも .gitignore されたものも含めて探す |
| `picker.commands` | `Space ?` | コマンドを名前と説明で探す |
| `picker.directory` | （なし） | ディレクトリの一覧をバッファに出す。`nib <ディレクトリ>` と `:config-dir` が開く |

- キーはリーダーの下（Emacs では `C-c f`、`C-c e`、`C-c ?`）。
- `files` と `all-files` は `{"path": "<dir>"}` を受け取り、その下を探す。なければ作業ディレクトリ。箱の見た目と重さは [finder.md](finder.md)。
- `all-files` を `files` と分けるのは、`target/` や `node_modules/`、`.git/` まで入ると候補が何十万にもなるため。いつもは軽く、要るときだけ全部。
- 0.12 までは、一覧を別の files プラグインが持っていた。探すものと一覧は同じ「ファイルを開く」道具で、箱と絞り込みのコードも一緒に使うので、picker にまとめた。

## ディレクトリを開く

`nib <ディレクトリ>` と、ディレクトリを渡されたときに何をするかは、`config.toml` の `[core]` に、呼ぶコマンドとして書く。

```toml
[core]
# A directory given to nib runs this command with {"path": "<dir>"}:
# "picker.directory" lists it, and "picker.files" picks a file under it.
open-directory = "picker.directory"
```

- コアは、ディレクトリを受け取ったら、このコマンドに `{"path": "<絶対パス>"}` を渡して呼ぶだけで、どのプラグインかは知らない。自作のプラグインのコマンドも書ける。
- 作業ディレクトリは変えない。
- そのコマンドがない（プラグインが無効など）ときは、「open-directory: no command named picker.directory」のように知らせ、空のバッファで始める。
- ファイルとディレクトリを混ぜて渡したら、渡した順に開く。

### 設定のディレクトリ

- コアのコマンド `config.open-directory` で、設定のディレクトリ（`~/.config/nib/`）を、上の `open-directory` のコマンドで開く。
- helix と vim のコマンドラインでは `:config-dir`、コアメニューでは「Open the settings directory」。
- 一覧で開けば、そこから `plugins/` に入り、各プラグインの設定のファイルを開ける。

## ディレクトリの一覧（`picker.directory`）

1 つのディレクトリの中身を、picker のバッファ（[plugin-buffers.md](plugin-buffers.md)）に並べる。netrw 風と dired 風があり、見た目とキーが違う。`plugins/picker.toml` で選ぶ。

```toml
[settings]
# "netrw" or "dired"; without it, netrw with the vim base and dired with
# the others.
directory-style = "netrw"
```

### netrw 風

```
" nib directory listing
"   /Users/q0tzly/.config/nib/
"   <ret>:open  -:up  %:new file  d:new directory  q:close
../
plugins/
config.toml
```

| キー | すること |
|------|----------|
| `Enter` | 開く。ディレクトリなら中に入る |
| `-` | 上のディレクトリへ |
| `%` | 名前を聞いて、新しいファイルを開く |
| `d` | 名前を聞いて、ディレクトリを作る |
| `q` | 一覧を閉じる |

- vim の netrw の見出しと並びに似せる（`"` で始まる見出し、名前だけ、ディレクトリに `/`）。見出しの文言は nib のもの。
- vim ベースは `%`（対応する括弧へ）を、プラグインのバッファに渡すキーにする（[vim.md](vim.md) の「プラグインのバッファのキー」）。一覧では括弧をたどることがなく、netrw の `%` を使えるようにするため。

### dired 風

```
  /Users/q0tzly/.config/nib:
  drwxr-xr-x     -  09-28 14:02  ./
  drwxr-xr-x     -  09-28 14:02  ../
  drwxr-xr-x     -  09-28 14:02  plugins/
  -rw-r--r--  2.1K  09-28 14:02  config.toml
```

| キー | すること |
|------|----------|
| `Enter` | 開く。ディレクトリなら中に入る |
| `^` | 上のディレクトリへ |
| `+` | 名前を聞いて、ディレクトリを作る |
| `g` | 読み直す |
| `q` | 一覧を閉じる |

- Emacs の dired の `ls -l` の列に似せる。権限、大きさ（`812`、`2.1K`）、更新の日時（半年より前なら年）、名前。所有者とリンクの数は出さない。
- 新しいファイルは、ベースのいつもの開き方で作る（Emacs の `C-x C-f`、helix と vim の `:e`）。dired にファイルを作るキーはないため。
- helix と vim は `g` と `^` を手放さない（`^` は vim だけ）ので、そのベースで dired 風を選ぶと、そのキーはベースのものになる。既定はベースに合わせてあり、困るのは設定で変えたときだけ。

### どちらにも共通

- ディレクトリ、ファイルの順で、それぞれ名前の順。隠しファイルも出す。
- バッファの名前はディレクトリの絶対パス（末尾に `/`）。ディレクトリごとに 1 つのバッファ。中に入る、上に戻るときは、同じビューに新しいディレクトリのバッファを出し、前の一覧は閉じる（一覧のバッファが積もらないように）。ファイルを開くと、一覧のバッファは残る。
- 引数がなければ、表示中のファイルのディレクトリを開き、カーソルをそのファイルの行に置く。ファイルでなければ作業ディレクトリを開く（Emacs の `dired-jump`、vim の `:Explore`）。
- ディレクトリの名前は `ui.directory` の色（テーマで変えられる）。
- 書き換えはできない（持ち主が `set-editable` しない）。名前の変更や削除は、あとで足す。
- 動き、検索、コピーは、ふつうのバッファと同じく、ベースのキーで効く。
- 新しいファイルは、保存したときに作られる。保存するとき、コアは、ないディレクトリも作る。

### コアに足すもの（API）

```wit
interface files {
    record walk-options { hidden: bool, ignored: bool }
    /// As `walk`, with hidden files and ignored ones when asked.
    walk-with: func(dir: option<string>, options: walk-options) -> result<u64, string>;

    record dir-entry {
        name: string,
        directory: bool,
        size: u64,
        /// The Unix permission bits, such as 0o755; none elsewhere.
        mode: option<u32>,
        /// When it last changed, in seconds since 1970.
        modified: option<u64>,
    }
    %list: func(dir: string) -> result<list<dir-entry>, string>;
    /// Makes a directory and those above it. Needs "fs-write".
    make-dir: func(path: string) -> result<_, string>;
}

interface editor {
    record date-time { year: u32, month: u8, day: u8, hour: u8, minute: u8 }
    /// A time, in seconds since 1970, in the time zone nib runs in.
    local-time: func(seconds: u64) -> date-time;
}
```

- `list` と `walk-with` は `fs-read` の権限で、どのディレクトリも読める（名前と大きさと日時だけで、中身は読めない）。`walk-with` でも `.git` の中は数えない。`make-dir` は `fs-write`。picker は両方を持つ。
- `local-time` を足すのは、WASI からは地域の時刻が分からないため。コアが Unix では `localtime_r` で直す。Windows では UTC を返す。
- `dir-entry` の形を変えるのは型の変更だが、`list` を足した 0.7.1 と 0.7.2 は公開していない（タグを push していない）。使っていたのは files プラグインだけで、それも picker にまとめた。
- `nib:plugin@0.7.3`、SDK は 0.7.3。マニフェストの `api` は `"0.7"` のまま。

## 設定のファイルを作りやすくする

プラグインの設定のファイル（`~/.config/nib/plugins/<name>.toml`）は、標準プラグインのものも含めて、置けば効く。コアメニューの「Open its settings」や `:config <name>` で開けば、なければ雛形の入った状態で開き、保存すると作られる。0.12.1 で次の 3 つを直した。

1. **雛形の `[settings]` が空だった。** 読み込み方（`enabled`、`timeout-ms` など）はコメントで入るが、そのプラグイン独自の設定（vim の `leader`、lsp の `servers` など）は、文書を読まないと分からなかった。
   - プラグインは、自分の設定の例を `settings.example.toml` としてプラグインのディレクトリに置ける。中身は、`[settings]` の下に入る行を、コメントにしたもの。
   - コアは、雛形の `[settings]` の下にそれを差し込む。標準プラグインは全部置く。`nib plugin pack` も同梱する。
2. **雛形が入るのは `config.open` で開いたときだけだった。** 設定のディレクトリの `config.toml` と `plugins/<name>.toml` を、どう開いても（一覧の新しいファイル、`:e`、`buffer.open`）、ファイルがなければ雛形を入れる（保存はしない）。
3. **`nib config init` は config.toml と helix.toml しか作らなかった。** 使っているベース（`config.toml` の `base`、なければ helix）の分を作る。

## 版

- 0.12.1: files プラグイン、`open-directory`、設定のファイルの雛形。`nib:plugin@0.7.1`。
- 0.13.0: files プラグインを picker（0.3.0）にまとめ、`open-directory` の既定を `picker.directory` に変えた（`files.open` と書いた設定は効かなくなるので、minor を上げる）。`nib:plugin@0.7.3`。

## 入れないもの

- 横に細く出すファイルの木（サイドバー）。分割の幅を決める API などが要る。
- 一覧の中での名前の変更、削除、コピー、印を付けてまとめて操作すること（dired の `R`、`D`、`m`、netrw の `R`、`D`）。
- 所有者、リンクの数、並べ替えの切り替え。
