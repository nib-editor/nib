# プラグイン API 1.0 の見直し

> ステータス: 提案（2026-09-28）。合意待ち。

1.0.0 の条件の 1 つ目「プラグイン API を固める」（[roadmap.md](roadmap.md) の「1.0.0」）のために、WIT（`api/wit/plugin.wit`、今は `nib:plugin@0.5.3`）を見直し、1.0 までに足すもの・変えるものを決める。1.x のあいだは、足すことはあっても変えたり消したりしない。

材料は、helix、nano、vim、Emacs の 4 つのベースと、lsp、picker、indent を公開 API だけで書いて困ったこと。

## 1.x で何ができて、何ができないか

見直しの前に、1.x のあいだに API をどこまで広げられるかを決める。コンポーネントモデルは、import と export の型をそのまま突き合わせるので、次のように分かれる。

| 変更 | 古いプラグインは動くか |
|------|------------------------|
| interface に関数を足す、resource にメソッドを足す | 動く（使わない import は求められない） |
| world に interface を足す | 動く |
| record にフィールドを、variant と enum に場合を、flags に旗を足す | 動かない（型が変わり、読み込みで合わなくなる） |
| 関数の引数や戻り値の型を変える | 動かない |

- つまり、**型の形は 1.0 で決めたものが 1.x の終わりまで続く。** 足せるのは関数と interface だけ。
- 特に `event`（guest の `on-event` の引数）は variant なので、1.x でイベントの種類を足せない。1.x で足すイベントは、今の `editor.*` と同じく custom イベントの形（名前と JSON）で出す。これを正式な仕組みにする（下の「イベント」）。
- 戻り値の `editor.error` も variant なので、1.x で失敗の種類を足せない。`other(string)` を足しておき、新しい失敗はそこに入れる。
- パッケージの版（`nib:plugin@1.0.0`）は import の名前に入る。1.1 のコアが 1.0 のプラグインを読めるように、wasmtime の Linker が semver の互換で名前を合わせることを、実装の最初に確かめる。合わせない場合は、コアが 1.0 から 1.x までの名前で同じ関数を並べて定義する。
- マニフェストの `api` は、今は `メジャー.マイナー` が完全に一致しないと読み込まない。1.x では「メジャーが同じで、マイナーがコア以下」なら読み込むようにする。

## 足すもの

### バッファが変更済みか（`buffer.modified`）

- 今: 分からない。Emacs の `C-x C-c`、nano の `^X`、vim の `:q` は、`editor.quit` を呼んで失敗したかで見ている。Emacs の `C-x s` は、変更のあるバッファを探せないので、今のバッファだけを保存している。
- 案: `buffer.modified: func() -> bool`。コアはもう、保存したときの undo の状態と今の状態を比べて持っている（`editor.quit` が断るのに使っている）。それを出すだけ。undo で保存したときの内容に戻れば false になる。

### どのバッファでも保存し、表示する（`buffer.save`、`view.show`）

- 今: 保存はコマンドの `buffer.save` で、表示中のバッファしか保存できない。表示の切り替えは `buffer.open`（パス）で、パスのないバッファ（新しいバッファ）には切り替えられない。
- 案:
  - `buffer.save: func(path: option<string>) -> result<_, string>`。そのバッファを保存する。`path` を渡すと、そこへ保存して以後そのパスにする。
  - `view.show: func(buf: borrow<buffer>)`。そのビューにバッファを表示する。
  - コマンドの `buffer.save` は残す（キーや `:` から呼ぶため）。中で同じものを使う。

### 入力欄の見出しを変える（`line.set-label`）

- 今: 見出しは作るときの 1 回だけ。Emacs の isearch は「Failing I-search」「Wrapped」「(incomplete input)」に変えるたびに、欄を作り直している。
- 案: `line.set-label: func(label: string)`。

### 検索のグループ（`buffer.find-groups`）

- 今: `find` は一致した範囲しか返さないので、vim の `:s/\(a\)/\1/` と Emacs の置き換えの `\1` が使えない（vim.md と emacs.md の「違い」に書いてある）。
- 案: `buffer.find-groups: func(pattern: string, start: offset, backward: bool) -> result<option<list<option<tuple<offset, offset>>>>, error>`。0 番が一致の全体、1 番からがグループ。参加しなかったグループは `none`。今の `find` と `find-all` は残す（使う側の多くは範囲だけでよい）。

### undo と redo の位置

- 今: `undo` と `redo` は、したかどうかの bool だけ。どこが変わったかは `buffer-changed` イベントで分かるが、呼び出しのあとに届くので、ポイントをすぐ置き直せない。Emacs の redo のあとのポイントが本物と違うのはこのため（emacs.md の「違い」）。
- 案: 戻り値を `option<tuple<offset, offset>>` にする。変わったところ（変更後のテキストでの、いちばん前の変更の範囲）。何もなければ `none`。型が変わるので、1.0 で変えるしかない。

### 文字を打っている状態か（モードの知らせ）

