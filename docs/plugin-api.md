# プラグイン API

> ステータス: 合意済み（2026-09-25）

[architecture.md](architecture.md) で決めたコアの構造を、プラグインから見た API に落とし込む。WIT の実際の定義は [api/wit/plugin.wit](../api/wit/plugin.wit) にあり、実装した範囲だけを載せている。ここでは M1 までの設計の方針と API の形を決める。

## 方針

1. **型のある API と、名前で呼ぶコマンドを使い分ける。**
   - バッファの読み書き、選択、UI のように頻繁に呼ぶものは、WIT の型つき関数にする。
   - プラグイン同士の連携や、設定・コマンドラインから呼ぶ操作は、名前つきのコマンドにする。
2. **画面の座標を出さない。** 位置はすべてバッファ上のバイトオフセットで表す。縦移動やスクロールのように画面の配置に依存する操作は、コアに依頼する。
3. **コアはプラグインを再入させない。** あるプラグインの呼び出し中に、同じプラグインをもう一度呼ぶことはない。イベントは、呼び出しが終わってから届ける。
4. **プラグインが作ったものは、プラグインと一緒に片付ける。** コマンド、装飾、UI 部品、入力スタックの層は、作ったプラグインが止まったときにコアが自動で取り除く。

## プラグインの形

プラグインは 1 つのディレクトリで、次の 2 つのファイルを持つ。

```
helix-keymap/
├── plugin.toml
└── plugin.wasm   # wasm32-wasip2 向けのコンポーネント
```

`plugin.toml` には、読み込む前に知る必要のある情報を書く。

```toml
name = "helix"            # コマンドの名前空間にもなる
version = "0.1.0"
api = "0.4"               # 対応する nib:plugin のバージョン（メジャー.マイナー）

capabilities = []         # "fs-read" / "fs-write" / "process" / "network"
events = ["buffer-changed"]
```

権限をマニフェストで宣言させるのは、WASM の import だけでは権限を判定できないため。たとえば Rust の標準ライブラリを使うと、使っていなくても `wasi:filesystem` を import する。

## world

WIT パッケージは `nib:plugin`。プラグインは `plugin` world に対して書く。

以下の WIT は形を示すためのスケッチで、一部の型は省略している。wasm-tools での検証は、`api/` に置くときに行う。

```wit
package nib:plugin@0.4.0;

world plugin {
    import editor;
    import input;
    import commands;
    import ui;
    import syntax;
    import events;
    import timers;
    import process;   // 宣言がなければ、呼ぶと permission-denied を返す

    export guest;
}
```

### プラグインが実装する関数（guest）

```wit
interface guest {
    use types.{key-event, event};

    enum key-result { handled, pass }

    /// 読み込み直後に 1 回呼ぶ。config は plugins/<name>.toml の [settings] を JSON にしたもの
    init: func(config: string) -> result<_, string>;

    /// 入力スタックに積んだ層にキーが届いたとき
    handle-key: func(ev: key-event) -> key-result;

    /// 自分が登録したコマンドが呼ばれたとき。name は登録したときの名前。引数と戻り値は JSON
    run-command: func(name: string, args: string) -> result<string, string>;

    /// 購読したイベント
    on-event: func(ev: event);
}
```

### コアが提供する関数（抜粋）

