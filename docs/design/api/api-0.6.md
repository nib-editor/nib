# プラグイン API 0.6 の改革

> ステータス: 合意済み（2026-09-28）。nib 0.10.0 と `nib:plugin@0.6.0` で入れる。

1.0.0 で API を固める前に（[roadmap.md](../project/roadmap.md) の「1.0.0」）、今の形（`nib:plugin@0.5.3`）を作り直す。使う人がまだ少なく、合わせて直すプラグインも標準プラグインとサンプル（`wordcount`）だけなので、互換を気にせず変える。

材料は、helix、nano、vim、Emacs の 4 つのベースと、lsp、picker、indent を公開 API だけで書いて困ったこと。プラグインが中身を持つバッファ（magit や dired のようなもの）は、この改革のあとで別に考える。

## 何を変えるか

1. **コアの操作を型のある関数にする。** 今はファイルを開く、保存する、ビューを分ける、終わるといったコアの操作を、名前と JSON の文字列で呼ぶコマンドにしている。プラグインのあちこちで JSON を組み立てていて（`format!(r#"{{"path":{}}}"#, json_string(..))`）、引数の間違いは実行するまで分からない。これを関数にする。
   - コマンドは、キーや `:`、`M-x` から名前で呼ぶための表という役に絞る。コアのコマンド（`buffer.save`、`view.split` など）は、同じ関数を呼ぶ名前として残す。
2. **イベントを型にする。** コアが出すイベントのうち `editor.buffer_closed` と `editor.syntax_updated` は、WIT の種類を足すと全プラグインを作り直すことになるのを避けて、custom イベント（名前と JSON）の形で出していた。0.x のうちは種類を足せるので、WIT の種類にする。custom イベントは、プラグインどうしの知らせだけに使う。
3. **interface を分け直す。** `editor` にバッファ、ビュー、エディタ全体の関数が同居しているのを、`buffer`、`view`、`editor` に分ける。同じことを 2 通りに書ける所を 1 つにする（`settings.get` と `get-for`）。
4. **ベースを作って困ったものを足す。** 下の「足したもの」。

## 新しい形

WIT の正は `api/wit/plugin.wit`。ここでは形と、そうした理由を書く。

### types

- `range`（`start`、`end`）を足し、結果で範囲を返すところ（`find`、`captures`、`visible-range` など）は `tuple<offset, offset>` の代わりにこれを使う。引数は今までどおり `start` と `end` を並べる。
- `error` を `editor` から `types` に移し、`other(string)` を足す。
- `key-code` に `insert` を足す（端末から届いても捨てていた）。

### buffer

| 関数 | 内容 |
|------|------|
| `buffer.open(path)` | ファイルを開いてバッファを返す。開いてあればそれを返す。表示はしない |
| `buffer.all()` | 開いているバッファ（今の `editor.buffers`） |
| `buffer.save(path)` | そのバッファを保存する。`path` を渡すと、そこへ保存して以後そのパスにする |
| `buffer.close(force)` | そのバッファを閉じる。表示していたビューは前のバッファを表示する |
| `buffer.modified()` | 保存したときから変わっているか。undo で保存したときの状態に戻れば false |
| `buffer.find-groups(pattern, start, backward)` | `find` と同じく探し、グループの範囲も返す。0 番が一致の全体、1 番からがグループ。参加しなかったグループは `none` |

ほかの関数（`version`、`slice`、`find`、`set-marks` など）は今と同じ。

### view

- ビューの resource が指すのは、今までどおりフォーカスのあるビュー。
- `view.show(buffer)` でバッファを表示する。`show-next()`、`show-previous()` は、今のコマンドの `buffer.next`、`buffer.previous`。
- `undo()` と `redo()` は、したかどうかの代わりに、変わったところを返す（`option<range>`。変更後のテキストで、いちばん前の変更の範囲）。Emacs の redo のあとのポイントを本物に合わせるため。
- 分割は関数: `view.split(direction)`、`view.close()`、`view.only()`、`view.focus(toward)`。`direction` と `toward` は enum。

### editor

エディタ全体のこと: `working-directory()`、`quit(force)`、`open-config(plugin)`、`reload-config()`、`open-menu()`。

### input

- `set-mode(name, typing)`: ベースが今の状態を知らせる。`name` はベースが決める（`normal`、`insert`、`visual` など。Emacs と nano は `global`）。`typing` は、修飾のない文字がテキストに入る状態か。ベース以外のプラグインが呼んでも何もしない。
- `mode()`: 今の状態（ベースの名前、`name`、`typing`）。途中で有効にしたプラグインのため。
- ベースが変わると、コアが新しいベースの状態を知らせる（新しいベースが `init` で `set-mode` を呼ぶ）。

lsp は、打鍵が止まったときに補完を出すかを `helix.mode_changed` で決めていたので、helix 以外のベースでは補完が自動で出なかった。`mode-changed` イベントの `typing` を見るようにする。

