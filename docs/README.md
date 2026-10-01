# docs

| 場所 | 内容 |
|------|------|
| [design/](design/) | 設計の記録（日本語）。決めたことと、その理由 |

## design/

| 場所 | 内容 |
|------|------|
| [vision.md](design/vision.md) | 目的と目的外。既存エディタに対して何を変えるか |
| [architecture.md](design/architecture.md) | コアとプラグインの境界、データモデル、描画、プロセスモデル |
| **core/** | |
| [core-menu.md](design/core/core-menu.md) | コアメニュー: 真ん中の箱、絞り込み、プラグインと操作の一覧、詳しい中身 |
| [plugin-buffers.md](design/core/plugin-buffers.md) | プラグインが中身を持つバッファ（magit や dired のようなもの）: 作る、書く、そのバッファでだけ効くキーと、ベースごとの譲り方 |
| **api/** | |
| [plugin-api.md](design/api/plugin-api.md) | WIT の設計方針、イベントとコマンドの流れ、ライフサイクル、権限 |
| [api-0.6.md](design/api/api-0.6.md) | プラグイン API 0.6 の改革: コアの操作とイベントを型に、interface の分け直し、足したもの。1.0 で固めるときの決まり |
| [plugin-dev.md](design/api/plugin-dev.md) | プラグインの開発（`nib plugin new` / `build` / `test`、テストの書き方、作者向けの文書） |
| [plugin-install.md](design/api/plugin-install.md) | プラグインの配布とインストール（`nib plugin search` / `add` / `update` / `remove`） |
| **bases/** | |
| [base.md](design/bases/base.md) | ベースプラグイン（helix、vim、Emacs、nano）の選び方、他のプラグインとのキーの分け方、共通にするもの |
| [helix.md](design/bases/helix.md) | モーダルの扱い、Helix 風キーマップをプラグインで実現するしくみ、実装するキー |
| [vim.md](design/bases/vim.md) | vim ベースのキー、コマンドライン、Neovim との比べ方と違い |
| [emacs.md](design/bases/emacs.md) | Emacs ベースのキー、ミニバッファ、Emacs との比べ方と違い |
| [nano.md](design/bases/nano.md) | nano ベースのキーと、nano との違い |
| **plugins/** | |
| [lsp.md](design/plugins/lsp.md) | LSP プラグインの範囲、サーバーの設定、位置の数え方 |
| [files.md](design/plugins/files.md) | ファイルを開く道具（picker）、ディレクトリの一覧（netrw 風と dired 風）、`nib <ディレクトリ>` と `open-directory`、設定のファイルの雛形 |
| [finder.md](design/plugins/finder.md) | 真ん中の箱で選ぶ: picker の見た目、ファイルのプレビュー、大きなディレクトリでの重さ |
| **project/** | 経過の記録。設計そのものではない |
| [roadmap.md](design/project/roadmap.md) | マイルストーン |
| [benchmarks.md](design/project/benchmarks.md) | vim と Helix との比べ方と結果、nib の起動の内訳、これまでの推移 |
| [distribution.md](design/project/distribution.md) | nib の配布（リリース、mise、Homebrew、Scoop、crates.io） |