```wit
interface types {
    type offset = u64;

    record sel-range { anchor: offset, head: offset }
    record selection { ranges: list<sel-range>, primary: u32 }

    /// 変更前のバッファでの範囲 [start, end) を text で置き換える
    record edit { start: offset, end: offset, text: string }

    enum undo-mode { new-step, merge }
    enum cursor-shape { block, bar, underline }

    flags modifiers { ctrl, alt, shift, super }
    variant key-code {
        %char(char), enter, escape, tab, backspace, delete,
        up, down, left, right, home, end, page-up, page-down, f(u8),
    }
    record key-event { code: key-code, modifiers: modifiers }

    /// style はテーマの名前（例: "ui.statusline"）
    record span { text: string, style: string }
    type styled-line = list<span>;
}

interface editor {
    use types.{offset, selection, edit, undo-mode, cursor-shape};

    variant error {
        stale-version, invalid-position, overlapping-edits,
        invalid-selection, invalid-pattern(string),
    }

    resource buffer {
        /// 変更のたびに増える（undo と redo も含む）
        version: func() -> u64;
        len: func() -> offset;
        slice: func(start: offset, end: offset) -> result<string, error>;
        line-count: func() -> u64;
        /// 行がなければ none
        line-start: func(line: u64) -> option<offset>;
        line-of: func(pos: offset) -> result<u64, error>;
        next-grapheme: func(pos: offset) -> result<offset, error>;
        prev-grapheme: func(pos: offset) -> result<offset, error>;
        path: func() -> option<string>;
        /// 正規表現で検索する。見つかった範囲を返す
        find: func(pattern: string, start: offset, backward: bool) -> result<option<tuple<offset, offset>>, error>;
        find-all: func(pattern: string, start: offset, end: offset) -> result<list<tuple<offset, offset>>, error>;
    }

    variant scroll-amount { lines(s32), half-page(s32), page(s32) }

    resource view {
        buffer: func() -> buffer;
        selection: func() -> selection;
        set-selection: func(sel: selection) -> result<_, error>;
        apply: func(base-version: u64, edits: list<edit>,
                    after: option<selection>, undo: undo-mode) -> result<_, error>;
        undo: func() -> bool;
        redo: func() -> bool;
        set-cursor-shape: func(shape: cursor-shape);
        /// 表示上の行単位で縦に動かした位置と、目指した列を返す（折り返しとタブを考慮する）。
        /// 返った列を次に渡すと、短い行を通っても列を保てる
        move-vertically: func(pos: offset, lines: s32, column: option<u32>) -> result<tuple<offset, u32>, error>;
        /// カーソルを動かさずに表示を動かし、動かした行数を返す（上向きは負）
        scroll: func(amount: scroll-amount) -> s32;
        /// 表示中のバッファの範囲
        visible-range: func() -> tuple<offset, offset>;
    }

    active-view: func() -> view;
    /// 開いているすべてのバッファ
    buffers: func() -> list<buffer>;
    /// エディタの作業ディレクトリ（絶対パス）。バッファのパスは開いたときの形なので、ここからの相対パスのことがある
    working-directory: func() -> string;
}
```

## バッファと選択

- 変更は `view.apply` で、`edit` のリストとして渡す。
  - 範囲はすべて変更前のバッファで数え、互いに重なってはいけない。LSP の `TextEdit` と同じ考え方なので、どの言語でも組み立てやすい。
  - `base-version` がバッファの現在のバージョンと違えば `stale-version` を返す。
- undo の単位は `undo-mode` で指定する。
  - `new-step` は新しい 1 手を始める。
  - `merge` は直前の 1 手にまとめる。
  - 挿入モードでは、最初の打鍵を `new-step`、以降を `merge` にすれば、挿入全体が 1 手になる。
- 閉じたバッファやビューの handle を使うと、プラグインはトラップする。プラグインのバグとして扱い、再起動の対象にする。
- 書記素の境界は、コアが `next-grapheme` / `prev-grapheme` として提供する。プラグインごとに Unicode の表を持たなくて済み、描画とも結果が食い違わない。
- 正規表現の検索は、コアが `find` / `find-all` として提供する。プラグインが自前で検索すると、大きなバッファの全文を毎回コピーすることになるため。
- 縦移動（`j` / `k`）、スクロール、表示範囲は、画面の配置を知っているコアが計算する。プラグインは画面の行と列を知らないまま、これらの操作を書ける。

## 構文木

構文木はコアが持ち、プラグインは `syntax` インターフェースで読む。テキストオブジェクト（`maf` など）、選択の拡大（`Alt-o`）、括弧の対応をプラグインが書けるようにするため。