### prompt

- `line.set-label(label)` を足す。Emacs の isearch は「Failing I-search」などに変えるたびに、欄を作り直していた。
- `action` に `page-next` と `page-previous`（一覧を 1 画面送る。picker の PageDown、Emacs の `C-v`、vim の `C-f`）を足す。

### settings

JSON の文字列で受け渡していたのを、値ごとの関数にする。表示中のバッファ向けと、バッファを渡すものの 2 通りあったのを、バッファを渡す 1 通りにする。

| 関数 | 内容 |
|------|------|
| `tab-width(buffer)` / `set-tab-width(buffer, width)` | tab の幅。`none` で config.toml の値に戻す |
| `indent(buffer)` / `set-indent(buffer, indent)` | インデント（`spaces(n)` か `tab`） |
| `scroll-margin()` | カーソルと画面の端のあいだに残す行 |

### events

`event` の種類:

| 種類 | 今までとの違い |
|------|----------------|
| `buffer-opened`、`buffer-saved`、`buffer-changed` | 同じ |
| `buffer-closed(path)` | custom イベントの `editor.buffer_closed` だったもの。閉じたバッファの handle は使えないので、パスを渡す |
| `syntax-updated(buffer, version)` | custom イベントの `editor.syntax_updated` だったもの。パスの代わりにバッファを渡す |
| `mode-changed(mode)` | 新しい。`input.set-mode` の知らせ。ベースごとの `helix.mode_changed`、`vim.mode_changed` はなくす |
| `custom`、`timer`、`process-output`、`process-exit`、`files-listed`、`prompt-changed`、`prompt-action` | 同じ |

マニフェストの `events` には、種類の名前（`buffer-closed`、`syntax-updated`、`mode-changed`）を書く。

### guest

- `handle-paste(text)` を足す。端末の bracketed paste を有効にし、貼り付けたものを 1 つの塊として、キーと同じく入力スタックの上から届ける。
  - 今は貼り付けた文字が 1 つずつキーとして届き、vim の normal モードでは貼った文字がコマンドとして動き、Emacs では改行のたびに前の行のインデントが足されていた。大きな貼り付けは、1 文字ごとにベースを呼ぶので遅かった。
  - ベースが入れ方を決める。入力欄が開いていれば欄に、vim の normal モードでは `p` のようにカーソルの後ろに、挿入モードと Emacs と nano ではカーソルの位置に入れる。
  - どの層も `pass` を返したら、コアが入れる。入力欄が開いていれば欄に、なければ表示中のバッファの各選択の前に入れる。

### コマンド

- コアのコマンドは、上の関数を名前で呼ぶためのもの。キーの設定や `:`、`M-x` から使う。
- 引数の空文字列は `{}` と同じ（今もそう動くが、書いていなかった）。
- 予約する名前は `buffer`、`config`、`core`、`editor`、`view`（2 か所で違っていたのを揃える）。

### 版

- API は `nib:plugin@0.6.0`、マニフェストの `api` は `"0.6"`。SDK も 0.6.0。
- nib は 0.10.0。

## 1.0 で固めるときの決まり

1.0 で固めたあと、1.x のあいだにどこまで広げられるかを、固める前に知っておく。コンポーネントモデルは import と export の型をそのまま突き合わせるので、次のように分かれる。

| 変更 | 古いプラグインは動くか |
|------|------------------------|
| interface に関数を足す、resource にメソッドを足す | 動く（使わない import は求められない） |
| world に import の interface を足す | 動く |
| guest（プラグインが export するもの）に関数を足す | 動かない（古いプラグインにないものを、コアが求める） |
| record にフィールドを、variant と enum に場合を、flags に旗を足す | 動かない（型が変わり、読み込みで合わなくなる） |
| 関数の引数や戻り値の型を変える | 動かない |

- 型の形は 1.0 で決めたものが 1.x の終わりまで続く。1.0 のときに、`event` に 1.x で足すイベントの受け皿（`other(name, data)`）を足す。
- パッケージの版は import の名前に入る。wasmtime の Linker は semver の互換で名前を合わせる: `nib:plugin@0.7.1` のコアが、`@0.7.0` で作ったプラグインをそのまま読んで動かすことを、0.7.1 を入れたとき（2026-09-29、wasmtime 48）に確かめた。0.x ではマイナーまで、1.x ではメジャーまでが同じなら合う。1.1 のコアは 1.0 のプラグインを読める。
- マニフェストの `api` は、今はメジャーとマイナーが一致しないと読み込まない。1.x では「メジャーが同じで、マイナーがコア以下」なら読み込むようにする。

## 入れないもの

- プラグインが中身を持つバッファ。この改革のあとで考える。
- 選択が変わったイベント。使うプラグインがまだない。
- キャンバスの UI 部品、キーを離したイベント（DOOM）。kitty keyboard protocol の hyper と meta の修飾。
