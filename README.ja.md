# decidle

Claude Code の Stop hook。エージェントに報告を JSON で書かせ、決まりと作業記録に合っているかを点検して、合格したら HTML のページにする。人間はターミナルの返事ではなく、そのページを読む。

仕組みと決まりは `docs/report-format.ja.md`、Maestri で使うロールは `docs/role-orchestrator.ja.md` と `docs/role-reviewer.ja.md` に書いてある。

## 新しい PC に入れる

1. このフォルダでビルドして、`~/.local/bin/decidle` に入れる。Rust が要る。

   ```
   cargo install --path . --root ~/.local --locked
   ```

   ソースを直したときも、同じコマンドで入れ直す。入れ直すまで、hook が呼ぶバイナリは変わらない。

2. `~/.claude/settings.json` の `hooks` に、`docs/settings-snippet.json` の項目を足す。どの hook も `~/.local/bin/decidle` を直接呼ぶ。Stop は `decidle stop` で報告を点検する。すべてのツールの PostToolUse は `decidle hint` を呼び、ターンに報告が要るとわかった時点で報告の書き方をエージェントに1回渡す。Stop で差し戻される前に報告を書かせるため。SessionStart、UserPromptSubmit、Notification、SessionEnd と、AskUserQuestion の PreToolUse と PostToolUse は、`decidle event` で Home ページの記録に使う。SessionStart の hook は、返事の最後に Home 用の一言を書く指示をエージェントの文脈に足す。

3. Maestri を使うときだけ、Maestri のターミナルで、このフォルダからロールを登録する。

   ```
   ./install-roles.sh
   ```

`~/.local/bin/decidle` が無い PC では、どの hook も何もせずに終わる。`hooks/stop_decidle.sh` は settings からは使わない。手で `hooks/stop_decidle.sh render <report.json>` と打って報告のページを書き直すときに使う。

## Codex CLI で使う

Codex CLI(0.169 で試した)でも、同じバイナリで報告の点検と Home の記録が動く(SPEC AMENDMENT、2026-11-18 依頼者承認)。

1. `docs/codex-hooks-snippet.toml` の中身を `~/.codex/config.toml` の末尾に足す。
2. codex の中で `/hooks` を開き、足した hook を信頼する。信頼するまで、hook は動かない。

Codex では、許可待ちを PermissionRequest で、承認の拒否と中断を Interrupt で記録する。中断のあとのセッションは表から消える。承認を許可したあとの流れ、圧縮、再開、subagent を待たずに止まった場合などは試していない。

## Herdr で使う

decidle は、hook の環境変数からセッションの端末を見分ける。`HERDR_PANE_ID`(Herdr TUI)、`HC_TERMINAL_ID`(Herdr アプリ)、`CMUX_SURFACE_ID`(cmux)の順に見る。端末が記録されていれば、Home から端末を前に出したり、答えを送ったりできる。このための設定は要らない。

### Herdr アプリに Home を出す

`herdr-mod/decidle/` の mod は、Herdr アプリのターミナルの右に Home の一覧を出す。アプリの mod のフォルダにコピーする。

```sh
mkdir -p ~/.config/herdr-app/mods/decidle
cp herdr-mod/decidle/main.js herdr-mod/decidle/mod.json ~/.config/herdr-app/mods/decidle/
herdr-app check ~/.config/herdr-app/mods/decidle
```

コピーするとアプリが読み込む。消すときは `~/.config/herdr-app/mods/decidle/` を消す。このリポジトリを更新したら、もう一度コピーする。詳しくは `herdr-mod/decidle/README.md` にある。

### `decidle serve` を起動する

mod もブラウザの Home も `decidle serve` を読むので、動かしたままにしておく。Herdr アプリのターミナルで起動する。

```sh
~/.local/bin/decidle serve
```

「端末へ行く」のあと、サーバーは自分を起動したアプリを前に出す。Herdr アプリのターミナルで起動すれば、それは Herdr アプリになる。mod は、ポートを設定の `home_port` から、合言葉を `~/.local/share/decidle/home.token` から読む。合言葉はサーバーが起動のたびに書く。

### 設定

Maestri から Herdr に移るときに、変える設定はない。Maestri 用の設定は、Maestri の外では何もしない。

- `portal_command` は既定で空なので、portal は開かない。
- `trigger_commands`(`maestri ask`)は、ターンで `maestri ask` を実行したときだけ報告を求める。
- `role_dir_marker`、`review_command`、`review_marker` は、Maestri のロールで起動したセッションにだけ効く。オーケストレーター役のレビューの手順は、Maestri の外にはない。

`./install-roles.sh` も要らない。

## 使い方

- 止めたいターミナルは `DECIDLE_MODE=off claude` で起動する。
- 点検の記録は `~/.local/share/decidle/gate.jsonl` に残る。
- 報告を求めるのは、編集したファイルが 3 個以上か、ツールの呼び出しが 20 回以上か、`maestri ask` を実行したターンだけ。数は設定の `edited_files_threshold` と `tool_call_threshold` で変えられる。
- 字数の上限やコード扱いの拡張子は、`config.default.json` と同じ形のファイルを `~/.config/decidle/config.json` に置くと、書いたキーだけ上書きされる。
- 報告の JSON とページは、作業したフォルダの `.decidle/` にできる。git に入れたくなければ、各自の gitignore に足す。
- 報告のページは、Home ページの行の題から開く。Home は `decidle serve` で起動し、`http://127.0.0.1:7878/` で開く(ポートは設定の `home_port`)。
- 報告の JSON とページは、セッションが解決か終了になってから 1 日経つと消える(SPEC AMENDMENT、2026-11-18 依頼者承認)。`decidle serve` が起動時と 1 日 1 回消し、手では `decidle clean` で消せる。`decidle clean --dry-run` は消すものの一覧だけを出す。日数は設定の `clean_after_days` で変えられ、0 にすると消さない。決めてほしいことが答えられないまま終わった報告(終了ボタンで終えたものを含む)は、終了から 7 日経つと消える(SPEC AMENDMENT、2026-11-18 依頼者承認)。日数は `clean_unanswered_after_days` で変えられ、0 にすると消さない。2 つの設定は互いに独立に効く。終わっていないセッションの報告、Home に出ているセッションの報告、記録にないファイル、`.decidle/` の外のファイルは消さない。
- 報告ごとに Maestri の portal を出す機能は、既定で止めてある。戻すには、設定の `portal_command` を `"maestri"` にする。戻すと、Maestri のターミナルでは、ページを書いたあと作業名の portal をキャンバスに出す。まとめ役が起動した作業ターミナル(Maestro ではなく、ほかのエージェントとつながっているターミナル)では出さない。

## 分かっている限界

- 報告のフォルダの場所を確かめてからページを書くまでの間に、同じ権限の別のプロセスがそのフォルダをリンクにすり替えると、ページが別の場所に書かれる。書く直前にもう一度確かめているが、間を無くすことはできていない。
- 会話記録は毎回すべて読む。256MB を超える会話記録は読まず、その回は点検しない。
- 確認のコマンドを実行したかどうかは、コマンドの文字列に語が含まれるかで見ている。実行した結果までは見ていない。
- Bash 経由で書き換えたファイルと、別のターミナルやサブエージェントが編集したファイルは、点検の対象に入らない。
- 点検の記録 `gate.jsonl` は増え続ける。要らなくなったら手で消す。