```wit
interface syntax {
    use types.{offset};
    use editor.{buffer};

    /// 構文木のノード。返したときの構文木での値
    record node {
        /// 同じ範囲のノード（式文とその中の呼び出しなど）を区別する
        id: u64,
        /// 文法での名前。"function_item" や "("
        kind: string,
        /// 記号やキーワードは false
        named: bool,
        start: offset,
        end: offset,
    }

    /// バッファの言語。構文木がなければ none
    language: func(buf: borrow<buffer>) -> option<string>;
    /// start..end を覆う最も小さいノード。named なら名前つきのノードに限る
    node-at: func(buf: borrow<buffer>, start: offset, end: offset, named: bool) -> option<node>;
    parent: func(buf: borrow<buffer>, of: node) -> option<node>;
    /// 名前つきでないものも含めて、先頭から順に
    children: func(buf: borrow<buffer>, of: node) -> list<node>;
    /// 言語のクエリ query（"textobjects" など）を start..end に実行し、
    /// capture という名前の捕獲の範囲を先頭から順に返す
    captures: func(buf: borrow<buffer>, query: string, capture: string,
                   start: offset, end: offset) -> list<tuple<offset, offset>>;
}
```

- ノードはリソースではなく値で渡す。tree-sitter のノードは構文木への参照なので、編集をまたいで持ち続けられない。`parent` と `children` は、渡されたノードを `id` と範囲で構文木から探し直す。
  - バッファが変わると、古いノードは見つからないことがある。そのときは `none` や空のリストを返す。プラグインは編集のたびに `node-at` からやり直す。
- 呼ばれたとき、構文木が古ければ（同じ呼び出しの中で `apply` した直後など）解析し直してから答える。裏のスレッドで解析中なら、それを待つ。隠れているバッファも、このときに解析する。
- 構文木のないバッファ（言語がない、文法の読み込みに失敗した）では、`none` や空のリストを返す。プラグインはテキストだけの処理に戻ればよい。
- injection の層（Markdown のコードブロックの中の Rust など、[architecture.md](architecture.md) の「構文木」）の中では、その層の構文木で答える。キーマップは、コードブロックの中でも `maf` や `Alt-o` を言語ごとの違いを知らずに書ける。
  - `node-at` は、範囲を覆ういちばん内側の層（解析済みのもの）の構文木で答える。層が範囲を覆うのは、範囲の始まりと終わりが、それぞれ層の解析範囲に入っているとき。ファイルのドキュメントコメントを全部つないだ Markdown の層でも、あいだにある普通のコードは層に入らない。
  - `parent` と `children` は、ノードが属する構文木で答える。層のいちばん外のノードの親は、埋め込んだ側の構文木の、層を囲むノード（`Alt-o` がコードブロックの外へ広がる）。
  - `captures` は、バッファの言語の構文木と、範囲にかかる解析済みの層のそれぞれで、その言語のクエリを実行し、結果を位置の順に混ぜる。
  - 答える前に、層の解析も待つ。画面から遠い層は解析していないことがあり、そこではバッファの言語の構文木で答える。
  - `language` はバッファの言語を返す（層の言語ではない）。
- `captures` は、1 つのマッチで同じ名前の捕獲が複数のノードにかかったとき（`(line_comment)+ @comment.around` など）、最初から最後までを 1 つの範囲にまとめる。同じ範囲は 1 度だけ返す。
  - 範囲と重なるマッチをすべて返す。カーソルを囲む関数を探すなら、カーソルの位置だけを渡して、返った範囲から選ぶ。
  - 言語がそのクエリを持っていなければ空のリストを返す。クエリの誤りは、言語プラグインのバグとしてメッセージで知らせる。
- クエリの捕獲の名前は Helix にそろえる（`function.inside` / `function.around`、`class.*`、`parameter.*`、`comment.*`、`test.*`）。言語プラグインが対応すれば、キーマッププラグインは言語ごとの違いを知らずに済む。

## コマンド

- `commands.register(name, description)` で登録する。登録名の前には、マニフェストの `name` が自動で付く（`move_next_word` → `helix.move_next_word`）。
  - 呼ばれると、登録したプラグインの `run-command(name, args)` に、登録したときの名前（`move_next_word`）で届く。
  - 同じ名前をもう一度登録すると、説明を差し替える。プラグインが止まると登録は消える。
