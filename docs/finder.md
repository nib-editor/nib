# 真ん中の箱で選ぶ（picker の見た目と重さ）

> ステータス: 合意済み（2026-09-29）。

picker（`picker.files`、`picker.commands`）を、コアメニュー（[core-menu.md](core-menu.md)）と同じく、画面の真ん中の箱に出す。右には、選んでいるファイルの中身を構文の色付きで出す（helix のプレビュー）。あわせて、ホームのような大きなディレクトリで開いたときの重さを軽くする。

## 見た目

```
╭─ files ───────────────────────────────────────────────────────────╮
│ files> main                                              3/1204   │
├──────────────────────────────┬────────────────────────────────────┤
│ src/main.rs                  │ fn main() {                        │
│ bench/main.py                │     let config = Config::load()?;  │
│ tui/src/main.rs              │     ...                            │
╰──────────────────────────────┴────────────────────────────────────╯
```

- 形は、コアメニューの箱と同じ（大きさ、枠、題、上の入力の行、左の一覧、右の欄）。描くのもコアの同じ処理。
- 入力の行は、プラグインの入力欄（`prompt.line`）そのもの。キーは今までどおりベースが受け、入力欄を編集し、操作（次、前、決定など）をプラグインに知らせる。どのベースでも、そのベースの作法で動く。
- 数（`3/1204`）は入力欄の `hint`。
- 一覧で合った文字の色は、プラグインが `span` で付ける（`ui.popup.key`）。

## API

入力欄（`prompt.line`）に、箱に出すためのメソッドを足す。

```wit
variant preview {
    none,
    /// Lines of text, such as a command's description.
    lines(list<styled-line>),
    /// The start of a file, colored as its language, or from a line of it.
    file(file-preview),
}

record file-preview { path: string, line: option<u64> }

resource line {
    ...
    /// Shows it as the input of a box in the middle of the screen, titled
    /// `title`, with the rows of `set-rows` under it and `set-preview`
    /// beside them. Its hint shows at the right of the input.
    show-in-box: func(title: string);
    /// The rows under the input, and the one selected. A window of rows
    /// around the selected one is enough.
    set-rows: func(rows: list<styled-line>, selected: option<u32>);
    set-preview: func(preview: preview);
}
```

- 箱に出していない入力欄は、今までどおり画面の下に出る。`set-rows` と `set-preview` は、箱に出していなければ何もしない。
- 一覧は、プラグインが全部を渡さなくてよい。選んでいる行のまわり（数百行）を渡せば、コアが箱の高さに合わせて、選んでいる行が見えるように出す。何十万件もの候補を毎回渡さないため。
- ファイルのプレビューは、コアがファイルを読み、言語を調べ、構文を解析して色を付ける。プラグインはパスを渡すだけ。
  - 読むのは先頭の 128 KiB まで（行の途中で切らない）。NUL を含むものは「binary file」と出す。
  - 同じパスが続けて来たら、読み直さない（選ぶ行を動かすたびに解析しないように）。
  - 中身はプラグインに渡らないので、権限（`fs-read`）は要らない。
  - `line` があれば、その行が上から 3 分の 1 あたりに来るように出し、その行に選択の色を付ける（grep の結果などのため）。
- 関数を足すだけなので `nib:plugin@0.7.2`。SDK も 0.7.2。

## 重さ

今は、`files.walk` の結果が 1,000 件届くたびに、それまでの全部を絞り込み直して並べ直している。ホーム（数十万ファイル）では、届くたびに全部をやり直すので、合わせると件数の 2 乗に近い手間になる。数えるのはコアの裏のスレッドなので、重いのは picker の側。

- 届いた分だけを絞り込み、それまでの結果に足す。
- 並べ直しは、見せる分（上から数百件）だけにする。全部を並べるのは、選ぶ行がそこより下に行ったときだけ。
- 何も打っていないときは、点数を付けず、届いた順のまま足す。
- 打った文字が前の文字の続き（`ma` → `mai`）なら、前に合ったものの中だけを絞り込み直す。

数える数や深さの上限は付けない（今回は付けないと決めた）。数えている間も打てるし、打った分はすぐ反映する。

15 万ファイル（1,500 ディレクトリ）で測った（2026-09-29、release ビルド、`nib-editor-core` のテストから picker を動かした）。

| | 前 | 後 |
|---|---|---|
| 何も打たずに開き、数え終わるまで | 0.24 秒、1 フレームは最長 6 ms | 0.23 秒、最長 0.6 ms |
| 開いてすぐ `file_1` と打ち、数え終わるまで | 3.3 秒、そのうち 1 フレームで 3.0 秒止まる | 0.23 秒、最長 3.4 ms |
| 数え終わったあと 1 文字打つ | 25〜45 ms | 12〜41 ms |

## 版

- nib は 0.12.3。picker は 0.2.0。
- API は `nib:plugin@0.7.2`。SDK の Rust と Go は 0.7.2 のタグを打つ。

## 入れないもの

- 箱の大きさや位置をプラグインが決めること。
- プレビューを動かす（スクロールする）キー。
- 数える数と深さの上限。
