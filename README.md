# AI Studio CLI

[Google AI Studio](https://aistudio.google.com/) の Playground(チャット)を、ターミナルからそのまま使うための CLI です。macOS / Windows / Linux で動く単一バイナリ(Rust 製)です。

- ストリーミングでのチャット(Markdown・コードブロックのハイライト付き)
- モデル切り替え、Thinking level(minimal / low / medium / high)や thinking budget
- 右パネルの Run settings をほぼすべて再現: System instructions、Structured outputs、Code execution、Function calling、Grounding with Google Search / Google Maps、URL context、Media resolution、Safety settings、Stop sequence、Output length、Temperature / Top-P / Top-K
- 画像・PDF・音声・動画・テキストファイルの添付
- チャット履歴の自動保存と再開(AI Studio の History 相当)、Markdown / JSON へのエクスポート
- 「Get code」相当(今の設定と会話を `curl` コマンドで表示)

## 仕組み

AI Studio の画面そのもの(ブラウザ用の内部 API)を操作するのではなく、AI Studio が裏で使っている **Gemini API** を、AI Studio で発行した API キーで呼び出します。AI Studio の「Get code」で出てくるのと同じ API です。

Web 画面の内部 API は Cookie 認証とブラウザ向けの保護がかかっていて仕様も告知なく変わるため、ラッパーとしてはすぐ壊れてしまいます。また利用規約上もグレーです。そのため公式 API を採用しました。モデルも設定項目も Web 版と同じものが使えます。

> **注意**: Web 版の無料枠と Gemini API の無料枠は別物で、レート制限も異なります。無料枠(課金なし)の API キーで送った内容は、Google のサービス改善に使われることがあります。詳しくは [Gemini API の利用規約](https://ai.google.dev/gemini-api/terms) を確認してください。

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

## 準備: API キー

1. <https://aistudio.google.com/apikey> で API キーを作成(サイドバー左下の鍵アイコン「Get API key」と同じ画面)
2. `aistudio login` を実行してキーを貼り付ける(設定ファイルに保存されます)

環境変数 `GEMINI_API_KEY`(または `GOOGLE_API_KEY`)が設定されていれば、そちらが優先されます。キー未設定のまま `aistudio` を起動した場合も入力を求められます。

## 使い方

### 対話モード

```sh
aistudio
```

```
Google AI Studio CLI 0.1.0
gemini-3.8-flash · thinking: default · tools: search
/help for commands · """ for multi-line input (or Alt+Enter) · Ctrl+C stops a response · Ctrl+D quits

› こんにちは
```

- **Enter** で送信。**Alt+Enter** または **Ctrl+J** で改行。`"""` だけの行で複数行ブロックの開始と終了、行末の `\` で次の行に継続できます
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

出力先がパイプやファイルのときは装飾のない素のテキストだけを標準出力に出し、思考・出典・トークン数は標準エラーに出します。

### コマンド一覧(対話モード)

| コマンド | 内容 |
| --- | --- |
| `/model [名前\|番号]` | モデル切り替え(引数なしで一覧から選択) |
| `/models` | API キーで使えるモデル一覧 |
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
| `/code` | 今の設定と会話を `curl` で表示(Get code) |
| `/key` | API キーを設定し直す |
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

Google Search は AI Studio と同じく最初から有効です。`/settings save` で今の設定を新しいチャットのデフォルトにできます。

### その他のサブコマンド

```sh
aistudio models        # 使えるモデル一覧
aistudio chats         # 保存済みチャット一覧
aistudio config        # 設定ファイルの場所・API キーの取得元・デフォルト設定
aistudio login / logout
```

## 保存場所

| OS | 場所 |
| --- | --- |
| macOS | `~/Library/Application Support/ai-studio-cli/` |
| Linux | `~/.config/ai-studio-cli/`(`$XDG_CONFIG_HOME` を尊重) |
| Windows | `%APPDATA%\ai-studio-cli\` |

`config.json`(API キーとデフォルト設定)、`chats/`(チャット履歴)、`images/`(モデルが生成した画像など)、`input_history.txt` が入ります。環境変数 `AISTUDIO_CLI_HOME` で場所を変えられます。

## 開発

```sh
cargo run              # 対話モード
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

TLS は Windows / macOS では OS 標準(SChannel / Security.framework。社内プロキシの証明書なども OS の設定どおりに信頼)、Linux では rustls + ring を使います。プロキシは `HTTPS_PROXY` などの環境変数と OS の設定に従います。`GEMINI_API_BASE` で API の接続先を差し替えられます(テストや中継用)。

## リリース

1. `Cargo.toml` の `version` を上げて main にマージ
2. GitHub の **Actions → release → Run workflow** を実行(`version` に同じ番号を入れると一致を確認します)
3. 全ターゲットのビルドが揃ったときだけ `v<version>` タグと GitHub Release が作られ、アーカイブ・`SHA256SUMS`・インストールスクリプトが添付されます

ビルド対象(`.github/workflows/reusable-build-matrix.yml`): macOS(arm64 / x86_64)、Windows(x64 / ARM64)、Linux(x86_64 / aarch64 の glibc 版と musl 版)。**Actions → build-check** で、公開せずにビルドだけ確認できます。`1.0.0-beta.1` のようにハイフン付きのバージョンはプレリリースになり、インストールスクリプトの「最新」の対象から外れます。リポジトリが public のときは、ビルドの来歴証明(attestation)も付きます。
