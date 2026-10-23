/// Embedded asset descriptor containing static bytes, MIME type, and ETag (REQ-066).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedAsset {
    pub content: &'static [u8],
    pub mime_type: &'static str,
    pub etag: &'static str,
}

impl EmbeddedAsset {
    pub const fn new(content: &'static [u8], mime_type: &'static str, etag: &'static str) -> Self {
        Self {
            content,
            mime_type,
            etag,
        }
    }
}

// --- Bundled Web Assets (Audited under GPL-3.0-or-later / MIT, REQ-066) ---

const TOKENS_CSS: &[u8] = include_bytes!("../../../ui/css/tokens.css");
const NORD_CSS: &[u8] = include_bytes!("../../../ui/css/themes/nord.css");
const STATUS_CARD_CSS: &[u8] = include_bytes!("../../../ui/css/components/status_card.css");
const CHANNEL_TABLE_CSS: &[u8] = include_bytes!("../../../ui/css/components/channel_table.css");
const ALERT_BANNER_CSS: &[u8] = include_bytes!("../../../ui/css/components/alert_banner.css");
const COMPACT_LAYOUT_CSS: &[u8] = include_bytes!("../../../ui/css/components/compact_layout.css");
const GALLERY_HTML: &[u8] = include_bytes!("../../../ui/fixtures/gallery.html");
const INDEX_FIXTURES_JSON: &[u8] = include_bytes!("../../../ui/fixtures/index_fixtures.json");
const CHANNELS_FIXTURES_JSON: &[u8] = include_bytes!("../../../ui/fixtures/channels_fixtures.json");

/// Standard JavaScript client adapter shim (`/api/1/peercaststation.js`).
/// Matches Station `PeerCastStationJSApp` (GPL-3.0-or-later).
const PEERCASTSTATION_JS: &[u8] = r#"(function() {
  var PeerCastStation = {
    postRequest: function(method, params, completed) {
      var req = {
        jsonrpc: "2.0",
        method: method,
        params: params || {},
        id: Math.floor(Math.random() * 1000000)
      };
      var xhr = new XMLHttpRequest();
      xhr.open("POST", "/api/1", true);
      xhr.setRequestHeader("Content-Type", "application/json");
      xhr.setRequestHeader("X-Requested-With", "XMLHttpRequest");
      xhr.onreadystatechange = function() {
        if (xhr.readyState === 4) {
          if (xhr.status === 200) {
            try {
              var resp = JSON.parse(xhr.responseText);
              if (resp.error) {
                if (completed && completed.error) completed.error(resp.error);
              } else {
                if (completed && completed.success) completed.success(resp.result);
              }
            } catch (e) {
              if (completed && completed.error) completed.error(e);
            }
          } else {
            if (completed && completed.error) completed.error({ code: xhr.status, message: xhr.statusText });
          }
        }
      };
      xhr.send(JSON.stringify(req));
    },
    getVersionInfo: function(cb) { this.postRequest("getVersionInfo", null, cb); },
    getStatus: function(cb) { this.postRequest("getStatus", null, cb); },
    getChannels: function(cb) { this.postRequest("getChannels", null, cb); },
    getSettings: function(cb) { this.postRequest("getSettings", null, cb); },
    setSettings: function(settings, cb) { this.postRequest("setSettings", settings, cb); },
    getPlugins: function(cb) { this.postRequest("getPlugins", null, cb); }
  };
  window.PeerCastStation = PeerCastStation;
})();
"#.as_bytes();

/// Standard index landing page (`/index.html`).
const INDEX_HTML: &[u8] = r#"<!DOCTYPE html>
<html lang="ja" data-theme="default">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>PeerCastStation (Rust)</title>
  <link rel="stylesheet" href="/css/tokens.css">
  <link rel="stylesheet" href="/css/components/status_card.css">
  <link rel="stylesheet" href="/css/components/channel_table.css">
  <link rel="stylesheet" href="/css/components/alert_banner.css">
  <script src="/api/1/peercaststation.js"></script>