- `buffer.`、`editor.`、`view.` で始まる名前はコア用に予約する。
- `commands.call(name, args)` で呼ぶ。
  - 引数と戻り値は JSON 文字列。
  - 呼び出しは同期的で、戻り値をその場で受け取れる。
  - 呼び出し先がすでに呼び出し中のプラグイン（呼び出し元自身や、その呼び出し元）なら、再入になるのでエラーを返す。
  - 呼び出し先のプラグインが落ちたときは、呼び出し元にはエラーが返る。落ちたプラグインの再起動は、いちばん外側の呼び出しが終わってから行う。
- `commands.all()` で、登録済みのコマンドの名前と説明を得る（コマンドの一覧や補完に使う）。
- コアのコマンド（引数は JSON）:

| コマンド | 内容 |
|----------|------|
| `buffer.open` | `{"path": string}` のファイルを開く |
| `buffer.save` | 表示中のバッファを保存する |
| `buffer.next` / `buffer.previous` | 次 / 前のバッファを表示する |
| `buffer.close` | 表示中のバッファを閉じる。保存していない変更があれば断り、`{"force": true}` で捨てる |
| `view.split` | `{"direction": "vertical" \| "horizontal"}` で分割する |
| `view.close` / `view.only` | フォーカスのあるビューを閉じる / それ以外を閉じる |
| `view.focus` | `{"to": "next" \| "left" \| "right" \| "up" \| "down"}` へフォーカスを移す |
| `editor.quit` | 終了する。`{"force": true}` で保存していない変更を捨てる |

  分割表示のコマンドは [architecture.md](architecture.md) の「分割表示」も見る。
- バッファを閉じると、それを表示していたビューは、前の開いているバッファを表示する（最後の 1 つなら、空のバッファを作る）。
  - 閉じたバッファは一覧の中に空で残し、番号をずらさない。プラグインが持つバッファの handle は番号なので、ほかのバッファの handle がずれないため。閉じたバッファの handle を使うとトラップする。`editor.buffers()` には出ない。
  - まだ届けていない、そのバッファのイベントは捨てる。閉じたあとで、閉じたバッファの handle を渡さないため。

呼び出しを同期にできるのは、呼び出し中はエディタの状態とプラグインの一覧をそのプラグインのストアに貸しているため。呼び出し先のプラグインへは、貸したものをそのまま又貸しする（[architecture.md](architecture.md) の「プラグインの実行」）。

JSON を選んだのは、WIT に再帰する型がなく、任意の値の木を型で表せないため。どの言語にも JSON の実装はある。

## 入力

- `input.push-layer()` で入力スタックに層を積み、`input.pop-layer()` で外す。キーは上の層から順に `handle-key` で届き、`pass` を返すと下の層に回る。
- Ctrl-g はコアの予約キーなので、プラグインには届かない。予約キーは利用者が `[core]` の `menu-key` で変えられる（[architecture.md](architecture.md) の「入力」）。
- キーマッププラグインは `init` で 1 層積み、それを外さない。
- 貼り付け（bracketed paste）は、キーではなく `paste` イベントとして届ける。

## イベント

- イベントは guest の `on-event(ev)` で届く。
- プラグインは、マニフェストの `events` に書いた種類のイベントだけを受け取る。全イベントを全プラグインに配ることはしない。

```toml
events = ["buffer-opened", "buffer-changed", "helix.mode_changed"]
```

- 主なイベント:

| イベント | 内容 | 届く先 |
|----------|------|--------|
| `buffer-opened` / `buffer-saved` | バッファを開いた・保存した | `events` に書いたプラグイン |
| `buffer-changed` | バッファの変更。変更後のバージョンと、変更の列 | 同上 |
| `<plugin>.<name>` | プラグインが `events.emit(name, json)` で出したもの（custom イベント） | 同上 |
| `editor.buffer_closed` | バッファを閉じた。`{"path": string \| null}` | 同上 |
| `editor.syntax_updated` | バッファの構文木が、編集のあとの解析で最新になった。`{"path": string \| null, "version": number}`（バッファのパスと、解析した時点のバージョン） | 同上 |
| `timer` | `timers.set` で予約した時間がたった | 予約したプラグインだけ |
| `process-output` / `process-exit` | 起動した外部プロセスの出力と終了 | 起動したプラグインだけ |
| `files-listed` | `files.walk` で頼んだファイルの一覧（1,000 件ずつ） | 頼んだプラグインだけ |
| `selection-changed`、`paste` | 必要になったときに足す | |

