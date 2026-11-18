# decidle

Claude Code の Stop hook。エージェントに報告を JSON で書かせ、決まりと作業記録に合っているかを点検して、合格したら HTML のページにする。人間はターミナルの返事ではなく、そのページを読む。

仕組みと決まりは `docs/report-format.md`、Maestri で使うロールは `docs/role-orchestrator.md` と `docs/role-reviewer.md` に書いてある。

## 新しい PC に入れる

1. このフォルダでビルドして、`~/.local/bin/decidle` に入れる。Rust が要る。

   ```
   cargo install --path . --root ~/.local --locked
   ```

   ソースを直したときも、同じコマンドで入れ直す。入れ直すまで、hook が呼ぶバイナリは変わらない。

2. `~/.claude/settings.json` の `hooks` に、`docs/settings-snippet.json` の項目を足す。どの hook も `~/.local/bin/decidle` を直接呼ぶ。Stop は `decidle stop` で報告を点検し、それ以外の SessionStart、UserPromptSubmit、Notification、SessionEnd は `decidle event` で Home ページの記録に使う。SessionStart の hook は、返事の最後に Home 用の一言を書く指示をエージェントの文脈に足す。

3. Maestri を使うなら、Maestri のターミナルで、このフォルダからロールを登録する。

   ```
   ./install-roles.sh
   ```

`~/.local/bin/decidle` が無い PC では、どの hook も何もせずに終わる。`hooks/stop_decidle.sh` は settings からは使わない。手で `hooks/stop_decidle.sh render <report.json>` と打って報告のページを書き直すときに使う。

## 使い方

- 止めたいターミナルは `DECIDLE_MODE=off claude` で起動する。
- 点検の記録は `~/.local/share/decidle/gate.jsonl` に残る。`docs/queries.sql` の SQL で集計できる。
- 報告を求めるのは、編集したファイルが 3 個以上か、ツールの呼び出しが 20 回以上か、`maestri ask` を実行したターンだけ。数は設定の `edited_files_threshold` と `tool_call_threshold` で変えられる。
- 字数の上限やコード扱いの拡張子は、`config.default.json` と同じ形のファイルを `~/.config/decidle/config.json` に置くと、書いたキーだけ上書きされる。
- 報告の JSON とページは、作業したフォルダの `.decidle/` にできる。git に入れたくなければ、各自の gitignore に足す。
- 報告のページは、Home ページの行の題から開く。Home は `decidle serve` で起動し、`http://127.0.0.1:7878/` で開く(ポートは設定の `home_port`)。仕様は `docs/spec-home.md` にある。
- 報告ごとに Maestri の portal を出す機能は、既定で止めてある。戻すには、設定の `portal_command` を `"maestri"` にする。戻すと、Maestri のターミナルでは、ページを書いたあと作業名の portal をキャンバスに出す。まとめ役が起動した作業ターミナル(Maestro ではなく、ほかのエージェントとつながっているターミナル)では出さない。

## 修正依頼の回数を数える

```
~/.local/bin/decidle extract > extract.jsonl
```

出力を `docs/classify-prompt.md` の指示で LLM に分類させ、`docs/queries.sql` の最後の SQL で週ごとに集計する。出力には会話のプロンプトがそのまま入るので、リポジトリには入れない。

## 分かっている限界

- 報告のフォルダの場所を確かめてからページを書くまでの間に、同じ権限の別のプロセスがそのフォルダをリンクにすり替えると、ページが別の場所に書かれる。書く直前にもう一度確かめているが、間を無くすことはできていない。
- 会話記録は毎回すべて読む。256MB を超える会話記録は読まず、その回は点検しない。
- 確認のコマンドを実行したかどうかは、コマンドの文字列に語が含まれるかで見ている。実行した結果までは見ていない。
- Bash 経由で書き換えたファイルと、別のターミナルやサブエージェントが編集したファイルは、点検の対象に入らない。
- 点検の記録 `gate.jsonl` は増え続ける。要らなくなったら手で消す。