</head>
<body style="font-family: var(--peca-font-family); background-color: var(--peca-surface-bg); color: var(--peca-text-primary); margin: 0; padding: var(--peca-space-lg);">
  <header style="display: flex; justify-content: space-between; align-items: center; border-bottom: 2px solid var(--peca-border-subtle); padding-bottom: var(--peca-space-md); margin-bottom: var(--peca-space-lg);">
    <h1 style="margin: 0; font-size: var(--peca-font-size-2xl); color: var(--peca-color-primary);">PeerCastStation (Rust)</h1>
    <nav style="display: flex; gap: var(--peca-space-md);">
      <a href="/index.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">ステータス</a>
      <a href="/channels.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">チャンネル一覧</a>
      <a href="/settings.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">設定</a>
      <a href="/gallery.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">UIギャラリー</a>
    </nav>
  </header>
  <main>
    <div id="status-card" class="peca-card">
      <div class="peca-card-header">
        <h2 class="peca-card-title">ノード情報</h2>
        <span id="node-status-badge" class="peca-badge peca-badge-info">読み込み中...</span>
      </div>
      <div class="peca-grid">
        <div class="peca-metric-item">
          <span class="peca-metric-label">稼働時間</span>
          <span id="metric-uptime" class="peca-metric-value">--:--:--</span>
        </div>
        <div class="peca-metric-item">
          <span class="peca-metric-label">ポート状態</span>
          <span id="metric-port" class="peca-badge peca-badge-warning" style="width: fit-content;">確認中</span>
        </div>
        <div class="peca-metric-item">
          <span class="peca-metric-label">ローカルアドレス</span>
          <span id="metric-local" class="peca-metric-value" style="font-size: var(--peca-font-size-base);">--</span>
        </div>
        <div class="peca-metric-item">
          <span class="peca-metric-label">グローバルアドレス</span>
          <span id="metric-global" class="peca-metric-value" style="font-size: var(--peca-font-size-base);">--</span>
        </div>
      </div>
    </div>
  </main>
  <script>
    document.addEventListener("DOMContentLoaded", function() {
      PeerCastStation.getStatus({
        success: function(status) {
          var hrs = Math.floor(status.uptime / 3600);
          var mins = Math.floor((status.uptime % 3600) / 60);
          var secs = status.uptime % 60;
          document.getElementById("metric-uptime").textContent =
            (hrs < 10 ? "0" : "") + hrs + ":" + (mins < 10 ? "0" : "") + mins + ":" + (secs < 10 ? "0" : "") + secs;
          var badge = document.getElementById("node-status-badge");
          badge.textContent = "稼働中";
          badge.className = "peca-badge peca-badge-success";
          var portBadge = document.getElementById("metric-port");
          if (status.isFirewalled === false) {
            portBadge.textContent = "ポート開放中 (Open)";
            portBadge.className = "peca-badge peca-badge-success";
          } else if (status.isFirewalled === true) {
            portBadge.textContent = "ポート未開放 (Firewalled)";
            portBadge.className = "peca-badge peca-badge-error";
          } else {
            portBadge.textContent = "ポート確認中 (Unknown)";
            portBadge.className = "peca-badge peca-badge-warning";
          }
          if (status.localRelayEndPoint) {
            document.getElementById("metric-local").textContent = status.localRelayEndPoint[0] + ":" + status.localRelayEndPoint[1];
          }
          if (status.globalRelayEndPoint) {
            document.getElementById("metric-global").textContent = status.globalRelayEndPoint[0] + ":" + status.globalRelayEndPoint[1];
          }
        },
        error: function(err) {
          var badge = document.getElementById("node-status-badge");
          badge.textContent = "通信エラー";
          badge.className = "peca-badge peca-badge-error";
        }
      });
    });
  </script>
</body>
</html>
"#.as_bytes();

/// Standard channels management page (`/channels.html`).
const CHANNELS_HTML: &[u8] = r#"<!DOCTYPE html>
<html lang="ja" data-theme="default">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>PeerCastStation - チャンネル一覧</title>
  <link rel="stylesheet" href="/css/tokens.css">
  <link rel="stylesheet" href="/css/components/status_card.css">
  <link rel="stylesheet" href="/css/components/channel_table.css">
  <link rel="stylesheet" href="/css/components/alert_banner.css">
  <script src="/api/1/peercaststation.js"></script>