- `buffer-changed` の変更の列は、先頭から順に 1 つずつ適用していけば変更後のテキストになるように並べる。LSP の `didChange` の `contentChanges` と同じ考え方で、変更ごとに、その時点のテキストでの行と列（バイト数）を付ける。LSP プラグインは、これをそのまま差分の同期に使える。
  - 1 回の `apply` の編集は、後ろから順に並べる。後ろの変更は前の位置を動かさないので、どれも変更前のテキストの位置のまま使える。
  - undo と redo も同じ形で届く。
- `editor.` で始まるイベントはコアが出す。形は custom イベントと同じで、WIT の `event` に種類を足さずに済む（足すと API のバージョンが上がり、すべてのプラグインを作り直すことになる）。
- custom イベントの名前には、出したプラグインの名前が自動で付く（`events.emit("mode_changed", ...)` → `helix.mode_changed`）。custom イベントはコマンドと対になる仕組み。コマンドは「誰かに頼む」、custom イベントは「起きたことを知らせる」。たとえばキーマッププラグインがモードの変化を知らせ、ステータスラインのプラグインがそれを表示する。
- イベントは、それを起こした呼び出しが終わってから、起きた順に届ける。イベントを受けたプラグインが出したイベントも、同じ順番の最後に並ぶ。
  - 1 回にさばくイベントは 1,000 個までにする。プラグイン同士がイベントを投げ合って止まらなくなったときに、エディタが固まらないようにするため。超えた分は捨てて、メッセージで知らせる。
- プラグインより前に開いたバッファの `buffer-opened` も、読み込んだあとに届く。起動時は、ファイルを開き、プラグインを読み込んでから、たまったイベントを配るため。途中で有効にしたプラグインは、`editor.buffers()` で開いているバッファを調べる。

## タイマー

```wit
interface timers {
    /// ms ミリ秒後に 1 回だけ timer(id) のイベントを届ける
    set: func(ms: u32) -> u64;
    cancel: func(id: u64);
}
```

- 繰り返したいときは、イベントを受けたときに予約し直す。
- タイマーの時刻は、キー入力などを処理していないときに確かめる。キーの処理が長引いても、処理中に割り込んで届けることはない。
- プラグインが止まると、そのプラグインのタイマーは消える。

## UI

| 関数 | 用途 |
|------|------|
| `ui.set-status(id, side, priority, content)` / `ui.remove-status(id)` | ステータスラインに項目を出す。`side` は左か右で、同じ側では `priority` の小さい順に並べる |
| `ui.show-message(text)` | 次のキーまでメッセージを出す |
| `ui.panel(lines)` | 画面下端（ステータスラインの上）のパネル。リソースで、`update` で中身を差し替え、捨てると閉じる |
| `panel.set-cursor(option<(line, byte)>)` | パネル内のカーソル。設定している間は、バッファのカーソルの代わりにここへカーソルを出す。コマンドラインの入力位置に使う |
| `ui.popup(anchor, lines)` | 本文の上に重ねるポップアップ。パネルと同じくリソースで、`update` で中身を差し替え、捨てると閉じる |
| `ui.set-decorations(buffer, namespace, decorations)` | バッファの範囲にスタイルを付ける |
| `ui.set-notes(buffer, namespace, notes)` | バッファの位置の行末に、文字列を出す（M3.4） |

中身はすべて `styled-line` で渡し、配置と切り詰めはコアが行う。

```wit
variant popup-anchor {
    /// 表示中のバッファのこの位置の行の下（入らなければ上）
    position(offset),
    /// 本文の右下の角
    corner,
}

resource popup {
    constructor(anchor: popup-anchor, lines: list<styled-line>);
    update: func(lines: list<styled-line>);
}

/// style はテーマの名前（例: "ui.cursor.match"）
record decoration { start: offset, end: offset, style: string }

/// このプラグインが buf の namespace に付けた装飾を、decorations で置き換える。
/// 空のリストで消える
set-decorations: func(buf: borrow<buffer>, namespace: string, decorations: list<decoration>);
```

