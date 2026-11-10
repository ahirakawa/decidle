# report-gate

Claude Code の Stop hook。エージェントに報告を JSON で書かせ、決まりと作業記録に合っているかを点検して、合格したら HTML のページにする。人間はターミナルの返事ではなく、そのページを読む。

仕組みと決まりは `docs/report-format.md`、Maestri で使うロールは `docs/role-orchestrator.md` と `docs/role-reviewer.md` に書いてある。

## 新しい PC に入れる

1. このフォルダでビルドして、`~/.local/bin/report-gate` に入れる。Rust が要る。

   ```
   cargo install --path . --root ~/.local --locked
   ```

   ソースを直したときも、同じコマンドで入れ直す。入れ直すまで、hook が呼ぶバイナリは変わらない。

2. hook のスクリプト `../hooks/stop_report_gate.sh` を `~/.claude/hooks/` に置く。hooks のフォルダごとリンクしてあるなら、すでに置かれている。

   ```
   ls ~/.claude/hooks/stop_report_gate.sh
   ```

3. `~/.claude/settings.json` の `hooks.Stop` に、`docs/settings-snippet.json` の 1 項目を足す。

4. Maestri を使うなら、Maestri のターミナルで、このフォルダからロールを登録する。

   ```
   ./install-roles.sh
   ```

`~/.local/bin/report-gate` が無い PC では、hook のスクリプトは何もせずに終わる。

## 使い方

- 止めたいターミナルは `REPORT_GATE_MODE=off claude` で起動する。
- 点検の記録は `~/.local/share/agent-report-gate/gate.jsonl` に残る。`docs/queries.sql` の SQL で集計できる。
- 字数の上限やコード扱いの拡張子は、`config.default.json` と同じ形のファイルを `~/.config/agent-report-gate/config.json` に置くと、書いたキーだけ上書きされる。
- 報告の JSON とページは、作業したフォルダの `.report-gate/` にできる。git に入れたくなければ、各自の gitignore に足す。

## 修正依頼の回数を数える

```
~/.local/bin/report-gate extract > extract.jsonl
```

出力を `docs/classify-prompt.md` の指示で LLM に分類させ、`docs/queries.sql` の最後の SQL で週ごとに集計する。出力には会話のプロンプトがそのまま入るので、リポジトリには入れない。

## 分かっている限界

- 報告のフォルダの場所を確かめてからページを書くまでの間に、同じ権限の別のプロセスがそのフォルダをリンクにすり替えると、ページが別の場所に書かれる。書く直前にもう一度確かめているが、間を無くすことはできていない。
- 会話記録は毎回すべて読む。256MB を超える会話記録は読まず、その回は点検しない。
- 確認のコマンドを実行したかどうかは、コマンドの文字列に語が含まれるかで見ている。実行した結果までは見ていない。
- Bash 経由で書き換えたファイルと、別のターミナルやサブエージェントが編集したファイルは、点検の対象に入らない。
- 点検の記録 `gate.jsonl` は増え続ける。要らなくなったら手で消す。
