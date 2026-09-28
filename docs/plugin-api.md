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
api = "0.7"               # 対応する nib:plugin のバージョン（メジャー.マイナー）

capabilities = []         # "fs-read" / "fs-write" / "process" / "network"
events = ["buffer-changed"]

base = true               # ベース（base.md）。[core] base で選ばれたものだけが動く
menu-key = "C-g"          # ベースだけ: このベースを使う間、コアメニューを開くキー

[keys]                    # 自分のコマンドのキー。ベースのリーダーからの相対（base.md）
f = "picker.files"        # helix なら Space f
"c d" = "lsp.definition"  # 空白で区切ると、前置きキーの下
```

- `[keys]` の右辺は自分のコマンド（`<name>.` で始まる）だけ。キーの書き方は設定と同じ（`C-x`、`A-f`、`space`、`tab` など）。ベースは `input.leader-keys()` で、有効なプラグインの分を読み込んだ順に受け取る。

- ベースは常に起動時に始まる（`load = "lazy"` は効かない）。選ばれていないベースは、止めたまま読み込み、コアメニューから切り替えられる。
- 名前の `buffer`、`config`、`core`、`editor`、`view` はコアのコマンドと重なるので使えない。

権限をマニフェストで宣言させるのは、WASM の import だけでは権限を判定できないため。たとえば Rust の標準ライブラリを使うと、使っていなくても `wasi:filesystem` を import する。

## world

WIT パッケージは `nib:plugin`。プラグインは `plugin` world に対して書く。型と関数の正は [api/wit/plugin.wit](../api/wit/plugin.wit) で、ここには形と決まりを書く。0.6 で作り直したときの理由は [api-0.6.md](api-0.6.md)。

### プラグインが実装する関数（guest）

| 関数 | 呼ばれるとき |
|------|--------------|
| `init(config)` | 読み込み直後に 1 回。`config` は `plugins/<name>.toml` の `[settings]` を JSON にしたもの |
| `handle-key(ev)` | 入力スタックに積んだ層にキーが届いたとき。`handled` か `pass`（下の層へ）を返す |
| `handle-paste(text)` | 端末に貼り付けたものが、キーと同じく層に届いたとき（下の「入力」） |
| `run-command(name, args)` | 自分が登録したコマンドが呼ばれたとき。`name` は登録したときの名前。引数と戻り値は JSON |
| `on-event(ev)` | 購読したイベント |

### コアが提供するもの

| interface | 中身 |
|-----------|------|
| `types` | 位置（バイトオフセット）、範囲（`range`）、選択、編集、キー、表示の文字列、失敗の種類（`error`） |
| `buffer` | バッファ（resource）の読み出し、検索（`find`、`find-groups`、`find-all`）、位置の印、保存、閉じる、変更済みか。`buffer.open`（表示せずに開く）、`buffer.all`、`buffer.create`（プラグインのバッファ） |
| `view` | ビュー（resource）の選択、編集の適用、undo と redo（変わった範囲を返す）、縦移動、スクロール、表示するバッファ。`view.active` と分割（`split`、`close`、`only`、`focus`） |
| `editor` | エディタ全体: 作業ディレクトリ、終了、設定を開く・読み直す、コアメニュー |
| `input` | 入力スタックの層、リーダーの下のキー、ベースのモード（`set-mode`、`current-mode`） |
| `prompt` | 入力欄と、文字を打つ欄のない一覧（下の「入力欄」） |
| `commands` | 名前で呼ぶコマンドの登録と呼び出し |
| `events` | イベントの種類と、custom イベントを出す `emit` |
| `ui` | ステータスライン、メッセージ、パネル、ポップアップ、装飾、注記 |
| `settings` | バッファの tab の幅とインデント、スクロールの余白 |
| `syntax` | 構文木のノードとクエリ |
| `timers`、`process`、`files`、`clipboard` | タイマー、外部プロセス、ファイルの一覧、クリップボード（権限が要るものは下の「権限」） |

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
- 正規表現の検索は、コアが `find` / `find-groups` / `find-all` として提供する。プラグインが自前で検索すると、大きなバッファの全文を毎回コピーすることになるため。`find-groups` はグループの範囲も返す（置き換えの `\1` のため）。
- `undo` と `redo` は、変わったところ（変更後のテキストで、いちばん前の変更の範囲）を返す。ベースが、元のエディタの作法でカーソルを置き直すため。
- `buffer.modified` は、保存したときの undo の状態と今の状態が違うか。undo で保存したときに戻れば、変更なしに戻る。
- 位置の印（`set-marks` / `marks`）は、vim のマークとジャンプリスト、Emacs のマークとマークリングのように、編集されても同じ場所を指し続けたい位置に使う（[base.md](base.md) の「コアに足すもの」）。
  - どのプラグインの編集でも、コアが動かす。ベースが自分で持つと、LSP の整形のような他のプラグインの編集でずれるため。
  - 動かし方は注記と同じ。印の位置に挿入された文字は印の後ろに入り（Emacs のマーカーの既定と同じ）、消された範囲の印は消えた場所へ動く。undo で文字が戻っても、印は戻らない。
  - 名前空間はプラグインごとに別で、ほかのプラグインの印は読めない。順番は置いたとおりに保つので、リストをそのままリング（マークリング）やジャンプリストに使える。
  - プラグインが止まると消える。ベースを切り替えると、前のベースの印も消える。
- 縦移動（`j` / `k`）、スクロール、表示範囲は、画面の配置を知っているコアが計算する。プラグインは画面の行と列を知らないまま、これらの操作を書ける。

## プラグインのバッファ

診断の一覧、ヘルプ、grep の結果のように、プラグインが中身を書くバッファ（[plugin-buffers.md](plugin-buffers.md)）。

- `buffer.create(name)` で作る。パスはなく、`name`（`*diagnostics*` など）がステータスラインとバッファの一覧に出る。表示は `view.show`。
- 中身は、作ったプラグイン（持ち主）が `buffer.apply` で書く。表示していなくても書ける。
- ビューでの編集（`view.apply`、undo、redo、コアの貼り付け）は、持ち主のものも含めて `read-only` で断り、「`<name>` is read-only」と知らせる。ベースが持ち主のときも、ベースの編集のキーで中身が変わらないようにするため。持ち主が `set-editable(true)` にすると、ビューで編集できる。
- 保存せず、変更済みにもならない。持ち主が止まると、コアが閉じる。
- `buffer.set-keys` で、そのバッファでだけ効くキーと、持ち主のコマンドの組を渡す。ベースが `buffer.keys` で読み、自分の作法で組み込む（どのキーを譲るかは、各ベースの文書）。
  - ベースは、自分のバッファのキーが自分のコマンドを指すとき、コマンドとして呼ばずに自分で動かす。呼び出し中のプラグインを呼び返すことはできないため。キーのほか、コマンドライン（`:vim.close-listing`）や `M-x`（`emacs.quit-window`）で名前を打たれたときも同じ。base-kit の `own_command` で、自分のコマンドかを見分ける。

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
- `buffer`、`config`、`core`、`editor`、`view` で始まる名前はコア用に予約する（プラグインの名前にも使えない）。
- `commands.call(name, args)` で呼ぶ。
  - 引数と戻り値は JSON 文字列。引数の空文字列は `{}` と同じ。
  - 呼び出しは同期的で、戻り値をその場で受け取れる。
  - 呼び出し先がすでに呼び出し中のプラグイン（呼び出し元自身や、その呼び出し元）なら、再入になるのでエラーを返す。
  - 呼び出し先のプラグインが落ちたときは、呼び出し元にはエラーが返る。落ちたプラグインの再起動は、いちばん外側の呼び出しが終わってから行う。
- `commands.all()` で、登録済みのコマンドの名前と説明を得る（コマンドの一覧や補完に使う）。
- コアのコマンド（引数は JSON）。どれも上の関数を名前で呼ぶためのもので、キーの設定や `:`、`M-x` から使う。プラグインは関数を直接呼ぶ:

| コマンド | 内容 |
|----------|------|
| `buffer.open` | `{"path": string}` のファイルを開く |
| `buffer.save` | 表示中のバッファを保存する。`{"path": string}` ならそこへ保存し、以後そのパスにする |
| `buffer.next` / `buffer.previous` | 次 / 前のバッファを表示する |
| `config.open` | `config.toml` を開く。`{"plugin": name}` なら `plugins/<name>.toml`。なければ既定値のコメントを書いた状態で開く |
| `config.reload` | 設定を読み直す（設定のディレクトリのファイルを保存したときも読み直す） |
| `core.menu` | コアメニューを開く |
| `buffer.close` | 表示中のバッファを閉じる。保存していない変更があれば断り、`{"force": true}` で捨てる |
| `view.split` | `{"direction": "vertical" \| "horizontal"}` で分割する |
| `view.close` / `view.only` | フォーカスのあるビューを閉じる / それ以外を閉じる |
| `view.focus` | `{"to": "next" \| "left" \| "right" \| "up" \| "down"}` へフォーカスを移す |
| `editor.quit` | 終了する。`{"force": true}` で保存していない変更を捨てる |

  分割表示のコマンドは [architecture.md](architecture.md) の「分割表示」も見る。
- バッファを閉じると、それを表示していたビューは、前の開いているバッファを表示する（最後の 1 つなら、空のバッファを作る）。
  - 閉じたバッファは一覧の中に空で残し、番号をずらさない。プラグインが持つバッファの handle は番号なので、ほかのバッファの handle がずれないため。閉じたバッファの handle を使うとトラップする。`buffer.all()` には出ない。
- `view.show` でバッファを表示すると、nib が起動したときの空のバッファ（パスがなく、何も打っていないもの）は閉じる。ファイルを開いたときにそれが残らないように。
  - まだ届けていない、そのバッファのイベントは捨てる。閉じたあとで、閉じたバッファの handle を渡さないため。

呼び出しを同期にできるのは、呼び出し中はエディタの状態とプラグインの一覧をそのプラグインのストアに貸しているため。呼び出し先のプラグインへは、貸したものをそのまま又貸しする（[architecture.md](architecture.md) の「プラグインの実行」）。

JSON を選んだのは、WIT に再帰する型がなく、任意の値の木を型で表せないため。どの言語にも JSON の実装はある。

## 入力

- `input.push-layer()` で入力スタックに層を積み、`input.pop-layer()` で外す。キーは上の層から順に `handle-key` で届き、`pass` を返すと下の層に回る。
- コアメニューのキー（既定は Ctrl-g、ベースが `menu-key` で決め、利用者が `[core]` の `menu-key` で変えられる）はプラグインに届かない（[architecture.md](architecture.md) の「入力」）。
- キーマッププラグインは `init` で 1 層積み、それを外さない。
- 端末に貼り付けたもの（bracketed paste）は、キーではなく 1 つの塊として、`handle-paste` に同じ順で届く。入力欄が開いているあいだはベースに届く。どのプラグインも `pass` を返したら、コアが入れる: 入力欄が開いていれば欄に（改行は空白にする）、なければ表示中のバッファの各選択の前に、新しい undo の 1 手として。
  - 貼り付けたものが 1 文字ずつキーとして届くと、vim の normal モードではコマンドとして動き、Emacs では改行のたびにインデントが足されるため。
- ベースは `input.set-mode(name, typing)` でモードを知らせる。`typing` は、修飾のない文字がテキストに入る状態か（挿入モード、モードのないベース）。ほかのプラグインは `mode-changed` イベントか `input.current-mode()` で知る（lsp は、打鍵が止まったときに補完を出すかをこれで決める）。ベース以外が呼んでも何もしない。

## 入力欄

`:` のコマンドライン、`/` の検索、picker の検索欄のように、1 行の文字を打つ欄。仕組みはコアが、キーの作法はベースが持つ（[base.md](base.md) の「プラグインの画面の操作」）。

```wit
interface prompt {
    enum action { accept, cancel, next, previous, page-next, page-previous, complete, complete-back }

