# Data Traffic Manager

ネットワークアダプターごとの通信速度と通信量（データ使用量）を監視する Windows 向けデスクトップアプリです。
Rust と [Slint](https://slint.dev/) で作られています。

Windows 11 で WireGuard などの仮想アダプターの通信量がタスクマネージャーや既存ツールで見られなくなった問題に対応するため、
OS のパフォーマンスカウンターではなく IP Helper API の `GetIfTable2` から NDIS の 64 ビットカウンターを直接読み取ります。
WireGuard（WireGuardNT）、Wintun、TAP などの VPN アダプターも、有線 LAN や Wi-Fi と同じように計測できます。

## 主な機能

- **リアルタイム監視** — インターフェースごとの受信・送信速度、直近 2 分間のグラフ（ホバーで各時点の値を表示）、最大速度
- **通信量の集計** — このセッション／今日／今期（月ごと）の通信量。日別（直近 30 日）と月別（12 か月）の履歴
- **VPN の履歴を保持** — WireGuard のトンネルを切断してアダプターが消えても、トンネル名ごとに履歴が残ります
- **月間の上限** — インターフェースごとに上限（GB）を設定し、使用率と期間末の予測を表示。80% で警告、超過で赤表示
- **締め日の設定** — 毎月の集計開始日（1〜28 日）を契約の締め日に合わせられます
- **停止中の通信量も集計** — 次回起動時にアダプターのカウンターから停止中の通信量を求めて加算します（過大に数えることはありません）
- **タスクトレイ常駐** — ウィンドウを閉じてもトレイで計測を継続。トレイアイコンにカーソルを合わせると現在の速度を表示
- **Windows のサインイン時に自動起動**（トレイに格納された状態で起動）、多重起動の防止
- ライト／ダークモード対応（Windows の設定に追従）、速度の単位は MB/s と Mbps を切り替え可能

## 動作環境

- Windows 10 / 11（x64）
- 開発用に Linux でも動作します（`/sys/class/net` から取得）

## インストール

[Releases](https://github.com/SHIN-DATA-CENTER/Data-Traffic-Manager/releases/latest) から次のどちらかをダウンロードします。

| ファイル | 内容 |
| --- | --- |
| `DataTrafficManager-<バージョン>-setup-x64.exe` | インストーラー（おすすめ）。`C:\Program Files\Data Traffic Manager` にインストールし、スタートメニューに登録します |
| `data-traffic-manager-<バージョン>-portable-x64.exe` | インストール不要の単体版。任意のフォルダーに置いて実行します |

実行ファイルにはコード署名をしていないため、初回は「Windows によって PC が保護されました」と表示されることがあります。
その場合は [詳細情報] → [実行] を選んでください。ダウンロードしたファイルは `SHA256SUMS.txt` で検証できます。

- **更新:** 新しいインストーラーをそのまま実行します。起動中のアプリは自動で終了し（記録中の通信量は保存されます）、同じ場所に上書きされます。
- **アンインストール:** Windows の「設定」→「アプリ」から削除します。使用量の履歴と設定を残すかどうかを選べます。
- **サイレントインストール:** `DataTrafficManager-<バージョン>-setup-x64.exe /S`（インストール先は `/D=C:\path` で指定。必ず最後の引数にします）。
  サイレントアンインストールは `"C:\Program Files\Data Traffic Manager\uninstall.exe" /S` で、使用量の履歴は残ります。

## ビルド

[Rust](https://rustup.rs/)（1.92 以降）をインストールしてから:

```powershell
git clone https://github.com/SHIN-DATA-CENTER/Data-Traffic-Manager.git
cd Data-Traffic-Manager
cargo build --release
```

`target\release\data-traffic-manager.exe` が生成されます。単体で動作するので、好きな場所にコピーして使えます。

インストーラーは [NSIS](https://nsis.sourceforge.io/)（3.x）で作成します。

```powershell
makensis /DVERSION=0.1.0 installer\installer.nsi   # dist\DataTrafficManager-0.1.0-setup-x64.exe
```

Linux でビルドする場合は X11/Wayland 関連のライブラリ（`libxkbcommon-x11`、`libfontconfig` など）が必要です。

### リリース手順

1. `Cargo.toml` の `version` と `CHANGELOG.md` を更新してコミットします。
2. `v<バージョン>` のタグ（例: `v0.1.0`）を push します。
3. GitHub Actions がテスト・インストーラーの動作確認を行い、インストーラー・単体版・チェックサムを添付したリリースを作成します。
   リリースノートには `CHANGELOG.md` の該当バージョンの節が使われます。

## 使い方

1. スタートメニューの「Data Traffic Manager」（単体版は exe）を起動します。管理者権限は不要です。
2. 左の一覧から監視したいインターフェースを選びます。VPN アダプターは一覧の上に表示されます。
3. 「モニター」タブで現在の速度とグラフ、「使用量の履歴」タブで日別・月別の通信量と月間の上限を確認・設定できます。
4. 右上の設定ボタンから、更新間隔・単位・締め日・トレイ常駐・自動起動などを変更できます。

既定では、ループバックや NDIS フィルター（`…-WFP Native MAC Layer LightWeight Filter-0000` など）、
Teredo などの IPv6 移行用の疑似インターフェース、一度も使われていない切断中のアダプターは表示されません。
一覧下部の「非表示のインターフェースも表示」で、すべてを表示できます。

### 計測の仕組みと注意点

- 通信量はアダプターの累積カウンター（受信・送信バイト数）の差分から求めています。
- トンネルの再接続などでカウンターが 0 に戻った場合も、0 からの増加分として正しく加算します。
- 「アプリ停止中の通信量も集計する」が有効な場合、停止中の通信量は**次に起動した日**の使用量として加算されます。
  再起動をまたいだ場合は、起動後の通信量のみが加算されます（停止中の通信量を多く見積もることはありません）。
- VPN の通信は、物理アダプター（有線 LAN / Wi-Fi）側でも暗号化されたパケットとして計測されます。
  そのため、VPN と物理アダプターの通信量を合計すると二重に数えることになります。

## データの保存先

`%LOCALAPPDATA%\Data-Traffic-Manager` に保存されます（設定画面の「フォルダーを開く」から開けます）。

| ファイル | 内容 |
| --- | --- |
| `usage.json` | インターフェースごとの日別の通信量と、最後に読み取ったカウンター値 |
| `settings.json` | 設定（更新間隔、単位、締め日、月間の上限など） |

環境変数 `DTM_DATA_DIR` で保存先を変更できます（USB メモリなどでのポータブル利用向け）。
ファイルは 30 秒ごとと終了時に保存されます。
`data-traffic-manager.exe --quit` を実行すると、起動中のアプリがデータを保存して終了します（インストーラーが更新・削除の前に使用します）。読み込めないファイルは `*.broken` に退避され、上書きされることはありません。

## 開発

```sh
cargo test                                  # ユニットテスト
cargo run --example list_interfaces         # 取得できるインターフェースの一覧（トラブルシューティング用）
cargo clippy --all-targets
```

Windows 向けコードは Linux からクロスコンパイルし、Wine 上でテストすることもできます。

```sh
rustup target add x86_64-pc-windows-gnu     # mingw-w64 と wine も必要
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --lib --target x86_64-pc-windows-gnu
```

### 構成

| パス | 内容 |
| --- | --- |
| `src/platform/` | OS ごとのカウンター取得（Windows: `GetIfTable2` / `GetAdaptersAddresses`、Linux: sysfs） |
| `src/monitor.rs` | サンプル間の差分から速度・セッション通信量を計算し、日別の使用量に加算 |
| `src/usage.rs` | 日別の使用量の保存、締め日に基づく集計期間 |
| `src/app.rs` | 設定・履歴・モニターを束ね、画面に表示する内容（ビューモデル）を作成 |
| `src/chart.rs` | グラフの座標（SVG パス）と目盛りの計算 |
| `src/desktop.rs` | 多重起動の防止、自動起動（レジストリ）、フォルダーを開く |
| `src/main.rs` | Slint の画面との接続。計測はバックグラウンドスレッドで行います |
| `ui/` | Slint の画面定義 |
| `installer/installer.nsi` | NSIS インストーラーのスクリプト |

## ライセンス

[MIT License](LICENSE)
