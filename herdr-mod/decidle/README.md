# decidle の Herdr アプリ用 mod

Herdr アプリのターミナルの右に、`decidle serve` の Home の一覧を出す。

## インストール

```sh
mkdir -p ~/.config/herdr-app/mods/decidle
cp herdr-mod/decidle/main.js herdr-mod/decidle/mod.json ~/.config/herdr-app/mods/decidle/
herdr-app check ~/.config/herdr-app/mods/decidle
```

コピーするとアプリが読み込み、初めてのときはターミナルの右に出る。消すときは `~/.config/herdr-app/mods/decidle/` を消す。

## 使うもの

- `decidle serve` が動いていること。ポートは `~/.config/decidle/config.json` の `home_port` から読み、無ければ 7878 を使う。
- 画面の言葉は、`/api/sessions` の `lang`(設定の `language`)に従う。サーバーが一度も答えていないあいだは、設定の既定値と同じ英語で出す。
- 答えと外すには、サーバーが起動のたびに書く `~/.local/share/decidle/home.token` が要る。

## 確かめ方

```sh
herdr-app check herdr-mod/decidle
```

check はフォルダに `gpui-kit.d.ts` と `jsconfig.json` を書く。どちらもアプリが作るファイルなので、コミットしない。