</head>
<body style="font-family: var(--peca-font-family); background-color: var(--peca-surface-bg); color: var(--peca-text-primary); margin: 0; padding: var(--peca-space-lg);">
  <header style="display: flex; justify-content: space-between; align-items: center; border-bottom: 2px solid var(--peca-border-subtle); padding-bottom: var(--peca-space-md); margin-bottom: var(--peca-space-lg);">
    <h1 style="margin: 0; font-size: var(--peca-font-size-2xl); color: var(--peca-color-primary);">チャンネル一覧</h1>
    <nav style="display: flex; gap: var(--peca-space-md);">
      <a href="/index.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">ステータス</a>
      <a href="/channels.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">チャンネル一覧</a>
      <a href="/settings.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">設定</a>
      <a href="/gallery.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">UIギャラリー</a>
    </nav>
  </header>
  <main>
    <div class="peca-table-container">
      <table class="peca-table">
        <thead>
          <tr>
            <th scope="col">チャンネル名</th>
            <th scope="col">ステータス</th>
            <th scope="col">ビットレート</th>
            <th scope="col">リレー / 直接</th>
            <th scope="col">操作</th>
          </tr>
        </thead>
        <tbody id="channels-table-body">
          <tr>
            <td colspan="5" class="peca-empty-state">チャンネル情報を読み込んでいます...</td>
          </tr>
        </tbody>
      </table>
    </div>
  </main>
  <script>
    document.addEventListener("DOMContentLoaded", function() {
      PeerCastStation.getChannels({
        success: function(channels) {
          var tbody = document.getElementById("channels-table-body");
          tbody.innerHTML = "";
          if (!channels || channels.length === 0) {
            tbody.innerHTML = '<tr><td colspan="5" class="peca-empty-state"><div class="peca-empty-icon">&#128250;</div>現在アクティブなチャンネルはありません</td></tr>';
            return;
          }
          channels.forEach(function(ch) {
            var tr = document.createElement("tr");
            tr.innerHTML = '<td><div class="peca-channel-name">' + ch.name + '</div><div class="peca-channel-id">' + ch.channelId + '</div></td>' +
              '<td><span class="peca-badge peca-badge-info">' + ch.status + '</span></td>' +
              '<td>' + ch.bitrate + ' kbps</td>' +
              '<td>' + ch.totalRelays + ' / ' + ch.totalDirects + '</td>' +
              '<td><a href="/stream/' + ch.channelId + '" class="peca-btn peca-btn-outline" style="text-decoration: none;">視聴</a></td>';
            tbody.appendChild(tr);
          });
        },
        error: function(err) {
          var tbody = document.getElementById("channels-table-body");
          tbody.innerHTML = '<tr><td colspan="5" class="peca-empty-state" style="color: var(--peca-state-error-text);">チャンネルの取得に失敗しました</td></tr>';
        }
      });
    });
  </script>
</body>
</html>
"#.as_bytes();

/// Standard settings management page (`/settings.html`).
const SETTINGS_HTML: &[u8] = r#"<!DOCTYPE html>
<html lang="ja" data-theme="default">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>PeerCastStation - 設定</title>
  <link rel="stylesheet" href="/css/tokens.css">
  <link rel="stylesheet" href="/css/components/status_card.css">
  <link rel="stylesheet" href="/css/components/alert_banner.css">
  <script src="/api/1/peercaststation.js"></script>
