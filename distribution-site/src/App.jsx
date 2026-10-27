import { useEffect, useState } from "react";
import {
  IconArrowDown,
  IconArrowRight,
  IconBrandGithub,
  IconBroadcast,
  IconDeviceDesktop,
  IconDownload,
  IconPlayerPlay,
  IconVideo,
} from "@tabler/icons-react";
import heroArt from "./assets/hero-lavender.png";
import peerLoop from "./assets/peer-loop.png";
import appScreenshot from "./assets/peercast-rt-window-latest.jpg";

const navItems = [
  { id: "top", label: "はじめに" },
  { id: "watch", label: "視聴する" },
  { id: "broadcast", label: "配信する" },
  { id: "licenses", label: "ライセンス" },
  { id: "download", label: "ダウンロード", download: true },
];

export function App() {
  const [activeSection, setActiveSection] = useState("top");
  const year = new Date().getFullYear();

  useEffect(() => {
    const targets = navItems
      .map(({ id }) => document.getElementById(id))
      .filter(Boolean);
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
        if (visible) setActiveSection(visible.target.id);
      },
      { rootMargin: "-28% 0px -58% 0px", threshold: [0, 0.1, 0.35, 0.6] },
    );
    targets.forEach((target) => observer.observe(target));
    return () => observer.disconnect();
  }, []);

  return (
    <>
      <header className="site-header">
        <div className="header-inner">
          <a className="brand" href="#top" aria-label="PeerCast RT はじめに">
            <img src={peerLoop} alt="" className="brand-mark" />
            <span>PeerCast RT</span>
          </a>
          <nav className="main-nav" aria-label="ページ内ナビゲーション">
            {navItems.map(({ id, label, download }) => (
              <a
                key={id}
                href={`#${id}`}
                className={`nav-link${download ? " nav-download" : ""}${activeSection === id ? " is-active" : ""}`}
                aria-current={activeSection === id ? "location" : undefined}
              >
                {download && <IconDownload size={17} stroke={1.8} aria-hidden="true" />}
                {label}
              </a>
            ))}
          </nav>
          <a
            className="source-link"
            href="https://github.com/4getkun/PeerCastStation_Rust"
            target="_blank"
            rel="noopener noreferrer"
          >
            <IconBrandGithub size={20} stroke={1.8} aria-hidden="true" />
            <span>ソースコード</span>
          </a>
        </div>
      </header>

      <main>
        <section id="top" className="hero page-section" aria-labelledby="hero-title">
          <img className="hero-art" src={heroArt} alt="" aria-hidden="true" />
          <div className="hero-inner content-width">
            <div className="hero-copy">
              <p className="eyebrow"><span className="eyebrow-mark" /> PeerCast RT / Windows向け</p>
              <h1 id="hero-title">配信と視聴を、<br />ひとつのアプリで。</h1>
              <p className="hero-lead">
                PeerCast RTは、動画配信をP2Pで中継するWindowsアプリです。
                公開中のチャンネルを選んで見たり、自分の配信を始めたりできます。
              </p>
              <div className="hero-actions">
                <a className="button button-primary" href="#watch">
                  <IconPlayerPlay size={18} stroke={2} aria-hidden="true" />
                  視聴のしかた
                </a>
                <a className="button button-secondary" href="#broadcast">
                  配信のしかた
                  <IconArrowRight size={17} stroke={1.8} aria-hidden="true" />
                </a>
              </div>
              <p className="platform-note">
                <IconDeviceDesktop size={18} stroke={1.7} aria-hidden="true" />
                Windows x64 <span aria-hidden="true">/</span> インストーラー版
              </p>
            </div>
            <figure className="app-preview">
              <img
                className="app-screenshot"
                src={appScreenshot}
                alt="PeerCast RTのチャンネル一覧。架空の配信チャンネルを5件表示した画面。"
              />
              <figcaption>画面は最新ビルドで撮影しています（掲載用の架空チャンネルを使用）。</figcaption>
            </figure>
          </div>
        </section>

        <section id="watch" className="content-section watch-section page-section" aria-labelledby="watch-title">
          <div className="content-width section-grid">
            <div className="section-intro">
              <p className="section-kicker">視聴する</p>
              <h2 id="watch-title">チャンネルを選んで見る。</h2>
              <p>公開中の配信を一覧から選び、設定したプレイヤーで再生します。</p>
              <p>
                YouTubeやTwitchのお気に入りチャンネルも登録できます。アプリの起動中は5分ごとに確認し、生配信が見つかると音と通知でお知らせします。同梱のPCRPlayerで視聴でき、ブラウザで開くこともできます。
              </p>
            </div>
            <div className="step-list">
              <article className="step-row">
                <span className="step-icon"><IconVideo size={23} stroke={1.7} aria-hidden="true" /></span>
                <div>
                  <h3>見たい配信を選ぶ</h3>
                  <p>チャンネル名や配信内容を見て、視聴する配信を選びます。</p>
                </div>
              </article>
              <article className="step-row">
                <span className="step-icon"><IconPlayerPlay size={23} stroke={1.7} aria-hidden="true" /></span>
                <div>
                  <h3>プレイヤーで再生する</h3>
                  <p>一覧の再生ボタンや右クリックメニューからプレイヤーを開きます。YouTubeとTwitchは現在の生配信に対応しています。</p>
                </div>
              </article>
            </div>
          </div>
        </section>

        <section id="broadcast" className="content-section broadcast-section page-section" aria-labelledby="broadcast-title">
          <div className="content-width broadcast-layout">
            <div className="section-intro">
              <p className="section-kicker">配信する</p>
              <h2 id="broadcast-title">自分の配信を始める。</h2>
              <p>エンコーダーから映像をPeerCast RTへ送り、チャンネルとして配信します。</p>
            </div>
            <div className="broadcast-details">
              <div className="input-method">
                <IconBroadcast size={23} stroke={1.7} aria-hidden="true" />
                <div>
                  <h3>配信入力</h3>
                  <p>HTTP POST または HTTP / HTTPS URL</p>
                </div>
              </div>
              <p className="compatibility-note">RTMP入力には対応していません。</p>
            </div>
          </div>
        </section>

        <section id="licenses" className="content-section licenses-section page-section" aria-labelledby="licenses-title">
          <div className="content-width license-layout">
            <div className="section-intro">
              <p className="section-kicker">ライセンス</p>
              <h2 id="licenses-title">GNU GPL v3.0以降で公開しています。</h2>
              <p>
                PeerCast RTのソースコードは、GNU General Public License version 3 or later
                （GPL-3.0-or-later）の条件で公開しています。
              </p>
            </div>
            <div className="license-details">
              <p className="license-identifier">GPL-3.0-or-later</p>
              <p>
                PeerCast RT本体のライセンスは、リポジトリのワークスペース設定に基づいています。
                依存ライブラリや同梱素材には個別の条件があるため、再配布時は各コンポーネントの表示も確認してください。
              </p>
              <a className="license-link" href="https://www.gnu.org/licenses/gpl" target="_blank" rel="noopener noreferrer">
                GNU GPLの本文を読む
                <IconArrowRight size={17} stroke={1.8} aria-hidden="true" />
              </a>
              <div className="license-files">
                <strong>配布ファイル内のライセンス</strong>
                <a href="https://peercast-rt-downloads.fourgetkun.com/licenses/LICENSE-GPL-3.0.txt">PeerCast RT — GPL-3.0-or-later</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/licenses/LICENSE_AUDIT.md">ライセンス確認メモ</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/licenses/PCRPlayer-LICENSE.txt">PCRPlayer</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/licenses/NotoSansJP-OFL.txt">Noto Sans JP — OFL</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/licenses/Tabler-Icons-MIT.txt">Tabler Icons — MIT</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/licenses/yt-dlp/THIRD_PARTY_LICENSES.txt">yt-dlpと依存ライブラリのライセンス表示</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/licenses/PCRPlayer/Node-LICENSE.txt">Node.jsと依存ライブラリのライセンス表示</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/licenses/PCRPlayer/FFmpeg-LGPL-2.1.txt">FFmpeg — LGPL-2.1-or-later</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/PCRPlayer-PeerCastRT-1.2.1-sources.7z">同梱PCRPlayer 対応ソース</a>
                <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/yt-dlp-2026.08.19-sources.7z">yt-dlp 2026.08.19 対応ソース</a>
              </div>
              <div className="third-party-license">
                <strong>このページのアイコン</strong>
                <span>Tabler Icons — MIT License</span>
                <small>Copyright (c) 2020–2026 Paweł Kuna</small>
                <details className="license-notice">
                  <summary>MITライセンス表示</summary>
                  <div>
                    <p>MIT License</p>
                    <p>Copyright (c) 2020–2026 Paweł Kuna</p>
                    <p>
                      Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
                    </p>
                    <p>The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.</p>
                    <p>
                      THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
                    </p>
                  </div>
                </details>
              </div>
            </div>
          </div>
        </section>

        <section id="download" className="download-section page-section" aria-labelledby="download-title">
          <div className="content-width">
            <div className="download-panel">
              <div className="download-copy">
                <p className="section-kicker">ダウンロード</p>
                <h2 id="download-title">PeerCast RT for Windows</h2>
                <p className="download-meta">Windows x64 <span>/</span> バージョン 1.2.1 <span>/</span> 7z形式</p>
                <p className="download-note">インストーラーとライセンス文書をひとつにまとめています。展開して、フォルダー内のセットアップを実行してください。</p>
                <p className="download-checksum">SHA-256: <code>1159e42fe1d7215a205d0123b4163a1c85b561361563a8b80c4a552b079dcfde</code> · <a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/PeerCastRT-1.2.1-windows-x64-installer.7z.sha256">チェックサム</a></p>
                <p className="download-note">セットアップにWindowsの発行元署名はありません。</p>
              </div>
              <div className="download-actions">
              <a className="button button-primary download-button" href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/PeerCastRT-1.2.1-windows-x64-installer.7z">
                <IconArrowDown size={19} stroke={2} aria-hidden="true" />
                Windows版をダウンロード
              </a>
              <small><a href="https://peercast-rt-downloads.fourgetkun.com/releases/1.2.1/yt-dlp-2026.08.19-sources.7z">yt-dlp 2026.08.19 対応ソース（7z）</a></small>
              </div>
            </div>
          </div>
        </section>
      </main>

      <footer className="site-footer">
        <div className="footer-inner content-width">
          <a className="footer-brand" href="#top">
            <img src={peerLoop} alt="" />
            <span>PeerCast RT</span>
          </a>
          <p>Windows向け動画配信ソフトウェア</p>
          <span className="copyright">© {year} PeerCast RT</span>
        </div>
      </footer>
    </>
  );
}