- 今: lsp は、打鍵が止まったときに補完を出すかどうかを、`helix.mode_changed` イベントで決めている。vim のベースは `vim.mode_changed` を出し、Emacs と nano はモードがないので何も出さない。そのため、**helix 以外のベースでは補完が自動で出ない。** ステータスラインにモードを出すプラグインも、ベースごとの名前を知らないと書けない。
- 案: ベースが今の状態をコアに伝え、コアがどのプラグインにも同じ形で知らせる。
  - `input.set-mode: func(name: string, typing: bool)`。`name` はベースが決める（`normal`、`insert`、`visual` など。Emacs と nano は `global`）。`typing` は、修飾のない文字がテキストに入る状態か。
  - イベント `editor.mode`（下の「イベント」）: `{"base": "vim", "name": "insert", "typing": true}`。lsp は `typing` を見る。
  - モードのないベースは `init` で一度 `set-mode("global", true)` を呼ぶ。
  - `input.mode: func() -> tuple<string, bool>` で今の状態も取れるようにする（途中で有効にしたプラグインのため）。

### 貼り付け

- 今: 何もしていない。端末の bracketed paste を有効にしていないので、貼り付けた文字が 1 つずつキーとして届く。vim の normal モードでは貼った文字がコマンドとして動き、Emacs では改行のたびに前の行のインデントが足される（`electric-indent-mode` の分）。貼り付けが大きいと、1 文字ごとにベースを呼ぶので遅い。
- 案:
  - tui で bracketed paste を有効にし、貼り付けを 1 つの塊として受ける。
  - ベースに届ける。キーではないので `handle-key` には入れず、guest に `handle-paste: func(text: string) -> key-result` を足す。入力スタックを上から順に回るのはキーと同じ。
  - ベースがどう入れるかを決める（vim の normal モードでは、`p` のように入れるか何もしないか。Emacs はポイントに入れる。入力欄が開いていれば欄に入れる）。`pass` が返ってきたら、コアが表示中のバッファのカーソルに入れる。
  - guest に関数を足すのは、1.x で足せない（古いプラグインが export していない関数を、コアが求めることになる）。今足す。

### 失敗の種類の余地（`error.other`）

- 案: `editor.error` に `other(string)` を足す。1.x で新しい失敗を、型を変えずに返すため。

### キーの種類の余地

- 今: `key-code` に Insert がない（端末から届いても捨てている）。`modifiers` は ctrl、alt、shift、super。
- 案: `insert` を足す。kitty keyboard protocol の hyper と meta は、使う場面がないので足さない（1.x で足せないことを承知のうえで）。キーを離したイベント（「保留中のアイデア」の DOOM）も、今は入れない。入れるときは、`handle-key` とは別の関数と interface で足す。

### 一覧の操作の意味

- 今: `prompt.action` は accept、cancel、next、previous、complete、complete-back。
- 案: `page-next`、`page-previous`（一覧を 1 画面送る。picker の PageDown、Emacs の `C-v`、vim の `C-f`）を足す。enum なので今足す。先頭と末尾（`first` `last`）は、どのエディタでもキーが決まっていないので足さない。

## 揃えるもの

### イベント

- `event` の種類は、1.0 で決めたものが最後になる（上の「1.x で何ができて、何ができないか」）。今の種類（`buffer-opened` から `prompt-action` まで）はそのまま残す。
- コアが出すイベントのうち、WIT の種類にないものは、custom イベントの形で出す。名前は `editor.` で始め、中身の JSON の形を plugin-api.md に書き、1.x のあいだ変えない。
  - 1.0 では `editor.buffer-closed`、`editor.syntax-updated`、`editor.mode` の 3 つ。
- 名前の書き方を揃える。WIT の種類は `buffer-opened` のようにハイフンなのに、コアの custom イベントは `editor.buffer_closed`、ベースの `mode_changed` は下線。ハイフンに揃える（`editor.buffer_closed` → `editor.buffer-closed`、`editor.syntax_updated` → `editor.syntax-updated`）。プラグインが出す custom イベントの名前は、プラグインが決めるので縛らないが、plugin-api.md でハイフンを勧める。
- `helix.mode_changed` と `vim.mode_changed` は、`editor.mode` に替えて出すのをやめる。

### コマンド

- 引数の空文字列は `{}` と同じにする（今もコアは同じに扱っているが、書いていない）。
- 予約する名前の一覧が 2 か所で違う（コマンドの節では `buffer.`、`editor.`、`view.`、マニフェストの節では `buffer`、`config`、`core`、`editor`、`view`）。後者に揃える。

### 名前と形

- `commands.call` の戻り値、`settings.get` の値は、どちらも JSON の文字列のまま（JSON を選んだ理由は plugin-api.md の「コマンド」）。
- 位置はバイトオフセット、画面の座標は出さない、という方針は変えない。

## 入れないもの

1.x で関数や interface として足せるので、今は入れない。

- プラグインが持つ特別なバッファ（magit や dired のようなもの）。新しい interface で足す。
- 選択が変わったイベント（`selection-changed`）。使うプラグインがまだない。要るときは custom イベントの `editor.selection` で足す。
- キャンバスの UI 部品、キーを離したイベント（DOOM）。
- 複数のカーソルの主を変える API、折りたたみ、仮想テキストの行。

## 進め方

1. このドキュメントで、足すもの・変えるものを決める。
2. wasmtime が semver の互換で import の名前を合わせるかを確かめる。
3. WIT を直して `nib:plugin@0.6.0` にする（変える型があるので 0.5 のマイナーを上げる）。コアと標準プラグインと SDK を同じコミットで直す。
4. ベースで困っていたところを新しい API で直す（Emacs の `C-x s` と redo のあとのポイント、vim と Emacs の `\1`、isearch の見出し、lsp の補完をどのベースでも出す、貼り付け）。
5. 使ってみて足りなければ直し、固まったら `nib:plugin@1.0.0` と SDK 1.0.0 にする。