</head>
<body style="font-family: var(--peca-font-family); background-color: var(--peca-surface-bg); color: var(--peca-text-primary); margin: 0; padding: var(--peca-space-lg);">
  <header style="display: flex; justify-content: space-between; align-items: center; border-bottom: 2px solid var(--peca-border-subtle); padding-bottom: var(--peca-space-md); margin-bottom: var(--peca-space-lg);">
    <h1 style="margin: 0; font-size: var(--peca-font-size-2xl); color: var(--peca-color-primary);">サーベント設定</h1>
    <nav style="display: flex; gap: var(--peca-space-md);">
      <a href="/index.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">ステータス</a>
      <a href="/channels.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">チャンネル一覧</a>
      <a href="/settings.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">設定</a>
      <a href="/gallery.html" class="peca-btn peca-btn-outline" style="text-decoration: none;">UIギャラリー</a>
    </nav>
  </header>
  <main>
    <div id="settings-card" class="peca-card">
      <div class="peca-card-header">
        <h2 class="peca-card-title">リレー・帯域設定</h2>
      </div>
      <form id="settings-form" style="display: flex; flex-direction: column; gap: var(--peca-space-md); max-width: 400px;">
        <div>
          <label for="input-max-relays" style="display: block; font-weight: 600; margin-bottom: 4px;">最大リレー数:</label>
          <input type="number" id="input-max-relays" min="0" max="1000" style="width: 100%; padding: var(--peca-space-xs); border: 1px solid var(--peca-border-strong); border-radius: var(--peca-radius-sm);">
        </div>
        <div>
          <label for="input-max-directs" style="display: block; font-weight: 600; margin-bottom: 4px;">最大直接接続数:</label>
          <input type="number" id="input-max-directs" min="0" max="500" style="width: 100%; padding: var(--peca-space-xs); border: 1px solid var(--peca-border-strong); border-radius: var(--peca-radius-sm);">
        </div>
        <div>
          <label for="input-max-upstream" style="display: block; font-weight: 600; margin-bottom: 4px;">最大上り帯域 (kbps):</label>
          <input type="number" id="input-max-upstream" min="0" style="width: 100%; padding: var(--peca-space-xs); border: 1px solid var(--peca-border-strong); border-radius: var(--peca-radius-sm);">
        </div>
        <button type="button" onclick="saveSettings()" class="peca-btn" style="width: fit-content; margin-top: var(--peca-space-sm);">設定を保存</button>
      </form>
      <div id="settings-msg" style="margin-top: var(--peca-space-md);"></div>
    </div>
  </main>
  <script>
    document.addEventListener("DOMContentLoaded", function() {
      PeerCastStation.getSettings({
        success: function(settings) {
          document.getElementById("input-max-relays").value = settings.maxRelays;
          document.getElementById("input-max-directs").value = settings.maxDirects;
          document.getElementById("input-max-upstream").value = settings.maxUpstreamRate;
        },
        error: function(err) {
          document.getElementById("settings-msg").innerHTML = '<div class="peca-alert peca-alert-error">設定の読み込みに失敗しました</div>';
        }
      });
    });

    function saveSettings() {
      var msg = document.getElementById("settings-msg");
      var newSettings = {
        maxRelays: parseInt(document.getElementById("input-max-relays").value, 10),
        maxDirects: parseInt(document.getElementById("input-max-directs").value, 10),
        maxUpstreamRate: parseInt(document.getElementById("input-max-upstream").value, 10)
      };
      PeerCastStation.setSettings(newSettings, {
        success: function() {
          msg.innerHTML = '<div class="peca-alert peca-alert-success">設定を保存しました</div>';
        },
        error: function(err) {
          msg.innerHTML = '<div class="peca-alert peca-alert-error">保存に失敗しました: ' + (err.message || "Unauthorized") + '</div>';
        }
      });
    }
  </script>
</body>
</html>
"#.as_bytes();

