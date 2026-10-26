# AI Studio CLI

[Google AI Studio](https://aistudio.google.com/) の Playground(チャット)を、ターミナルから使うための CLI です。**Web 版 AI Studio のラッパー**として動き、macOS / Windows / Linux 対応の単一バイナリ(Rust 製)です。

- ストリーミングでのチャット(Markdown・コードブロックのハイライト・Thinking・出典の表示付き)
- モデル切り替え、Thinking level(minimal / low / medium / high)
- 右パネルの Run settings: System instructions、Grounding with Google Search / Google Maps、URL context、Code execution、Structured outputs と Function calling のオン・オフ、Media resolution、Temperature、Top P、Output length、Stop sequence
- ファイル添付、チャットの保存と再開、Markdown / JSON への書き出し
- **Vim キーバインド**(`--vim` / `/vim`)と、`$EDITOR` でのメッセージ作成

## 仕組み

お使いの Chrome(または Edge / Chromium / Brave)を、この CLI 専用のプロファイルで起動し、**本物の AI Studio のページ**(aistudio.google.com)をバックグラウンドで操作します。プロンプトを入力欄に打ち込んで Run を押し、ページに表示される回答を読み取ってターミナルに流します。設定も右パネルのスイッチやメニューを操作して変えます。

- Google アカウントでのログインは、最初に一度 `aistudio login` で開くブラウザ画面で、普段どおり自分で行います。パスワードを CLI が扱うことはありません
- 使っているのは Web 版そのものです。API キーは不要で、Web 版の無料枠・Web 版で選べるモデルがそのまま使えます。チャットは Web 版と同じく AI Studio の履歴(Google ドライブ)にも保存されます
- 普段使いのブラウザのプロファイルには触れません

> **注意**
> - 非公式ツールです。Google の利用規約上、自動操作が問題になる可能性はゼロではありません。ご自身の判断で使ってください。
> - AI Studio の画面は予告なく変わります。変更でボタンなどが見つからなくなったら、`aistudio debug` でページの状態を保存して報告してください(下記「トラブルシューティング」)。
> - Gemini API(API キー)を使う従来の方式も `--api` で使えます。

## インストール

### macOS / Linux

```sh
curl -fsSL https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.sh | sh
```

`~/.local/bin/aistudio` に入ります。Linux 版は静的リンク(musl)なので、ディストリビューションを問わず動きます。

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.ps1 | iex
```

`%LOCALAPPDATA%\Programs\aistudio\aistudio.exe` に入り、ユーザーの PATH に追加されます(新しいターミナルから有効)。x64 と ARM64 に対応しています。

### インストーラーのオプション

| 環境変数 | 内容 |
| --- | --- |
| `AISTUDIO_VERSION` | 入れるバージョン(例: `v0.1.0`)。省略時は最新リリース |
| `AISTUDIO_INSTALL_DIR` | インストール先ディレクトリ |
| `GITHUB_TOKEN` / `GH_TOKEN` | リポジトリが private の間に必要なトークン。`gh auth login` 済みなら `gh auth token` から自動で取得します |

```sh
# 例: バージョンとインストール先を指定
curl -fsSL https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.sh | AISTUDIO_VERSION=v0.1.0 AISTUDIO_INSTALL_DIR=/usr/local/bin sh
```

リポジトリが private の間は、`raw.githubusercontent.com` からのスクリプト取得にも認証が必要です。

```sh
gh api -H "Accept: application/vnd.github.raw" repos/igarinpiano/ai-studio-cli/contents/install.sh | sh
```

インストールスクリプトはリリースの `SHA256SUMS` でアーカイブを検証します。各リリースには `install.sh` / `install.ps1` も添付されます。

### 手動 / ソースから

- [Releases](https://github.com/igarinpiano/ai-studio-cli/releases) からお使いの環境のアーカイブを取得して、中の `aistudio`(`aistudio.exe`)を PATH の通った場所に置く
- または Rust 環境があれば: `cargo install --git https://github.com/igarinpiano/ai-studio-cli`

### アンインストール

バイナリを削除するだけです(macOS / Linux: `rm ~/.local/bin/aistudio`、Windows: `%LOCALAPPDATA%\Programs\aistudio` を削除)。設定と履歴は下記「保存場所」にあります。

## 準備: ログイン

Chrome / Edge / Chromium / Brave のどれかが必要です(自動で探します。別の場所にある場合は環境変数 `AISTUDIO_BROWSER` で実行ファイルを指定)。

```sh
aistudio login
```

1. ブラウザのウィンドウが開くので、Google アカウントでログインする(初回は AI Studio の利用規約への同意なども画面で済ませる)
2. チャット画面が出たらターミナルに戻って Enter

ログイン状態は CLI 専用のブラウザプロファイルに保存され、以後は画面を出さずに動きます。`aistudio logout` でこのプロファイルを削除します。

### API キーで使う場合(`--api`)

`aistudio login --api` で [API キー](https://aistudio.google.com/apikey) を保存するか、環境変数 `GEMINI_API_KEY` を設定し、`aistudio --api` で起動します。常に API を使うなら設定ファイルに `"backend": "api"` と書きます。API モードでは Safety settings、JSON Schema、関数宣言、Top-K、thinking budget、`/retry`、`/undo`、`/code` も使えます。

## 使い方

### 対話モード

```sh
aistudio
```

```
Google AI Studio CLI 0.1.0
via AI Studio in your browser (signed-in Google account)
gemini-3.8-flash · thinking: medium · tools: search
/help for commands · """ or Alt+Enter for multi-line input · /edit opens $EDITOR · Ctrl+C stops a response · Ctrl+D quits

› こんにちは
```

起動時のモデルや設定は、Web 版 AI Studio の右パネルの状態をそのまま読み込みます。

- **Enter** で送信。**Alt+Enter** または **Ctrl+J** で改行。`"""` だけの行で複数行ブロックの開始と終了、行末の `\` で次の行に継続できます
- `/edit` で `$VISUAL` / `$EDITOR`(未設定なら vi、Windows はメモ帳)を開いて長いメッセージを書けます
- **Tab** でコマンド・モデル名・ファイル名を補完
- 生成中の **Ctrl+C** で回答を止めます(途中までの回答は残ります)。**Ctrl+D** か `/exit` で終了
- Thinking のサマリーは薄い文字で、Google Search の検索語と出典は回答の下に、トークン数はフッターに表示されます

### ワンショット

```sh
aistudio "RAII を2文で説明して"
cat main.rs | aistudio -t high "このコードをレビューして"
aistudio -f receipt.jpg --schema receipt.schema.json "項目を抽出して"
aistudio -c "Rust だとどう書く?"        # 直前のチャットの続き
aistudio -r 3 -i                          # 保存済みチャット 3 番を開いて対話モードへ
```

### Vim キーバインド

```sh
aistudio --vim        # この起動だけ
```

対話モードで `/vim on`(`/vim off` で Emacs 風に戻す)とすると設定が保存され、次回からも Vim モードで起動します。入力行が Vi モードになり、`Esc` でノーマルモード、`i` / `a` で挿入、`h` `j` `k` `l` `w` `b` `0` `$` での移動、`x` `dw` `dd` `cw` `p` `u` `.` などの編集、`k` / `j` で入力履歴、`Ctrl+R` で履歴検索が使えます。ノーマルモードで `v` を押すと、入力中の行を `$EDITOR` で開いて編集し、保存して閉じるとそのまま送信します(bash / zsh の vi モードと同じ)。

出力先がパイプやファイルのときは装飾のない素のテキストだけを標準出力に出し、思考・出典・トークン数は標準エラーに出します。

### コマンド一覧(対話モード)

| コマンド | 内容 |
| --- | --- |
| `/model [名前\|番号]` | モデル切り替え(引数なしで一覧から選択) |
| `/models` | 使えるモデル一覧(AI Studio のモデル選択から読み取り) |
| `/thinking [値]` | `default` / `minimal` / `low` / `medium` / `high`、または `off` / `dynamic` / トークン数(2.5 系の thinking budget) |
| `/thoughts on\|off` | 思考サマリーの表示 |
| `/system [テキスト\|clear]` | System instructions(引数なしで複数行入力) |
| `/settings [save\|reset]` | 現在の Run settings を表示。`save` で次回以降のデフォルトに保存 |
| `/set <キー> <値>` | 任意の設定を変更(引数なしでキー一覧) |
| `/tools` | ツールの有効・無効を一覧表示 |
| `/search`, `/maps`, `/url`, `/codeexec`, `/json` `[on\|off]` | Grounding with Google Search / Google Maps / URL context / Code execution / Structured outputs |
| `/schema <ファイル\|JSON\|clear>` | Structured outputs の JSON Schema |
| `/functions <ファイル\|JSON\|clear>` | Function calling の関数宣言。モデルが関数を呼ぶと結果の入力を求めます(AI Studio と同じ流れ) |
| `/safety [カテゴリ 閾値]` | 例: `/safety all none`、`/safety hate high` |
| `/media <default\|low\|medium\|high>` | Media resolution |
| `/temp`, `/maxtokens`, `/stop` | Temperature / Output length / Stop sequence |
| `/attach <ファイル...>` / `/detach` | 次のメッセージにファイルを添付 / 取り消し |
| `/retry` / `/undo` | 最後の回答を再生成 / 最後のやり取りを削除 |
| `/new` | 新しいチャット |
| `/chats` / `/load <番号>` / `/rename <名前>` | 保存済みチャットの一覧 / 開く / 名前変更 |
| `/export <file.md\|file.json>` | チャットを書き出し |
| `/edit` | `$EDITOR` でメッセージを書いて送信 |
| `/vim [on\|off]` | Vim キーバインド(保存されます) |
| `/debug` | AI Studio のページのスクリーンショットと HTML を保存(トラブル報告用) |
| `/code` | 今の設定と会話を API の `curl` で表示(Get code 相当) |
| `/key` | API キーを設定し直す(`--api` 用) |
| `/render markdown\|raw` | 表示形式 |

### Run settings との対応

| AI Studio | 対話モード | コマンドライン |
| --- | --- | --- |
| モデル | `/model` | `-m gemini-3.8-flash` |
| System instructions | `/system` | `-s "..."`, `--system-file FILE` |
| Thinking level | `/thinking medium` | `-t medium` |
| Structured outputs | `/json`, `/schema FILE` | `--json`, `--schema FILE` |
| Code execution | `/codeexec on` | `--code-execution` |
| Function calling | `/functions FILE` | `--functions FILE` |
| Grounding with Google Search | `/search on\|off` | `--search` / `--no-search` |
| Grounding with Google Maps | `/maps on` | `--maps` |
| URL context | `/url on` | `--url-context` |
| Media resolution | `/media low` | `--media-resolution low` |
| Safety settings | `/safety all none` | `--safety all=none` |
| Add stop sequence | `/stop END` | `--stop END` |
| Output length | `/maxtokens 65536` | `--max-tokens 65536` |
| Temperature / Top P / Top K | `/temp 1.0`, `/set top_p 0.95`, `/set top_k 40` | `--temperature`, `--top-p`, `--top-k` |

変更した設定は、次にメッセージを送る直前に AI Studio のページへ反映されます(反映できなかった項目は警告が出ます)。`/settings save` で保存すると起動時に毎回その設定を適用し、`/settings reset` で AI Studio 側の設定に戻します。

Web モードで未対応のもの(API モードでは使えます): Safety settings、Structured outputs の Schema と Function calling の関数宣言の編集(オン・オフは可)、Top-K、thinking budget、`/retry` と `/undo`。これらは `aistudio login` で開くブラウザ画面で直接操作してください。Function calling で関数が呼ばれた場合の応答入力も、今はブラウザ側で行ってください。

### その他のサブコマンド

```sh
aistudio models        # 使えるモデル一覧
aistudio chats         # 保存済みチャット一覧
aistudio config        # 設定ファイルの場所・API キーの取得元・デフォルト設定
aistudio login / logout          # --api で API キーの保存・削除
aistudio debug                   # ページの状態を保存(トラブル報告用)
```

### 主なオプション

| オプション | 内容 |
| --- | --- |
| `--headed` | ブラウザを画面に表示して動かす(ログインが見えないモードで通らない場合や、動きを確認したいとき)。常にそうするなら設定ファイルに `"headless": false` |
| `--api` | Gemini API(API キー)を使う |
| `--vim` / `--emacs` | 入力行のキーバインド |
| `-c` / `-r <番号>` | 前回 / 指定したチャットの続き(AI Studio のそのチャットのページを開きます) |

## 保存場所

| OS | 場所 |
| --- | --- |
| macOS | `~/Library/Application Support/ai-studio-cli/` |
| Linux | `~/.config/ai-studio-cli/`(`$XDG_CONFIG_HOME` を尊重) |
| Windows | `%APPDATA%\ai-studio-cli\` |

`config.json`(設定)、`browser-profile/`(ログイン状態を持つ専用ブラウザプロファイル)、`chats/`(チャット履歴)、`debug/`(`aistudio debug` の出力)、`input_history.txt` が入ります。環境変数 `AISTUDIO_CLI_HOME` で場所を変えられます。

## トラブルシューティング

- **「Not signed in」と出る**: `aistudio login` をやり直してください。ログインしたのに出る場合は `aistudio --headed` で試してください(画面なしのブラウザを受け付けないアカウントがあります)
- **ボタンや設定が見つからない・回答が取れない**: AI Studio の画面が変わった可能性があります。`aistudio debug` を実行すると `screenshot.png` / `page.html` / `outline.txt` / `state.json` が保存されるので、それを添えて報告してください(チャットの内容が含まれるので共有前に確認を)。`AISTUDIO_DEBUG=1` を付けると、回答の読み取り状況を逐次表示します
- **セレクタの一時的な上書き**: 新しいビルドを待たずに直したいときは、保存場所に `web-selectors.json` を置くと、ページの要素の探し方を追加できます(キーは `src/web/agent.js` の `SEL` と同じ。例: `{"runButton": ["button.my-run"]}`)
- **root で動かすコンテナなど**: Chrome がサンドボックスなしでないと起動しない環境では `AISTUDIO_BROWSER_ARGS="--no-sandbox"` を指定します

## 開発

```sh
cargo run              # 対話モード
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Web モードの中身: `src/web/browser.rs` がブラウザを起動し、`src/web/cdp.rs`(Chrome DevTools Protocol)で接続して、`src/web/agent.js` をページに注入して操作します。テストやミラー用に `AISTUDIO_WEB_URL`(開くページ)と `AISTUDIO_BROWSER_ARGS`(ブラウザへの追加引数)で差し替えられます。

API モードの TLS は Windows / macOS では OS 標準(SChannel / Security.framework。社内プロキシの証明書なども OS の設定どおりに信頼)、Linux では rustls + ring を使います。プロキシは `HTTPS_PROXY` などの環境変数と OS の設定に従います。`GEMINI_API_BASE` で API の接続先を差し替えられます(テストや中継用)。

## リリース

1. `Cargo.toml` の `version` を上げて main にマージ
2. GitHub の **Actions → release → Run workflow** を実行(`version` に同じ番号を入れると一致を確認します)
3. 全ターゲットのビルドが揃ったときだけ `v<version>` タグと GitHub Release が作られ、アーカイブ・`SHA256SUMS`・インストールスクリプトが添付されます

ビルド対象(`.github/workflows/reusable-build-matrix.yml`): macOS(arm64 / x86_64)、Windows(x64 / ARM64)、Linux(x86_64 / aarch64 の glibc 版と musl 版)。**Actions → build-check** で、公開せずにビルドだけ確認できます。`1.0.0-beta.1` のようにハイフン付きのバージョンはプレリリースになり、インストールスクリプトの「最新」の対象から外れます。リポジトリが public のときは、ビルドの来歴証明(attestation)も付きます。