    resource line {
        constructor(label: string);   // label は ":" や "files> "
        id: func() -> u64;
        text: func() -> string;
        cursor: func() -> u32;        // text の中のバイト位置
        set: func(text: string, cursor: u32);
        set-label: func(label: string);
        set-hint: func(hint: string); // 右端に出す。"3/10" など
    }

    resource choices {
        constructor(actions: list<action>);  // 補完の一覧など、文字を打つ欄のない一覧
        id: func() -> u64;
    }

    record state { id: u64, label: string, text: string, cursor: u32, mine: bool }
    record offer { id: u64, actions: list<action> }
    active: func() -> option<state>;       // ベースが使う
    edit: func(text: string, cursor: u32); // ベースが使う
    offered: func() -> option<offer>;      // ベースが使う
    act: func(action: action);             // ベースが使う
}
```

- 欄を開くのは、使うプラグイン（持ち主）。`prompt.line(label)` で開き、リソースを捨てると閉じる。ベース自身の `:` も同じ。
- 開いている欄のうち、最後に開いたもの（アクティブな欄）だけをステータスラインのすぐ上に描き、端末のカーソルをそこに置く。パネルはその上に並ぶ。
- アクティブな欄があるあいだ、キーは入力スタックではなく、使っているベースの `handle-key` に届く。ベースは `prompt.active()` で欄があるかを知り、自分の作法でキーを解釈する。
  - 文字列を変えるときは `prompt.edit(text, cursor)`。持ち主に `prompt-changed` が届く。
  - 決定、取り消し、次の候補などは `prompt.act(action)`。持ち主に `prompt-action` が届く。
  - 知らないキーは `pass` を返す。コアが既定の動きをする: 修飾のない文字は入れる、Backspace は 1 文字消す（空なら `cancel`）、Delete、左右、Home、End で動く、上下は `previous` / `next`、PageUp と PageDown は `page-previous` / `page-next`、Tab と Shift-Tab は `complete` / `complete-back`、Enter は `accept`、Esc は `cancel`。ベースがない、または作りかけでも、欄は使える。
- 持ち主は、イベントを受けて動く。決定や取り消しで欄を閉じるのも持ち主（捨てる）。欄を開いたまま持ち主が止まると、コアが閉じる。
- `set` は持ち主が補完などで文字列を書き換えるためのもので、`prompt-changed` は出ない。
- 一覧（picker の候補、補完の候補）は、持ち主がパネルやポップアップで描く。欄が持つのは 1 行の文字と右端の hint だけ。

### 文字を打つ欄のない一覧（choices）

LSP の補完やホバーのように、文字は本文に打ちながら、一覧の操作だけを受けたいもの。

- 持ち主は `prompt.choices(actions)` で、受け取る操作の意味を並べて開く（補完なら `next`、`previous`、`accept`）。捨てると閉じる。描くのは持ち主。
- キーはいつもどおり入力スタックを流れる。ベースは `prompt.offered()` で開いている一覧と、それが受け取る操作を知り、自分の作法のキー（helix なら `C-n` / `C-p` / 上下 / Tab / Enter）を `prompt.act` で送る。入力欄が開いているあいだは、`offered` は何も返さない。
- 一覧に操作を送らなかったキーは、ふつうに処理したうえで、コアが持ち主に `cancel` を送る。そのキーが起こしたイベント（本文の変更など）より前に届く。一覧はもう合わないので、持ち主は閉じる。操作を 1 つも受けない一覧（ホバー）は、次のキーで閉じる。
- ベースが対応していなければ、どのキーでも閉じる。入力欄と違って既定のキーはない（キーの意味を本文の編集と取り合うため）。
- 入力欄と id の数え方を共有し、同じ `prompt-action` で届く。
- 入力スタックに層を積んでキーを先取りする方法は、プラグインの画面がベースの作法に合わなくなるので、一覧には使わない。
## イベント

- イベントは guest の `on-event(ev)` で届く。
- プラグインは、マニフェストの `events` に書いた種類のイベントだけを受け取る。全イベントを全プラグインに配ることはしない。

```toml
events = ["buffer-opened", "buffer-changed", "mode-changed", "wordcount.counted"]
```

- 主なイベント:

| イベント | 内容 | 届く先 |
|----------|------|--------|
| `buffer-opened` / `buffer-saved` | バッファを開いた・保存した | `events` に書いたプラグイン |
| `buffer-changed` | バッファの変更。変更後のバージョンと、変更の列 | 同上 |
| `buffer-closed` | バッファを閉じた。閉じたバッファの handle は使えないので、パスと名前を渡す | 同上 |
| `syntax-updated` | バッファの構文木が、編集のあとの解析で最新になった。バッファと、解析した時点のバージョン | 同上 |
| `mode-changed` | ベースがモードを知らせた（`input.set-mode`）。ベース、モードの名前、`typing` | 同上 |
| `<plugin>.<name>` | プラグインが `events.emit(name, json)` で出したもの（custom イベント） | 同上 |
| `timer` | `timers.set` で予約した時間がたった | 予約したプラグインだけ |
| `process-output` / `process-exit` | 起動した外部プロセスの出力と終了 | 起動したプラグインだけ |
| `files-listed` | `files.walk` で頼んだファイルの一覧（1,000 件ずつ） | 頼んだプラグインだけ |
| `prompt-changed` / `prompt-action` | 入力欄の文字列が変わった（id、文字列、カーソル）・操作の意味（id、`action`） | 欄を開いたプラグインだけ |
| `selection-changed` | 必要になったときに足す | |

- `buffer-changed` の変更の列は、先頭から順に 1 つずつ適用していけば変更後のテキストになるように並べる。LSP の `didChange` の `contentChanges` と同じ考え方で、変更ごとに、その時点のテキストでの行と列（バイト数）を付ける。LSP プラグインは、これをそのまま差分の同期に使える。
  - 1 回の `apply` の編集は、後ろから順に並べる。後ろの変更は前の位置を動かさないので、どれも変更前のテキストの位置のまま使える。
  - undo と redo も同じ形で届く。
- コアが出すイベントは、どれも WIT の `event` の種類にする。custom イベントは、プラグインどうしの知らせだけに使う（0.5 までは、コアのイベントの一部を custom の形の `editor.*` で出していた。[api-0.6.md](api-0.6.md)）。
- custom イベントの名前には、出したプラグインの名前が自動で付く（`events.emit("counted", ...)` → `wordcount.counted`）。custom イベントはコマンドと対になる仕組み。コマンドは「誰かに頼む」、custom イベントは「起きたことを知らせる」。名前は種類の名前と同じく、ハイフンでつなぐのを勧める。
- イベントは、それを起こした呼び出しが終わってから、起きた順に届ける。イベントを受けたプラグインが出したイベントも、同じ順番の最後に並ぶ。
  - 1 回にさばくイベントは 1,000 個までにする。プラグイン同士がイベントを投げ合って止まらなくなったときに、エディタが固まらないようにするため。超えた分は捨てて、メッセージで知らせる。
- プラグインより前に開いたバッファの `buffer-opened` も、読み込んだあとに届く。起動時は、ファイルを開き、プラグインを読み込んでから、たまったイベントを配るため。途中で有効にしたプラグインは、`buffer.all()` で開いているバッファを調べる。

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
    variant indentation { spaces(u8), tab }
    tab-width: func(buf: borrow<buffer>) -> u8;
    indent: func(buf: borrow<buffer>) -> indentation;
    scroll-margin: func() -> u32;
    /// buf でだけ値を変える。none で config.toml の値に戻す
    set-tab-width: func(buf: borrow<buffer>, width: option<u8>) -> result<_, string>;
    set-indent: func(buf: borrow<buffer>, indent: option<indentation>) -> result<_, string>;
}
```

- 値は、config.toml の `[core]` の値を、プラグインがバッファ単位で上書きしたもの（[architecture.md](architecture.md) の「config.toml」）。
- バッファ単位で変えられるのは `tab-width` と `indent` だけ。`scroll-margin` は画面の設定なので、バッファには持たせない。安全装置（予約キー、プラグインの上限）は、どの形でもプラグインから変えられない。
- 同じバッファの同じキーを複数のプラグインが変えたら、あとから変えたほうが勝つ。変えたプラグインが止まると、その上書きは消える。
- 言語ごとの既定値は、標準プラグイン `indent` が受け持つ。バッファが開いたら言語を調べ、`set-tab-width` と `set-indent` で上書きする。既定では Go をタブ、YAML と JSON を空白 2 つにする。利用者は `plugins/indent.toml` で変えられる。

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