/// Maps a normalized request path to its audited embedded asset (REQ-066).
pub fn get_embedded_asset(path: &str) -> Option<EmbeddedAsset> {
    // Normalize path
    let clean_path = path.trim_start_matches('/');
    let normalized = match clean_path {
        "" | "index.html" | "html/index.html" => "index.html",
        "channels.html" | "html/channels.html" => "channels.html",
        "settings.html" | "html/settings.html" => "settings.html",
        "gallery.html" | "ui/fixtures/gallery.html" => "gallery.html",
        "api/1/peercaststation.js" => "api/1/peercaststation.js",
        "css/tokens.css" | "ui/css/tokens.css" => "css/tokens.css",
        "css/themes/nord.css" | "ui/css/themes/nord.css" => "css/themes/nord.css",
        "css/components/status_card.css" | "ui/css/components/status_card.css" => {
            "css/components/status_card.css"
        }
        "css/components/channel_table.css" | "ui/css/components/channel_table.css" => {
            "css/components/channel_table.css"
        }
        "css/components/alert_banner.css" | "ui/css/components/alert_banner.css" => {
            "css/components/alert_banner.css"
        }
        "css/components/compact_layout.css" | "ui/css/components/compact_layout.css" => {
            "css/components/compact_layout.css"
        }
        "fixtures/index_fixtures.json" | "ui/fixtures/index_fixtures.json" => {
            "fixtures/index_fixtures.json"
        }
        "fixtures/channels_fixtures.json" | "ui/fixtures/channels_fixtures.json" => {
            "fixtures/channels_fixtures.json"
        }
        other => other,
    };

    match normalized {
        "index.html" => Some(EmbeddedAsset::new(
            INDEX_HTML,
            "text/html; charset=utf-8",
            "\"peca-idx-1.0\"",
        )),
        "channels.html" => Some(EmbeddedAsset::new(
            CHANNELS_HTML,
            "text/html; charset=utf-8",
            "\"peca-chn-1.0\"",
        )),
        "settings.html" => Some(EmbeddedAsset::new(
            SETTINGS_HTML,
            "text/html; charset=utf-8",
            "\"peca-set-1.0\"",
        )),
        "gallery.html" => Some(EmbeddedAsset::new(
            GALLERY_HTML,
            "text/html; charset=utf-8",
            "\"peca-gal-1.0\"",
        )),
        "api/1/peercaststation.js" => Some(EmbeddedAsset::new(
            PEERCASTSTATION_JS,
            "application/javascript; charset=utf-8",
            "\"peca-js-1.0\"",
        )),
        "css/tokens.css" => Some(EmbeddedAsset::new(
            TOKENS_CSS,
            "text/css; charset=utf-8",
            "\"peca-tok-1.0\"",
        )),
        "css/themes/nord.css" => Some(EmbeddedAsset::new(
            NORD_CSS,
            "text/css; charset=utf-8",
            "\"peca-nrd-1.0\"",
        )),
        "css/components/status_card.css" => Some(EmbeddedAsset::new(
            STATUS_CARD_CSS,
            "text/css; charset=utf-8",
            "\"peca-sc-1.0\"",
        )),
        "css/components/channel_table.css" => Some(EmbeddedAsset::new(
            CHANNEL_TABLE_CSS,
            "text/css; charset=utf-8",
            "\"peca-ct-1.0\"",
        )),
        "css/components/alert_banner.css" => Some(EmbeddedAsset::new(
            ALERT_BANNER_CSS,
            "text/css; charset=utf-8",
            "\"peca-ab-1.0\"",
        )),
        "css/components/compact_layout.css" => Some(EmbeddedAsset::new(
            COMPACT_LAYOUT_CSS,
            "text/css; charset=utf-8",
            "\"peca-cl-1.0\"",
        )),
        "fixtures/index_fixtures.json" => Some(EmbeddedAsset::new(
            INDEX_FIXTURES_JSON,
            "application/json; charset=utf-8",
            "\"peca-ifx-1.0\"",
        )),
        "fixtures/channels_fixtures.json" => Some(EmbeddedAsset::new(
            CHANNELS_FIXTURES_JSON,
            "application/json; charset=utf-8",
            "\"peca-cfx-1.0\"",
        )),
        _ => None,
    }
}

/// Type alias for embedded asset HTTP response tuple (status, headers, optional body).
pub type AssetHttpResponse = (u16, Vec<(&'static str, String)>, Option<&'static [u8]>);

/// Builds an HTTP response for an embedded asset, honoring ETags (304 Not Modified)
/// and injecting security headers (nosniff, sameorigin, CSP) (REQ-066).
pub fn build_asset_http_response(
    asset: &EmbeddedAsset,
    if_none_match: Option<&str>,
) -> AssetHttpResponse {
    let mut headers = vec![
        ("Content-Type", asset.mime_type.to_string()),
        ("ETag", asset.etag.to_string()),
        ("X-Content-Type-Options", "nosniff".to_string()),
        ("X-Frame-Options", "SAMEORIGIN".to_string()),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'".to_string(),
        ),
    ];

    // Conditional GET: 304 Not Modified
    if let Some(inm) = if_none_match {
        if inm.trim() == asset.etag {
            headers.push(("Content-Length", "0".to_string()));
            return (304, headers, None);
        }
    }

    headers.push(("Content-Length", asset.content.len().to_string()));
    (200, headers, Some(asset.content))
}