- 装飾の位置は、付けたあとの編集に合わせてコアが動かす（[architecture.md](architecture.md) の「装飾」）。プラグインが編集のたびに付け直す必要はない。
- 装飾の範囲はバッファの長さに切り詰め、空の範囲は捨てる。
- 注記（`record note { at: offset, text: string, style: string }`）も、装飾と同じく名前空間ごとに差し替え、位置は編集に合わせて動く。`at` の行の末尾に描く。まわりのテキストが消えても取り除かず、消えた場所へ動く（LSP の診断のように、編集のたびに付け直されるものに使う想定）。
- ポップアップの位置は編集に合わせて動かない。位置を変えたいときは作り直す。

## 権限

| 権限 | 与えるもの |
|------|------------|
| （なし） | 上記の API すべてと、プラグイン専用のデータディレクトリ（`~/.local/share/nib/plugins/<name>/`） |
| `fs-read` / `fs-write` | 作業ディレクトリ以下の読み取り / 書き込み（WASI の preopen で渡す）。`fs-read` は `files.walk` も使える |
| `process` | `process.spawn` による外部プロセスの起動 |
| `network` | `wasi:sockets` / `wasi:http` |
| `clipboard` | `clipboard.get` / `clipboard.set` によるシステムのクリップボードの読み書き |

- 宣言した権限は、確認なしですべて与える。
- 宣言していない権限は与えない。宣言を強制することで、一覧の内容が常に実態と一致する。
  - `process` がなければ、`process.spawn` はエラーを返す。
  - `fs-read` / `fs-write` がなければ、WASI にディレクトリを渡さない。`network` がなければ、WASI のソケットはどこにもつながらない。
  - 知らない名前の権限を書いたプラグインは、読み込まない。書き間違いで権限が抜けたまま動くのを防ぐため。
- 読み込んでいるプラグインと権限は、Ctrl-g のメニューで一覧できる。
- プラグイン専用のデータディレクトリは、プラグインから WASI の `/data` として見える（Rust なら `std::fs::write("/data/state.json", …)`）。権限の宣言は要らない。そのプラグインのものだけが見え、ほかのプラグインのものは見えない。
  - ディレクトリは、プラグインを起動するときに作る（WASI に渡すにはディレクトリが要るため）。
  - 置き場はフロントエンドが決める（tui は `~/.local/share/nib/plugins/<name>/`、`$XDG_DATA_HOME` があればその下）。決めなければ `/data` はない。`nib plugin test` は、テストごとに空の一時ディレクトリを渡す。
- 大量のファイルを列挙するような重い I/O は、コアが非同期の仕事として提供し、結果をイベントで返す。WASI のファイル API は同期的なので、プラグインが直接やるとメインスレッドが止まる。

## 設定

```wit
interface settings {
    /// 表示中のバッファでの値（JSON）: "tab-width"、"indent"（空白の数か "tab"）、"scroll-margin"
    get: func(key: string) -> option<string>;
    /// buf での値
    get-for: func(buf: borrow<buffer>, key: string) -> option<string>;
    /// buf でだけ値を変える。none で元に戻す
    set-for: func(buf: borrow<buffer>, key: string, value: option<string>) -> result<_, string>;
}
```

- 値は、config.toml の `[core]` の値を、プラグインがバッファ単位で上書きしたもの（[architecture.md](architecture.md) の「config.toml」）。
- バッファ単位で変えられるのは `tab-width` と `indent` だけ。`scroll-margin` は画面の設定なので、バッファには持たせない。安全装置（予約キー、プラグインの上限）は、どの形でもプラグインから変えられない。
- 同じバッファの同じキーを複数のプラグインが変えたら、あとから変えたほうが勝つ。変えたプラグインが止まると、その上書きは消える。
- 言語ごとの既定値は、標準プラグイン `indent` が受け持つ。バッファが開いたら言語を調べ、`set-for` で上書きする。既定では Go をタブ、YAML と JSON を空白 2 つにする。利用者は `plugins/indent.toml` で変えられる。

