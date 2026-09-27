# docs

設計ドキュメント。日本語で書く。

| ファイル | 内容 |
|----------|------|
| [vision.md](vision.md) | 目的と目的外。既存エディタに対して何を変えるか |
| [architecture.md](architecture.md) | コアとプラグインの境界、データモデル、描画、プロセスモデル |
| [plugin-api.md](plugin-api.md) | WIT の設計方針、イベントとコマンドの流れ、ライフサイクル、権限 |
| [keymap.md](keymap.md) | モーダルの扱い、キーマップをプラグインで実現するしくみ |
| [nano.md](nano.md) | nano ベースのキーと、nano との違い |
| [base.md](base.md) | ベースプラグイン（helix、vim、Emacs、nano）の選び方、他のプラグインとのキーの分け方、共通にするもの |
| [lsp.md](lsp.md) | LSP プラグインの範囲、サーバーの設定、位置の数え方 |
| [plugin-dev.md](plugin-dev.md) | プラグインの開発（`nib plugin new` / `build` / `test`、テストの書き方、作者向けの文書） |
| [plugin-install.md](plugin-install.md) | プラグインの配布とインストール（`nib plugin search` / `add` / `update` / `remove`） |
| [distribution.md](distribution.md) | nib の配布（リリース、mise、Homebrew、Scoop、crates.io） |
| [benchmarks.md](benchmarks.md) | vim と Helix との比べ方と結果、nib の起動の内訳、これまでの推移 |
| [roadmap.md](roadmap.md) | マイルストーン |
| adr/ | 個別の設計判断の記録（`NNNN-title.md`） |