```toml
# ~/.config/nib/plugins/indent.toml
[settings.languages.python]
indent = 4
tab-width = 8
```

## ファイルの一覧

```wit
interface files {
    /// dir（なければ作業ディレクトリ）の下のファイルを、裏のスレッドで数える。
    /// 結果は files-listed イベントで少しずつ届く。仕事の id を返す
    walk: func(dir: option<string>) -> result<u64, string>;
    cancel: func(id: u64);
}
```

- `.gitignore`（と `.ignore`、git の除外設定）を守り、隠しファイルは数えない。ripgrep と同じ `ignore` クレートを使う。
- 結果は `files-listed(id, paths, done)` のイベントで、1,000 件ずつ届く。`paths` は `dir` からの相対パスで、区切りは `/`。最後の 1 回は `done` が真。
- イベントは、一覧を頼んだプラグインにだけ届く。
- 使うには `fs-read` の権限が要る。ファイル名から、利用者のディレクトリの中身がわかるため。
- WASI のファイル API で数えると、呼び出しのあいだメインスレッドが止まる。この API なら、数えているあいだもエディタは動き続ける（「待たせない」）。

## クリップボード

```wit
interface clipboard {
    get: func() -> result<string, string>;
    set: func(text: string) -> result<_, string>;
}
```

- 同期で読み書きする。貼り付けのように、その場で中身が要る操作のため。OS のクリップボードの読み書きは速いので、呼び出しの時間の上限に収まる。
- 使うには `clipboard` の権限が要る。クリップボードにはパスワードのような秘密が入ることがあるため、読み書きの両方を権限の対象にする。
- 読み書きの中身はフロントエンドが決める（[architecture.md](architecture.md) の「クレート構成」）。

## 外部プロセス

```wit
interface process {
    enum stream { stdout, stderr }

    /// 起動した子プロセス。捨てると終了させる
    resource child {
        /// イベントで、どの子プロセスのものかを見分ける
        id: func() -> u64;
        /// 標準入力に書く
        write: func(data: list<u8>) -> result<_, string>;
        /// 標準入力を閉じる
        close-stdin: func();
        kill: func();
    }

    /// command を args で起動する。cwd がなければエディタの作業ディレクトリで動かす
    spawn: func(command: string, args: list<string>, cwd: option<string>) -> result<child, string>;
}
```

- 出力は `process-output(id, stream, data)`、終了は `process-exit(id, code)` のイベントで、起動したプラグインにだけ届く。マニフェストの `events` に書かなくてよい。
  - 出力はバイト列のまま、読めた分ずつ届ける。行や LSP のメッセージへの区切りは、プラグインが行う。
  - 終了コードは、シグナルで終わったときなど、ないこともある。
- 読み取りは裏のスレッドで行う。プラグインは出力を待たずに戻り、届いたらイベントで受け取る（architecture.md の「待たせない」）。
- 標準入力への書き込みは、そのままパイプに書く。相手が読まずにパイプが詰まると書き込みが止まるので、大量に書くときは相手の出力も読むこと（LSP では問題にならない量）。
- プラグインが止まると、そのプラグインが起動したプロセスはすべて終了させる。

## ライフサイクル

1. `plugin.toml` を読み、`api` のバージョンを確かめる。合わなければ読み込まずにエラーを表示する。
2. `plugin.wasm` をコンパイルする（キャッシュがあれば使う）。
3. 宣言した権限に合わせて import を用意し、インスタンスを作る。
4. `init` を呼ぶ（時間の上限は 5 秒）。コマンドの登録や入力スタックへの層の追加は、ここで行う。
5. 以降は、キー・コマンド・イベントのたびに呼ぶ（時間の上限は 1 秒）。
6. 停止やトラップが起きたら、そのプラグインが作ったものをすべて片付け、新しいインスタンスで 3 からやり直す。1 分間に 3 回落ちたら無効にする。

再起動すると、プラグインの内部状態は失われる。残したい状態は、データディレクトリ（`/data`）に書く。

## バージョン

- `nib:plugin` は semver で管理する。1.0 までは互換性を保証しない。
- コアが対応するのは 1 つのバージョンだけ。標準プラグインは同じリポジトリにあるので、API を変えるときは同じコミットで直す。
- SDK のバージョンは、`nib:plugin` のバージョンが変わったときだけ上げる。例外は、Go のモジュールのパスが変わったときのようにタグを打ち直さないと SDK を取れなくなるときで、パッチだけを上げる（下の「SDK」の Go）。

## SDK

- **Rust**（`sdk/rust`、クレート `nib-plugin`）: wit-bindgen の生成コードを包み、`Plugin` トレイトと `export!` マクロを提供する。`wasm32-wasip2` 向けにビルドするだけで、コンポーネントが出来上がる。crates.io には出さず、リポジトリの外のプラグインは git のタグ `sdk/rust/vX.Y.Z` で取る（`nib-plugin = { git = "https://github.com/nib-editor/nib", tag = "sdk/rust/v0.4.2" }`）。
- **Go**（`sdk/go`、モジュール `github.com/nib-editor/nib/sdk/go`）: M4.8 で作った。
  - 本家の Go（1.27）は `wasip1` までで、コンポーネント（`wasip2`）を作れない。TinyGo（0.42 以降）の `-target=wasip2` で作る。TinyGo は wasm の最適化とゴルーチンの仕組みのために binaryen の `wasm-opt` を、コンポーネントに包むために `wasm-tools` を使うので、それらも要る。
  - 型と関数は、wit-bindgen-go（Bytecode Alliance）で WIT から生成し、生成したコードをリポジトリに置く。利用者は生成の道具を持たなくてよい。`api/wit/` を変えたら、`go generate` で作り直す。
  - 生成したままの形は書きにくい（戻り値が `cm.Result` など）ので、薄いパッケージ `nib` をかぶせる。`nib.Plugin` インターフェースを実装して `nib.Register` に渡すだけで、4 つの関数（`init`、`handle-key`、`run-command`、`on-event`）がつながる。使わない関数は `nib.Base` を埋め込めば省ける。
  - TinyGo はコンポーネントを作るときに WIT のファイルを読むので、モジュールに WIT の写しを置く（`sdk/go/wit/deps/nib-plugin/`）。`api/wit/` と同じであることを CI で確かめる。
  - TinyGo の実行環境は WASI（環境変数、時計など）を import する。Rust のツールはそれを world に自動で足すが、TinyGo は足さないので、`sdk/go/wit/world.wit` に nib の `plugin` world と `wasi:cli/imports@0.2.0` をまとめた world（`nib:go-plugin`）を置く。WASI の WIT は TinyGo に同梱のものを写す。
  - ビルドは次のとおり。

```sh
tinygo build -target=wasip2 \
  --wit-package "$(go list -m -f '{{.Dir}}' github.com/nib-editor/nib/sdk/go)/wit" \
  --wit-world plugin -o plugin.wasm .
```

  - バージョンは Rust の SDK と同じく API に合わせ、`v0.4.0` から始めた。タグは `sdk/go/vX.Y.Z`。
  - リポジトリを `nib-editor` に移したとき、モジュールのパスが `github.com/q0tzly/nib/sdk/go` から変わったので、API は同じまま `v0.4.1` を出した。Go は、タグの go.mod に書かれたパスが取りに行ったパスと違うと使えないため、古いタグ（`v0.4.0`）は新しいパスでは取れない。

## 作る順番

Helix 風キーマップで nib 自身を編集するのに必要なものから作る。

| 範囲 | 時期 |
|------|------|
| `editor`（バッファ、選択、`apply`、undo、検索、縦移動、スクロール、カーソルの形） | M1 |
| `input`（入力スタック） | M1 |
| `commands.call`（コアのコマンド） | M1 |
| `ui.set-status`、`ui.panel` | M1 |
| 構文木の API | M2.2 |
| `ui.popup`、装飾 | M2.3 |
| `commands.register`、`events`、`timers`、`editor.buffers` | M3.1 |
| `process`、権限の宣言 | M3.2 |
| Go SDK | M4 |
