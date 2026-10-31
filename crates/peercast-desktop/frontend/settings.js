(() => {
  "use strict";

  function normalizeUpdateStatus(value = {}) {
    const phases = ["idle", "checking", "upToDate", "notPublished", "available", "downloading", "ready", "applying", "error"];
    const phase = phases.includes(value?.phase) ? value.phase : "error";
    const count = (number) => Number.isSafeInteger(number) && number >= 0 ? number : null;
    return {
      phase,
      currentVersion: typeof value?.currentVersion === "string" ? value.currentVersion : "",
      version: typeof value?.version === "string" ? value.version : null,
      notes: typeof value?.notes === "string" ? value.notes : null,
      receivedBytes: count(value?.receivedBytes) ?? 0,
      totalBytes: count(value?.totalBytes),
      error: typeof value?.error === "string" ? value.error : null,
    };
  }
  function updateCardModel(status) {
    const phase = status?.phase || "idle";
    const busy = ["checking", "downloading", "applying"].includes(phase);
    const labels = {
      idle: "更新を確認できます。", checking: "更新を確認しています…", upToDate: "最新バージョンを使用しています。",
      notPublished: "現在、更新版は公開されていません。", available: `バージョン ${status?.version || ""} を利用できます。`,
      downloading: "更新データを取得しています…", ready: `バージョン ${status?.version || ""} の更新データを取得しました。`,
      applying: "更新を適用しています…", error: "更新を確認できませんでした。通信状態を確認して再試行してください。",
    };
    const total = status?.totalBytes;
    const received = Number(status?.receivedBytes) || 0;
    const hasProgress = phase === "downloading" || phase === "ready";
    const percent = phase === "ready" ? 100
      : Number.isSafeInteger(total) && total >= 0 ? Math.max(0, Math.min(100, total === 0 ? 100 : Math.floor(received * 100 / total))) : null;
    return {
      phase, busy, message: labels[phase] || labels.error,
      check: ["idle", "upToDate", "notPublished", "error"].includes(phase),
      download: phase === "available", cancel: phase === "downloading",
      install: phase === "ready", later: phase === "ready",
      hasProgress, indeterminate: hasProgress && percent === null, percent,
      progressText: phase === "ready" ? "ダウンロード済み"
        : percent === null ? `${received.toLocaleString()} bytes` : `${percent}% (${received.toLocaleString()} / ${total.toLocaleString()} bytes)`,
    };
  }
  function renderUpdateNotes(element, notes) { element.textContent = notes || "更新内容はありません。"; }

  window.createSettingsView = (root, { onBack, onSaved, initialSnapshot, platformCapabilities = { platform: "unknown", liveServices: false, updater: false } } = {}) => {
  const invoke = (name, args) => {
    const call = window.__TAURI__?.core?.invoke;
    return typeof call === "function"
      ? call(name, args)
      : Promise.reject(new Error("設定画面はアプリ内でのみ利用できます。バックエンドに接続できません。"));
  };
  const $ = (id) => root.querySelector(`#${id}`);
  const defaultArguments = platformCapabilities.platform === "linux" ? `"<stream/>"` : `"<stream/>" "<channelname/>" "<contact/>"`;
  function normalizeWindowPreferences(value = {}) {
    const width = value.width == null || value.width === "" ? Number.NaN : Number(value.width);
    const height = value.height == null || value.height === "" ? Number.NaN : Number(value.height);
    return {
      width: Number.isFinite(width) ? Math.trunc(width) : 1600,
      height: Number.isFinite(height) ? Math.trunc(height) : 900,
      presets: Array.isArray(value.presets) ? value.presets.map((preset) => ({
        id: String(preset?.id ?? ""), name: String(preset?.name ?? ""),
        width: Number(preset?.width), height: Number(preset?.height),
      })) : [],
      logicalSize: Boolean(value.logicalSize),
      minimizeToTray: Boolean(value.minimizeToTray),
      launchAtStartup: Boolean(value.launchAtStartup),
    };
  }
  function parseWindowPreset(value) {
    const match = /^(\d{3,4})x(\d{3,4})$/.exec(String(value).trim());
    return match ? { width: Number(match[1]), height: Number(match[2]) } : null;
  }
  function isValidWindowSize(width, height) {
    return Number.isInteger(width) && width >= 1280 && width <= 7680
      && Number.isInteger(height) && height >= 720 && height <= 4320;
  }
  function addWindowPreset(windowSettings, name, id, width = windowSettings.width, height = windowSettings.height) {
    const normalizedName = String(name ?? "").trim();
    if (!normalizedName || [...normalizedName].length > 40) return { error: "プリセット名は1〜40文字で入力してください。" };
    if (!isValidWindowSize(width, height)) return { error: "サイズは幅1280〜7680、高さ720〜4320pxで入力してください。" };
    const current = Array.isArray(windowSettings.presets) ? windowSettings.presets : [];
    if (current.length >= 32) return { error: "プリセットは32件まで登録できます。" };
    if (!String(id).trim() || [...String(id)].length > 80) return { error: "プリセットIDを作成できませんでした。" };
    if (current.some((preset) => preset.id === String(id))) return { error: "プリセットIDが重複しています。" };
    if (current.some((preset) => String(preset.name).trim().toLowerCase() === normalizedName.toLowerCase())) return { error: "同じ名前のプリセットがすでにあります。" };
    const preset = { id: String(id), name: normalizedName, width, height };
    return { window: { ...windowSettings, presets: [...current, preset] }, preset };
  }
  function removeWindowPreset(windowSettings, id) {
    const current = Array.isArray(windowSettings.presets) ? windowSettings.presets : [];
    return { ...windowSettings, presets: current.filter((preset) => preset.id !== id) };
  }
  function applyWindowPreset(windowSettings, preset) {
    return { ...windowSettings, width: preset.width, height: preset.height };
  }
  function windowPresetKeyForSize(windowSettings) {
    const selected = windowSettings.presets.find((preset) => preset.width === windowSettings.width && preset.height === windowSettings.height);
    return selected ? `user:${selected.id}` : `${windowSettings.width}x${windowSettings.height}`;
  }
  function formatBackendUptime(seconds) {
    if (seconds === null || seconds === undefined || seconds === "") return "—";
    const value = Number(seconds);
    if (!Number.isFinite(value) || value < 0) return "—";
    const total = Math.floor(value);
    const secs = total % 60;
    if (total < 60) return `${total}秒`;
    const minutes = Math.floor(total / 60);
    if (total < 3600) return `${minutes}分 ${secs}秒`;
    const hours = Math.floor(total / 3600);
    const remainingMinutes = Math.floor((total % 3600) / 60);
    return `${hours}時間 ${remainingMinutes}分 ${secs}秒`;
  }
  const ruleFields = ["name", "details", "comment", "contactUrl", "yellowPage", "streamUrl", "contentType", "listeners", "bitrate"];
  const recordingDefaults = { enabled: false, directory: "recordings", channelSubfolders: true, limitConcurrent: true, maxConcurrent: 5, retryCount: 30, retryIntervalSecs: 10, disconnectEveryRetries: 5, disconnectOnRetryEnd: true, splitMinutes: null, splitSizeMb: null, allDay: true, watchStart: "00:00", watchEnd: "23:59", monitorIntervalSecs: 60, rules: [] };
  function cloneRecording(value) { return { ...value, rules: (value.rules || []).map((rule) => ({ ...rule, fields: [...rule.fields] })) }; }
  const state = { capabilities: platformCapabilities, snapshot: null, limits: null, pages: [], savedPages: [], selected: null, player: { executable: null, arguments: defaultArguments }, savedPlayer: { executable: null, arguments: defaultArguments }, rules: [], savedRules: [], selectedRule: null, rulesSearch: "", recording: cloneRecording(recordingDefaults), savedRecording: cloneRecording(recordingDefaults), directory: { youtubeAtBottom: false, twitchAtBottom: false }, window: normalizeWindowPreferences(), selectedWindowPreset: "", selectedRecordingRule: null, recordingRulesSearch: "", recordingRulesFingerprint: null, recordingListFingerprint: null, dirty: false, loading: true, ready: false, busy: false, previewing: false, preferenceFingerprint: null, listFingerprint: null, rulesFingerprint: null, leavePromise: null, resolveLeave: null, updateStatus: null, updateStatusLoading: false, updateActionBusy: false, updatePollTimer: null, updatePollRequest: null, updateViewActive: false, updateDeferred: false, updateDisposed: false };
  Object.assign(state, { youtubeSnapshot: null, youtubeLoaded: false, youtubeLoading: false, youtubeBusy: false, youtubeMessage: "読み込み中", twitchSnapshot: null, twitchLoaded: false, twitchLoading: false, twitchBusy: false, twitchMessage: "読み込み中" });
  Object.assign(state, { noticeTimer: null, noticeGeneration: 0, statusTimer: null, statusGeneration: 0, providerMessages: new Map(), providerMessageToken: 0 });
  const meta = {
    general: ["全般", "現在のバックエンド状態と設定ファイルを確認します。"],
    connection: ["接続", "接続上限を変更します。保存時に実バックエンドへ反映します。"],
    "yellow-pages": ["イエローページ", "チャンネル一覧の取得先とYouTube・Twitchのお気に入りを管理します。\nSPへの掲載は、配信画面で掲載を選んだ場合に行われます。"],
    player: ["プレイヤー", "視聴時に起動するプレイヤーと引数を設定します。"],
    "channel-rules": ["NG・お気に入り", "チャンネル情報に一致する条件と表示・通知方法を設定します。"],
    recording: ["録画", "保存先、録画の再試行と自動監視条件を設定します。"],
    appearance: ["外観", "画面の表示方法を設定します。"],
    version: ["バージョン情報", "アプリと使用しているライセンスを確認します。"],
  };

  function setNotice(message, error = false, { persistent = false } = {}) {
    const notice = $("notice");
    const next = String(message || "");
    if (next && notice.textContent === next && state.noticeError === error && state.noticePersistent === persistent) return;
    window.clearTimeout(state.noticeTimer);
    state.noticeTimer = null;
    const generation = ++state.noticeGeneration;
    state.noticeError = error;
    state.noticePersistent = persistent;
    notice.textContent = next;
    notice.classList.toggle("error", error);
    notice.hidden = !next;
    if (next && !persistent) state.noticeTimer = window.setTimeout(() => {
      if (generation !== state.noticeGeneration) return;
      notice.textContent = "";
      notice.hidden = true;
      state.noticeTimer = null;
    }, error ? 12000 : 5000);
  }
  function setStatus(message, { persistent = false } = {}) {
    const element = $("save-status");
    const next = String(message || "");
    if (next && element.textContent === next && state.statusPersistent === persistent) return;
    window.clearTimeout(state.statusTimer);
    state.statusTimer = null;
    const generation = ++state.statusGeneration;
    state.statusPersistent = persistent;
    element.textContent = next;
    if (next && !persistent) state.statusTimer = window.setTimeout(() => {
      if (generation !== state.statusGeneration) return;
      element.textContent = state.dirty ? "未保存の変更があります" : "";
      state.statusTimer = null;
    }, 5000);
  }
  function setProviderMessage(provider, message, { persistent = false, error = false } = {}) {
    const previous = state.providerMessages.get(provider);
    if (message && previous?.message === message && previous.persistent === persistent && previous.error === error) return;
    if (previous) window.clearTimeout(previous.timer);
    const token = ++state.providerMessageToken;
    const entry = { message, persistent, error, token, timer: null };
    state.providerMessages.set(provider, entry);
    state[`${provider}Message`] = message;
    $(`${provider}-status`).textContent = message;
    if (message && !persistent) entry.timer = window.setTimeout(() => {
      if (state.providerMessages.get(provider)?.token !== token) return;
      state[`${provider}Message`] = "";
      $(`${provider}-status`).textContent = "";
      entry.timer = null;
    }, error ? 12000 : 5000);
  }
  function safeColor(value) { return typeof value === "string" && /^#[0-9a-fA-F]{6}$/.test(value) ? value : null; }
  function normalizedRule(rule = {}) {
    return {
      id: String(rule.id || ""), name: String(rule.name || ""), enabled: Boolean(rule.enabled), pattern: String(rule.pattern || ""),
      fields: Array.isArray(rule.fields) ? [...new Set(rule.fields.filter((field) => ruleFields.includes(field)))] : [],
      useRegex: Boolean(rule.useRegex), normalizeWidth: rule.normalizeWidth !== false, matchCase: Boolean(rule.matchCase),
      favorite: Boolean(rule.favorite), hidden: Boolean(rule.hidden), mute: Boolean(rule.mute),
      textColor: safeColor(rule.textColor), backgroundColor: safeColor(rule.backgroundColor),
      soundPath: typeof rule.soundPath === "string" && rule.soundPath ? rule.soundPath : null,
    };
  }
  function exactNamePattern(name) {
    const normalized = String(name || "").normalize("NFKC");
    return normalized ? `^${normalized.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$` : "";
  }
  function newRule(kind = "normal", channel = null) {
    const id = window.crypto?.randomUUID?.() || `rule-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    const name = channel?.name ? String(channel.name).slice(0, 80) : "新しいルール";
    const pattern = channel?.name ? exactNamePattern(channel.name) : "";
    return normalizedRule({
      id, name, enabled: true, pattern, fields: ["name"],
      useRegex: Boolean(pattern), normalizeWidth: true, matchCase: false,
      favorite: kind === "favorite" || kind === "normal", hidden: kind === "ng", mute: false,
      textColor: null, backgroundColor: null, soundPath: null,
    });
  }
  function normalizedRecordingFilter(rule = {}) {
    return {
      id: String(rule.id || ""), name: String(rule.name || "新しい録画条件"), enabled: Boolean(rule.enabled), pattern: String(rule.pattern || ""),
      fields: Array.isArray(rule.fields) ? [...new Set(rule.fields.filter((field) => ruleFields.includes(field)))] : ["name"],
      useRegex: Boolean(rule.useRegex), normalizeWidth: rule.normalizeWidth !== false, matchCase: Boolean(rule.matchCase),
    };
  }
  function normalizeRecordingSettings(value = {}) {
    const optionalNumber = (field) => value[field] === undefined ? recordingDefaults[field] : value[field] === null ? null : Number(value[field]);
    return {
      enabled: Boolean(value.enabled ?? recordingDefaults.enabled),
      directory: String(value.directory ?? recordingDefaults.directory),
      channelSubfolders: Boolean(value.channelSubfolders ?? recordingDefaults.channelSubfolders),
      limitConcurrent: Boolean(value.limitConcurrent ?? recordingDefaults.limitConcurrent),
      maxConcurrent: Number(value.maxConcurrent ?? recordingDefaults.maxConcurrent),
      retryCount: Number(value.retryCount ?? recordingDefaults.retryCount),
      retryIntervalSecs: Number(value.retryIntervalSecs ?? recordingDefaults.retryIntervalSecs),
      disconnectEveryRetries: optionalNumber("disconnectEveryRetries"),
      disconnectOnRetryEnd: Boolean(value.disconnectOnRetryEnd ?? recordingDefaults.disconnectOnRetryEnd),
      splitMinutes: optionalNumber("splitMinutes"),
      splitSizeMb: optionalNumber("splitSizeMb"),
      allDay: Boolean(value.allDay ?? recordingDefaults.allDay),
      watchStart: String(value.watchStart ?? recordingDefaults.watchStart),
      watchEnd: String(value.watchEnd ?? recordingDefaults.watchEnd),
      monitorIntervalSecs: Number(value.monitorIntervalSecs ?? recordingDefaults.monitorIntervalSecs),
      rules: Array.isArray(value.rules) ? value.rules.map((rule) => normalizedRecordingFilter(rule)) : [],
    };
  }
  function newRecordingFilter(source = null) {
    const id = window.crypto?.randomUUID?.() || `recording-rule-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    if (!source) return normalizedRecordingFilter({ id, name: "新しい録画条件", enabled: true, pattern: "", fields: ["name"], useRegex: false, normalizeWidth: true, matchCase: false });
    return normalizedRecordingFilter({ ...source, id, name: `${source.name || "録画条件"}（コピー）`.slice(0, 80) });
  }
  function setBusy(busy) {
    state.busy = busy;
    renderControls();
  }
  function renderControls() {
    const editable = !state.loading && state.ready && !state.busy;
    const csharpBackend = state.snapshot?.backendKind === "csharp";
    ["max-directs", "max-relays", "max-upstream-rate"].forEach((id) => { $(id).disabled = !editable || csharpBackend; });
    const connectionNote = $("connection-backend-note");
    if (connectionNote) {
      connectionNote.hidden = !csharpBackend;
      connectionNote.textContent = csharpBackend ? "接続上限は起動済みC#版で設定してください。" : "";
    }
    $("page-name").disabled = !editable || state.selected === null;
    $("page-url").disabled = !editable || state.selected === null;
    $("add-page").disabled = !editable || state.pages.length >= 32;
    $("delete-page").disabled = !editable || state.selected === null;
    $("save-button").disabled = !editable || !state.dirty;
    $("cancel-button").disabled = state.busy;
    $("youtube-at-bottom").disabled = !editable || state.capabilities?.liveServices === false;
    $("twitch-at-bottom").disabled = !editable || state.capabilities?.liveServices === false;
    const youtubeDisabled = state.capabilities?.liveServices === false || !state.ready || state.loading || state.busy || state.youtubeLoading || state.youtubeBusy;
    ["youtube-url", "youtube-add", "youtube-refresh"].forEach((id) => { $(id).disabled = youtubeDisabled; });
    root.querySelectorAll("[data-youtube-action]").forEach((button) => { button.disabled = youtubeDisabled; });
    const refreshState = youtubeRefreshState(state.youtubeSnapshot, Date.now());
    $("youtube-refresh").disabled = youtubeDisabled || refreshState.disabled;
    $("youtube-refresh").title = refreshState.remainingSeconds > 0 ? `手動確認はあと${refreshState.remainingSeconds}秒後にできます` : "登録チャンネルを今すぐ確認します";
    const twitchDisabled = state.capabilities?.liveServices === false || !state.ready || state.loading || state.busy || state.twitchLoading || state.twitchBusy;
    ["twitch-url", "twitch-add", "twitch-refresh"].forEach((id) => { $(id).disabled = twitchDisabled; });
    root.querySelectorAll("[data-twitch-action]").forEach((button) => { button.disabled = twitchDisabled; });
    const twitchRefresh = youtubeRefreshState(state.twitchSnapshot, Date.now());
    $("twitch-refresh").disabled = twitchDisabled || twitchRefresh.disabled;
    $("twitch-refresh").title = twitchRefresh.remainingSeconds > 0 ? `手動確認はあと${twitchRefresh.remainingSeconds}秒後にできます` : "登録チャンネルを今すぐ確認します";
    ["window-preset", "window-width", "window-height", "window-logical-size", "window-minimize-to-tray", "window-launch-at-startup", "window-preset-name"].forEach((id) => { $(id).disabled = !editable; });
    $("window-preset-add").disabled = !editable || state.window.presets.length >= 32;
    $("window-preset-remove").disabled = !editable || !state.selectedWindowPreset.startsWith("user:");
    root.querySelectorAll(".yp-row").forEach((button) => { button.disabled = state.loading || state.busy; });
    const hasRule = state.selectedRule !== null && state.rules[state.selectedRule];
    ["use-bundled-player", "player-arguments", "rule-name", "rule-pattern", "rule-enabled", "rule-regex", "rule-normalize-width", "rule-match-case", "rule-favorite", "rule-hidden", "rule-mute", "rule-text-color-enabled", "rule-text-color", "rule-background-color-enabled", "rule-background-color", "rule-sound-path", "preview-rule-sound", "preview-rules", ...ruleFields.map((field) => `rule-field-${field}`)].forEach((id) => { const element = $(id); if (element) element.disabled = !editable || (id.startsWith("rule-") && !hasRule); });
    $("player-executable").disabled = !editable || $("use-bundled-player").checked;
    $("choose-player").disabled = !editable || $("use-bundled-player").checked;
    root.querySelectorAll("[data-player-tag]").forEach((button) => { button.disabled = !editable; });
    $("add-rule").disabled = !editable || state.rules.length >= 128;
    $("rules-search").disabled = !state.ready || state.loading;
    $("delete-rule").disabled = !editable || !hasRule;
    $("move-rule-up").disabled = !editable || !hasRule || state.selectedRule <= 0 || Boolean(state.rulesSearch.trim());
    $("move-rule-down").disabled = !editable || !hasRule || state.selectedRule >= state.rules.length - 1 || Boolean(state.rulesSearch.trim());
    $("choose-rule-sound").disabled = !editable || !hasRule;
    $("preview-rules").disabled = !editable || state.previewing;
    $("preview-rule-sound").disabled = !editable || !hasRule || !state.rules[state.selectedRule]?.soundPath || state.previewing;
    $("rule-text-color").disabled = !editable || !hasRule || !$("rule-text-color-enabled").checked;
    $("rule-background-color").disabled = !editable || !hasRule || !$("rule-background-color-enabled").checked;
    root.querySelectorAll("#panel-recording input, #panel-recording button").forEach((element) => { element.disabled = !editable; });
    $("recording-max-concurrent").disabled = !editable || !$("recording-limit-concurrent").checked;
    $("recording-disconnect-every").disabled = !editable || !$("recording-disconnect-every-enabled").checked;
    $("recording-split-minutes").disabled = !editable || !$("recording-split-minutes-enabled").checked;
    $("recording-split-size").disabled = !editable || !$("recording-split-size-enabled").checked;
    $("recording-watch-start").disabled = !editable || $("recording-all-day").checked;
    $("recording-watch-end").disabled = !editable || $("recording-all-day").checked;
    $("recording-rules-search").disabled = !state.ready || state.loading || state.busy;
    const hasRecordingRule = state.selectedRecordingRule !== null && state.recording.rules[state.selectedRecordingRule];
    $("add-recording-rule").disabled = !editable || state.recording.rules.length >= 128;
    $("duplicate-recording-rule").disabled = !editable || !hasRecordingRule || state.recording.rules.length >= 128;
    $("delete-recording-rule").disabled = !editable || !hasRecordingRule;
    $("recording-rule-up").disabled = !editable || !hasRecordingRule || state.selectedRecordingRule <= 0 || Boolean(state.recordingRulesSearch.trim());
    $("recording-rule-down").disabled = !editable || !hasRecordingRule || state.selectedRecordingRule >= state.recording.rules.length - 1 || Boolean(state.recordingRulesSearch.trim());
    $("preview-recording-rules").disabled = !editable || state.previewing;
    root.querySelectorAll(".recording-rule-row").forEach((button) => { button.disabled = !editable; });
    root.querySelectorAll(".recording-rule-row input").forEach((input) => { input.disabled = !editable; });
    root.querySelectorAll("[data-page=recording]").forEach((tab) => { tab.disabled = state.loading; });
    renderUpdaterControls();
  }
  function setDirty() {
    state.dirty = true;
    setStatus("未保存の変更があります", { persistent: true });
    renderControls();
  }
  function renderWindowPresetOptions(selectedKey = state.selectedWindowPreset) {
    const group = $("window-custom-presets");
    group.replaceChildren();
    for (const preset of state.window.presets) {
      const option = document.createElement("option");
      option.value = `user:${preset.id}`;
      option.textContent = preset.name;
      group.append(option);
    }
    const select = $("window-preset");
    select.value = selectedKey;
    if (select.value !== selectedKey) select.value = "";
    state.selectedWindowPreset = select.value;
  }
  function renderYoutubeSubscriptions() {
    const rootElement = $("youtube-subscriptions");
    const channels = Array.isArray(state.youtubeSnapshot?.channels) ? state.youtubeSnapshot.channels : [];
    const focused = rootElement.contains(rootElement.ownerDocument.activeElement)
      ? [rootElement.ownerDocument.activeElement?.dataset?.youtubeAction, rootElement.ownerDocument.activeElement?.dataset?.youtubeId]
      : null;
    rootElement.replaceChildren();
    if (!channels.length) {
      const empty = document.createElement("p");
      empty.className = "empty-state";
      empty.textContent = "YouTubeチャンネルはまだ登録されていません。";
      rootElement.append(empty);
    }
    for (const channel of channels) {
      const row = document.createElement("div");
      row.className = "youtube-settings-subscription";
      const label = document.createElement("span");
      label.className = "youtube-settings-subscription-name";
      const name = document.createElement("strong");
      name.textContent = String(channel.name || channel.url || "登録チャンネル");
      label.append(name);
      const url = document.createElement("span");
      url.className = "youtube-settings-subscription-url";
      url.textContent = String(channel.url || "");
      label.append(url);
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "button button-secondary";
      toggle.textContent = channel.enabled === true ? "監視を無効にする" : "監視を有効にする";
      toggle.dataset.youtubeAction = "enabled";
      toggle.dataset.youtubeId = String(channel.id || "");
      toggle.setAttribute("aria-pressed", String(channel.enabled === true));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "button button-secondary youtube-remove";
      remove.textContent = "解除";
      remove.dataset.youtubeAction = "remove";
      remove.dataset.youtubeId = String(channel.id || "");
      row.append(label, toggle, remove);
      rootElement.append(row);
    }
    if (focused?.[0]) {
      const nextFocus = [...rootElement.querySelectorAll("[data-youtube-action]")]
        .find((button) => button.dataset.youtubeAction === focused[0] && button.dataset.youtubeId === focused[1]);
      nextFocus?.focus();
    }
    $("youtube-status").textContent = state.youtubeMessage;
    renderControls();
  }
  function youtubeRefreshState(snapshot, nowMs) {
    const lastRefresh = Number(snapshot?.lastRefreshMs);
    const remainingSeconds = Number.isFinite(lastRefresh) && lastRefresh > 0
      ? Math.max(0, Math.ceil((lastRefresh + 30_000 - nowMs) / 1000))
      : 0;
    return { disabled: !Array.isArray(snapshot?.channels) || snapshot.channels.length === 0 || snapshot.refreshing === true || remainingSeconds > 0, remainingSeconds };
  }
  function renderTwitchSubscriptions() {
    const rootElement = $("twitch-subscriptions");
    const channels = Array.isArray(state.twitchSnapshot?.channels) ? state.twitchSnapshot.channels : [];
    const focused = rootElement.contains(rootElement.ownerDocument.activeElement)
      ? [rootElement.ownerDocument.activeElement?.dataset?.twitchAction, rootElement.ownerDocument.activeElement?.dataset?.twitchId]
      : null;
    rootElement.replaceChildren();
    if (!channels.length) {
      const empty = document.createElement("p");
      empty.className = "empty-state";
      empty.textContent = "Twitchチャンネルはまだ登録されていません。";
      rootElement.append(empty);
    }
    for (const channel of channels) {
      const row = document.createElement("div");
      row.className = "youtube-settings-subscription";
      const label = document.createElement("span");
      label.className = "youtube-settings-subscription-name";
      const name = document.createElement("strong");
      name.textContent = String(channel.name || channel.url || "登録チャンネル");
      label.append(name);
      const url = document.createElement("span");
      url.className = "youtube-settings-subscription-url";
      url.textContent = String(channel.url || "");
      label.append(url);
      const channelStatus = twitchSubscriptionStatus(channel);
      const status = document.createElement("span");
      status.className = `twitch-subscription-status${channelStatus === "error" ? " has-error" : channelStatus === "live" ? " is-live" : ""}`;
      status.textContent = ({ error: "確認できず", disabled: "監視無効", live: "配信中", waiting: "確認待ち", offline: "配信なし" })[channelStatus];
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "button button-secondary";
      toggle.textContent = channel.enabled === true ? "監視を無効にする" : "監視を有効にする";
      toggle.dataset.twitchAction = "enabled";
      toggle.dataset.twitchId = String(channel.id || "");
      toggle.setAttribute("aria-pressed", String(channel.enabled === true));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "button button-secondary youtube-remove";
      remove.textContent = "解除";
      remove.dataset.twitchAction = "remove";
      remove.dataset.twitchId = String(channel.id || "");
      row.append(label, status, toggle, remove);
      rootElement.append(row);
    }
    if (focused?.[0]) {
      const nextFocus = [...rootElement.querySelectorAll("[data-twitch-action]")]
        .find((button) => button.dataset.twitchAction === focused[0] && button.dataset.twitchId === focused[1]);
      nextFocus?.focus();
    }
    $("twitch-status").textContent = state.twitchMessage;
    renderControls();
  }
  function acceptYoutubeSnapshot(snapshot, message) {
    if (!snapshot || !Array.isArray(snapshot.channels)) return false;
    state.youtubeSnapshot = snapshot;
    state.youtubeLoaded = true;
    setProviderMessage("youtube", message);
    renderYoutubeSubscriptions();
    window.dispatchEvent(new CustomEvent("youtube-snapshot-updated", { detail: snapshot }));
    return true;
  }
  async function loadYoutubeSettings() {
    if (state.capabilities?.liveServices === false) return;
    if (state.youtubeLoaded || state.youtubeLoading) return;
    state.youtubeLoading = true;
    setProviderMessage("youtube", "YouTube登録を読み込み中です…", { persistent: true });
    renderControls();
    window.dispatchEvent(new Event("youtube-snapshot-requested"));
  }
  window.addEventListener("youtube-snapshot-polled", (event) => {
    const snapshot = event.detail;
    if (!snapshot || !Array.isArray(snapshot.channels)) return;
    const fingerprint = (channels) => JSON.stringify(channels.map((channel) => [channel.id, channel.url, channel.name, channel.enabled]));
    const changed = fingerprint(state.youtubeSnapshot?.channels || []) !== fingerprint(snapshot.channels);
    state.youtubeSnapshot = snapshot;
    const firstLoad = !state.youtubeLoaded;
    state.youtubeLoaded = true;
    state.youtubeLoading = false;
    if (firstLoad) setProviderMessage("youtube", "YouTube登録を読み込みました。");
    else if (state.youtubeMessage.startsWith("YouTubeの状態を確認できませんでした:")) setProviderMessage("youtube", "YouTube登録を読み込みました。");
    $("youtube-status").textContent = state.youtubeMessage;
    if (changed) renderYoutubeSubscriptions();
    else renderControls();
  });
  window.addEventListener("youtube-snapshot-error", (event) => {
    state.youtubeLoading = false;
    setProviderMessage("youtube", `YouTubeの状態を確認できませんでした: ${String(event.detail || "")}`, { error: true });
    $("youtube-status").textContent = state.youtubeMessage;
    renderControls();
  });
  async function youtubeSettingsMutation(command, args) {
    if (state.capabilities?.liveServices === false) return;
    if (state.youtubeBusy || !state.ready) return false;
    state.youtubeBusy = true;
    setProviderMessage("youtube", "YouTube登録を更新しています…", { persistent: true });
    renderYoutubeSubscriptions();
    try {
      const snapshot = await invoke(command, args);
      acceptYoutubeSnapshot(snapshot, "YouTube登録を更新しました。");
      return true;
    } catch (error) {
      setProviderMessage("youtube", `YouTubeを更新できませんでした: ${error.message || error}`, { error: true });
      renderYoutubeSubscriptions();
      return false;
    } finally {
      state.youtubeBusy = false;
      renderControls();
    }
  }
  async function loadTwitchSettings() {
    if (state.capabilities?.liveServices === false) return;
    if (state.twitchLoaded || state.twitchLoading) return;
    state.twitchLoading = true;
    setProviderMessage("twitch", "Twitch登録を読み込み中です…", { persistent: true });
    renderControls();
    window.dispatchEvent(new Event("twitch-snapshot-requested"));
  }
  function twitchSubscriptionStatus(channel) {
    if (channel.error) return "error";
    if (channel.enabled !== true) return "disabled";
    if (Array.isArray(channel.streams) && channel.streams.some((stream) => stream?.status === "live")) return "live";
    return channel.lastCheckedMs == null ? "waiting" : "offline";
  }
  function twitchSubscriptionFingerprint(channels) {
    return JSON.stringify(channels.map((channel) => [channel.id, channel.url, channel.name, channel.enabled, twitchSubscriptionStatus(channel)]));
  }
  function acceptTwitchPolledSnapshot(snapshot) {
    if (!snapshot || !Array.isArray(snapshot.channels)) return;
    const changed = twitchSubscriptionFingerprint(state.twitchSnapshot?.channels || []) !== twitchSubscriptionFingerprint(snapshot.channels);
    state.twitchSnapshot = snapshot;
    const firstLoad = !state.twitchLoaded;
    state.twitchLoaded = true;
    state.twitchLoading = false;
    if (firstLoad) setProviderMessage("twitch", "Twitch登録を読み込みました。");
    else if (state.twitchMessage.startsWith("Twitchの状態を確認できませんでした:")) setProviderMessage("twitch", "Twitch登録を読み込みました。");
    $("twitch-status").textContent = state.twitchMessage;
    if (changed) renderTwitchSubscriptions();
    else renderControls();
  }
  window.addEventListener("twitch-snapshot-polled", (event) => acceptTwitchPolledSnapshot(event.detail));
  window.addEventListener("twitch-snapshot-error", (event) => {
    state.twitchLoading = false;
    setProviderMessage("twitch", `Twitchの状態を確認できませんでした: ${String(event.detail || "")}`, { error: true });
    $("twitch-status").textContent = state.twitchMessage;
    renderControls();
  });
  async function twitchSettingsMutation(command, args) {
    if (state.capabilities?.liveServices === false) return;
    if (state.twitchBusy || !state.ready) return false;
    state.twitchBusy = true;
    setProviderMessage("twitch", "Twitch登録を更新しています…", { persistent: true });
    renderTwitchSubscriptions();
    try {
      const snapshot = await invoke(command, args);
      if (!snapshot || !Array.isArray(snapshot.channels)) throw new Error("Twitchの状態を取得できませんでした");
      state.twitchSnapshot = snapshot;
      state.twitchLoaded = true;
      setProviderMessage("twitch", "Twitch登録を更新しました。");
      renderTwitchSubscriptions();
      window.dispatchEvent(new CustomEvent("twitch-snapshot-updated", { detail: snapshot }));
      return true;
    } catch (error) {
      setProviderMessage("twitch", `Twitchを更新できませんでした: ${error.message || error}`, { error: true });
      renderTwitchSubscriptions();
      return false;
    } finally {
      state.twitchBusy = false;
      renderControls();
    }
  }
  function pageValue() {
    return { name: $("page-name").value, channelsUrl: $("page-url").value };
  }
  function ruleValue() { return state.selectedRule === null ? null : state.rules[state.selectedRule] || null; }
  function commitRuleEditor() {
    const rule = ruleValue();
    if (!rule || state.loading) return;
    rule.name = $("rule-name").value;
    rule.pattern = $("rule-pattern").value;
    rule.enabled = $("rule-enabled").checked;
    rule.fields = ruleFields.filter((field) => $(`rule-field-${field}`).checked);
    rule.useRegex = $("rule-regex").checked;
    rule.normalizeWidth = $("rule-normalize-width").checked;
    rule.matchCase = $("rule-match-case").checked;
    rule.favorite = $("rule-favorite").checked;
    rule.hidden = $("rule-hidden").checked;
    rule.mute = $("rule-mute").checked;
    rule.textColor = $("rule-text-color-enabled").checked ? safeColor($("rule-text-color").value) : null;
    rule.backgroundColor = $("rule-background-color-enabled").checked ? safeColor($("rule-background-color").value) : null;
    rule.soundPath = $("rule-sound-path").value || null;
    renderRulesList();
  }
  function syncRuleFields() {
    const rule = ruleValue();
    $("rule-name").value = rule?.name || "";
    $("rule-pattern").value = rule?.pattern || "";
    $("rule-enabled").checked = Boolean(rule?.enabled);
    ruleFields.forEach((field) => { $(`rule-field-${field}`).checked = Boolean(rule?.fields.includes(field)); });
    $("rule-regex").checked = Boolean(rule?.useRegex);
    $("rule-normalize-width").checked = rule?.normalizeWidth !== false;
    $("rule-match-case").checked = Boolean(rule?.matchCase);
    $("rule-favorite").checked = Boolean(rule?.favorite);
    $("rule-hidden").checked = Boolean(rule?.hidden);
    $("rule-mute").checked = Boolean(rule?.mute);
    $("rule-text-color-enabled").checked = Boolean(rule?.textColor);
    $("rule-text-color").value = rule?.textColor || "#17143e";
    $("rule-background-color-enabled").checked = Boolean(rule?.backgroundColor);
    $("rule-background-color").value = rule?.backgroundColor || "#ffffff";
    $("rule-sound-path").value = rule?.soundPath || "";
    $("rules-editor-heading").textContent = rule?.name || "条件を選択";
    renderControls();
  }
  function clearRuleSelection() {
    if (state.selectedRule === null) return;
    commitRuleEditor();
    state.selectedRule = null;
    syncRuleFields();
  }
  function handleRuleDisclosureToggle() {
    if (!$("saved-rules-disclosure").open) clearRuleSelection();
  }
  function renderRulesList() {
    const list = $("rules-list");
    const query = state.rulesSearch.trim().toLocaleLowerCase();
    const matches = state.rules.map((rule, index) => ({ rule, index })).filter(({ rule }) => !query || `${rule.name} ${rule.pattern}`.toLocaleLowerCase().includes(query));
    $("rules-count").textContent = `${state.rules.length} / 128 件`;
    $("rules-empty").hidden = matches.length > 0;
    const fingerprint = JSON.stringify(matches.map(({ rule, index }) => [rule.id, rule.name, rule.pattern, rule.enabled, index === state.selectedRule]));
    if (fingerprint !== state.rulesFingerprint) {
      state.rulesFingerprint = fingerprint;
      list.replaceChildren();
      matches.forEach(({ rule, index }) => {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button"; button.className = `rule-row${index === state.selectedRule ? " selected" : ""}`; button.dataset.ruleIndex = String(index); button.disabled = state.loading || !state.ready || state.busy; button.setAttribute("aria-pressed", String(index === state.selectedRule));
        const enabled = document.createElement("input"); enabled.type = "checkbox"; enabled.checked = rule.enabled; enabled.disabled = state.loading || !state.ready || state.busy; enabled.setAttribute("aria-label", `${rule.name || "名前なし"}を有効にする`);
        enabled.addEventListener("click", (event) => event.stopPropagation());
        enabled.addEventListener("change", (event) => { event.stopPropagation(); rule.enabled = enabled.checked; if (state.selectedRule === index) $("rule-enabled").checked = enabled.checked; setDirty(); renderRulesList(); });
        const title = document.createElement("span"); title.className = "rule-row-title"; title.textContent = rule.name || "名前なし";
        const pattern = document.createElement("span"); pattern.className = "rule-row-pattern"; pattern.textContent = rule.pattern || "検索文字列なし";
        button.append(enabled, title, pattern); button.addEventListener("click", () => { commitRuleEditor(); state.selectedRule = index; syncRuleFields(); renderRulesList(); }); item.append(button); list.append(item);
      });
    }
    renderControls();
  }
  function syncPlayerFields() {
    const player = state.player || { executable: null, arguments: defaultArguments };
    $("use-bundled-player").checked = player.executable == null;
    $("player-executable").value = player.executable || "";
    $("player-arguments").value = player.arguments || "";
    renderControls();
  }
  function commitPlayerFields() {
    state.player = { executable: $("use-bundled-player").checked ? null : ($("player-executable").value.trim() || null), arguments: $("player-arguments").value };
  }
  function syncRecordingFields() {
    const value = state.recording;
    $("recording-enabled").checked = value.enabled;
    $("recording-directory").value = value.directory;
    $("recording-subfolders").checked = value.channelSubfolders;
    $("recording-limit-concurrent").checked = value.limitConcurrent;
    $("recording-max-concurrent").value = String(value.maxConcurrent);
    $("recording-retry-count").value = String(value.retryCount);
    $("recording-retry-interval").value = String(value.retryIntervalSecs);
    $("recording-disconnect-every-enabled").checked = value.disconnectEveryRetries !== null;
    $("recording-disconnect-every").value = value.disconnectEveryRetries === null ? "" : String(value.disconnectEveryRetries);
    $("recording-disconnect-on-end").checked = value.disconnectOnRetryEnd;
    $("recording-split-minutes-enabled").checked = value.splitMinutes !== null;
    $("recording-split-minutes").value = value.splitMinutes === null ? "" : String(value.splitMinutes);
    $("recording-split-size-enabled").checked = value.splitSizeMb !== null;
    $("recording-split-size").value = value.splitSizeMb === null ? "" : String(value.splitSizeMb);
    $("recording-all-day").checked = value.allDay;
    $("recording-watch-start").value = value.watchStart;
    $("recording-watch-end").value = value.watchEnd;
    $("recording-monitor-interval").value = String(value.monitorIntervalSecs);
    const now = new Date();
    const pad = (part) => String(part).padStart(2, "0");
    const date = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
    $("recording-filename-example").textContent = `チャンネル名_${date}_${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}_配信内容.flv`;
    renderControls();
  }
  function commitRecordingFields() {
    const oldRules = state.recording.rules;
    const number = (id) => Number($(id).value);
    state.recording = {
      enabled: $("recording-enabled").checked,
      directory: $("recording-directory").value,
      channelSubfolders: $("recording-subfolders").checked,
      limitConcurrent: $("recording-limit-concurrent").checked,
      maxConcurrent: number("recording-max-concurrent"),
      retryCount: number("recording-retry-count"),
      retryIntervalSecs: number("recording-retry-interval"),
      disconnectEveryRetries: $("recording-disconnect-every-enabled").checked ? number("recording-disconnect-every") : null,
      disconnectOnRetryEnd: $("recording-disconnect-on-end").checked,
      splitMinutes: $("recording-split-minutes-enabled").checked ? number("recording-split-minutes") : null,
      splitSizeMb: $("recording-split-size-enabled").checked ? number("recording-split-size") : null,
      allDay: $("recording-all-day").checked,
      watchStart: $("recording-watch-start").value,
      watchEnd: $("recording-watch-end").value,
      monitorIntervalSecs: number("recording-monitor-interval"),
      rules: oldRules,
    };
  }
  function recordingRuleValue() { return state.selectedRecordingRule === null ? null : state.recording.rules[state.selectedRecordingRule] || null; }
  function commitRecordingRuleEditor() {
    const rule = recordingRuleValue(); if (!rule || state.loading) return;
    rule.name = $("recording-rule-name").value;
    rule.pattern = $("recording-rule-pattern").value;
    rule.enabled = $("recording-rule-enabled").checked;
    rule.fields = ruleFields.filter((field) => $(`recording-rule-field-${field}`).checked);
    rule.useRegex = $("recording-rule-regex").checked;
    rule.normalizeWidth = $("recording-rule-normalize-width").checked;
    rule.matchCase = $("recording-rule-match-case").checked;
    renderRecordingRulesList();
  }
  function syncRecordingRuleFields() {
    const rule = recordingRuleValue();
    $("recording-rule-name").value = rule?.name || "";
    $("recording-rule-pattern").value = rule?.pattern || "";
    $("recording-rule-enabled").checked = Boolean(rule?.enabled);
    ruleFields.forEach((field) => { $(`recording-rule-field-${field}`).checked = Boolean(rule?.fields.includes(field)); });
    $("recording-rule-regex").checked = Boolean(rule?.useRegex);
    $("recording-rule-normalize-width").checked = rule?.normalizeWidth !== false;
    $("recording-rule-match-case").checked = Boolean(rule?.matchCase);
    $("recording-rule-heading").textContent = rule?.name || "条件を選択";
    renderControls();
  }
  function renderRecordingRulesList() {
    const list = $("recording-rules-list");
    const query = state.recordingRulesSearch.trim().toLocaleLowerCase();
    const matches = state.recording.rules.map((rule, index) => ({ rule, index })).filter(({ rule }) => !query || `${rule.name} ${rule.pattern}`.toLocaleLowerCase().includes(query));
    $("recording-rules-count").textContent = `${state.recording.rules.length} 件`;
    $("recording-rules-empty").hidden = matches.length > 0;
    const fingerprint = JSON.stringify(matches.map(({ rule, index }) => [rule.id, rule.name, rule.pattern, rule.enabled, index === state.selectedRecordingRule]));
    if (fingerprint !== state.recordingRulesFingerprint) {
      state.recordingRulesFingerprint = fingerprint;
      list.replaceChildren();
      matches.forEach(({ rule, index }) => {
        const item = document.createElement("li");
        const button = document.createElement("button"); button.type = "button"; button.className = `rule-row recording-rule-row${index === state.selectedRecordingRule ? " selected" : ""}`; button.disabled = state.loading || !state.ready || state.busy;
        const enabled = document.createElement("input"); enabled.type = "checkbox"; enabled.checked = rule.enabled; enabled.disabled = state.loading || !state.ready || state.busy; enabled.setAttribute("aria-label", `${rule.name}を有効にする`);
        enabled.addEventListener("click", (event) => event.stopPropagation());
        enabled.addEventListener("change", (event) => { event.stopPropagation(); rule.enabled = enabled.checked; if (state.selectedRecordingRule === index) $("recording-rule-enabled").checked = enabled.checked; setDirty(); renderRecordingRulesList(); });
        const title = document.createElement("span"); title.className = "rule-row-title"; title.textContent = rule.name || "名前なし";
        const pattern = document.createElement("span"); pattern.className = "rule-row-pattern"; pattern.textContent = rule.pattern || "検索文字列なし";
        button.append(enabled, title, pattern); button.addEventListener("click", () => { commitRecordingRuleEditor(); state.selectedRecordingRule = index; syncRecordingRuleFields(); renderRecordingRulesList(); }); item.append(button); list.append(item);
      });
    }
    renderControls();
  }
  function recordingValidationError() {
    const value = state.recording;
    const intIn = (number, min, max) => Number.isInteger(number) && number >= min && number <= max;
    if (!value.directory.trim() || value.directory.length > 32767 || /[\u0000-\u001f\u007f]/.test(value.directory)) return "保存先フォルダーを入力してください。";
    if (!intIn(value.maxConcurrent, 1, 32) || !intIn(value.retryCount, 0, 1000) || !intIn(value.retryIntervalSecs, 1, 3600) || !intIn(value.monitorIntervalSecs, 10, 3600)) return "同時録画数、再試行、監視間隔の値が範囲外です。";
    if (value.disconnectEveryRetries !== null && !intIn(value.disconnectEveryRetries, 1, 1000)) return "接続し直す回数は1〜1000で指定してください。";
    if (value.splitMinutes !== null && !intIn(value.splitMinutes, 1, 10080)) return "時間分割は1〜10080分で指定してください。";
    if (value.splitSizeMb !== null && !intIn(value.splitSizeMb, 1, 1000000)) return "容量分割は1〜1000000 MBで指定してください。";
    const validTime = (time) => /^([01]\d|2[0-3]):[0-5]\d$/.test(time);
    if (!validTime(value.watchStart) || !validTime(value.watchEnd)) return "監視時刻を正しい時刻で指定してください。";
    if (value.rules.length > 128) return "録画条件は128件まで登録できます。";
    const invalid = value.rules.find((rule) => rule.name.length > 80 || rule.pattern.length > 2048 || rule.fields.some((field) => !ruleFields.includes(field)) || (rule.enabled && (!rule.pattern.trim() || !rule.fields.length)));
    if (invalid) return "有効な録画条件には検索文字列と検索対象が必要です。条件名は80文字、検索文字列は2048文字までです。";
    return "";
  }
  function renderList() {
    const list = $("yp-list");
    $("yp-count").textContent = `${state.pages.length} 件`;
    $("yp-empty").hidden = state.pages.length !== 0;
    const fingerprint = JSON.stringify(state.pages.map((page, index) => ({
      name: page.name,
      channelsUrl: page.channelsUrl,
      selected: state.selected === index,
      saved: state.savedPages.some((saved) => saved.name === page.name && saved.channelsUrl === page.channelsUrl),
    })));
    if (fingerprint === state.listFingerprint) {
      renderControls();
      return;
    }
    state.listFingerprint = fingerprint;
    list.replaceChildren();
    state.pages.forEach((page, index) => {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = `yp-row${state.selected === index ? " selected" : ""}`;
      button.setAttribute("aria-pressed", String(state.selected === index));
      button.disabled = state.loading || !state.ready || state.busy;
      button.setAttribute("aria-label", `${page.name}、${page.channelsUrl}`);
      const icon = document.createElement("span");
      icon.className = "yp-icon";
      const glyph = document.createElement("span");
      glyph.className = "icon icon-world";
      glyph.setAttribute("aria-hidden", "true");
      icon.append(glyph);
      const details = document.createElement("span");
      details.className = "yp-details";
      const name = document.createElement("span");
      name.className = "yp-name";
      name.textContent = page.name;
      const url = document.createElement("span");
      url.className = "yp-url";
      url.textContent = page.channelsUrl;
      details.append(name, url);
      const badge = document.createElement("span");
      const saved = state.savedPages.some((entry) => entry.name === page.name && entry.channelsUrl === page.channelsUrl);
      badge.className = `registered-badge${saved ? "" : " unsaved-badge"}`;
      badge.textContent = saved ? "登録済み" : "未保存";
      button.append(icon, details, badge);
      button.addEventListener("click", () => selectPage(index));
      item.append(button);
      list.append(item);
    });
    renderControls();
  }
  function syncPageFields() {
    const page = state.selected === null ? null : state.pages[state.selected];
    $("page-name").value = page?.name ?? "";
    $("page-url").value = page?.channelsUrl ?? "";
    $("editor-heading").textContent = page ? "選択したイエローページの設定" : "イエローページを選択";
    $("editor-subtitle").textContent = page ? "チャンネル一覧の取得先を編集" : "追加して一覧に登録できます";
    $("form-error").hidden = true;
  }
  function selectPage(index) {
    commitEditor();
    state.selected = index;
    syncPageFields();
    renderList();
  }
  function commitEditor() {
    if (state.selected === null || state.loading) return;
    state.pages[state.selected] = pageValue();
    renderList();
  }
  function applySnapshot(snapshot, allowEdits = true) {
    state.snapshot = snapshot;
    const snapshotLimits = snapshot.limits || {};
    const snapshotPages = Array.isArray(snapshot.yellowPages)
      ? snapshot.yellowPages.map((page) => ({ name: String(page.name || ""), channelsUrl: String(page.channelsUrl || "") }))
      : [];
    const desktop = snapshot.desktop || {};
    const snapshotPlayer = { executable: typeof desktop.player?.executable === "string" && desktop.player.executable ? desktop.player.executable : null, arguments: typeof desktop.player?.arguments === "string" ? desktop.player.arguments : defaultArguments };
    const snapshotRules = Array.isArray(desktop.channelRules) ? desktop.channelRules.map((rule) => normalizedRule(rule)) : [];
    const snapshotRecording = normalizeRecordingSettings(desktop.recording || {});
    const snapshotWindow = normalizeWindowPreferences(desktop.window || {});
    const snapshotDirectory = { youtubeAtBottom: desktop.directory?.youtubeAtBottom === true, twitchAtBottom: desktop.directory?.twitchAtBottom === true };
    const fingerprint = JSON.stringify({
      maxDirects: Number(snapshotLimits.maxDirects) || 1,
      maxRelays: Number(snapshotLimits.maxRelays) || 1,
      maxUpstreamRate: Number(snapshotLimits.maxUpstreamRate) || 1,
      yellowPages: snapshotPages,
      player: snapshotPlayer,
      channelRules: snapshotRules,
      recording: snapshotRecording,
      directory: snapshotDirectory,
      window: snapshotWindow,
    });
    if (allowEdits && !state.dirty && fingerprint !== state.preferenceFingerprint) {
      state.preferenceFingerprint = fingerprint;
      const limits = snapshot.limits || {};
      state.limits = {
        maxDirects: Number(limits.maxDirects) || 1,
        maxRelays: Number(limits.maxRelays) || 1,
        maxUpstreamRate: Number(limits.maxUpstreamRate) || 1,
      };
      $("max-directs").value = String(state.limits.maxDirects);
      $("max-relays").value = String(state.limits.maxRelays);
      $("max-upstream-rate").value = String(state.limits.maxUpstreamRate);
      state.pages = snapshotPages;
      state.savedPages = snapshotPages.map((page) => ({ ...page }));
      state.player = { ...snapshotPlayer };
      state.savedPlayer = { ...snapshotPlayer };
      state.rules = snapshotRules;
      state.savedRules = snapshotRules.map((rule) => ({ ...rule, fields: [...rule.fields] }));
      state.selectedRule = null;
      state.recording = { ...snapshotRecording, rules: snapshotRecording.rules.map((rule) => ({ ...rule, fields: [...rule.fields] })) };
      state.savedRecording = cloneRecording(state.recording);
      state.directory = snapshotDirectory;
      $("youtube-at-bottom").checked = state.directory.youtubeAtBottom;
      $("twitch-at-bottom").checked = state.directory.twitchAtBottom;
      state.window = { ...snapshotWindow };
      state.selectedWindowPreset = windowPresetKeyForSize(state.window);
      $("window-width").value = String(state.window.width);
      $("window-height").value = String(state.window.height);
      renderWindowPresetOptions(state.selectedWindowPreset);
      $("window-logical-size").checked = state.window.logicalSize;
      $("window-minimize-to-tray").checked = state.window.minimizeToTray;
      $("window-launch-at-startup").checked = state.window.launchAtStartup;
      state.selectedRecordingRule = state.recording.rules.length ? Math.min(state.selectedRecordingRule ?? 0, state.recording.rules.length - 1) : null;
      state.selected = state.pages.length ? Math.min(state.selected ?? 0, state.pages.length - 1) : null;
      syncPageFields();
      syncPlayerFields();
      syncRuleFields();
      renderRulesList();
      syncRecordingFields();
      syncRecordingRuleFields();
      renderRecordingRulesList();
      setStatus("設定を読み込みました");
    }
    const backendLabel = snapshot.backendLabel || (snapshot.backendKind === "csharp" ? "PeerCastStation (C#)" : "Peercast RT");
    $("backend-status").textContent = `${backendLabel} · ${snapshot.running ? "稼働中" : "停止中"}`;
    $("backend-endpoint").textContent = snapshot.endpoint || "—";
    $("backend-uptime").textContent = snapshot.running ? formatBackendUptime(snapshot.uptimeSecs) : "—";
    $("backend-tasks").textContent = snapshot.running ? String(Number(snapshot.activeTasks) || 0) : "—";
    $("config-path").textContent = snapshot.configPath || "—";
    const configuredPlayer = typeof snapshot.desktop?.player?.executable === "string" && snapshot.desktop.player.executable.trim() ? snapshot.desktop.player.executable : null;
    const playerPath = configuredPlayer || (typeof snapshot.playerPath === "string" && snapshot.playerPath.trim() ? snapshot.playerPath : "同梱プレイヤー");
    const playerStatus = typeof snapshot.playerAvailable === "boolean"
      ? snapshot.playerAvailable ? "利用可能" : (state.capabilities?.platform === "linux" ? "見つからないか実行権限がありません。設定から変更できます" : "見つかりません")
      : "状態を確認できません";
    $("player-status").textContent = `${playerPath}（${playerStatus}）`;
    $("player-editor-status").textContent = `保存済み設定: ${playerPath}（${playerStatus}）。${state.capabilities?.platform === "linux" ? " mpvを別途インストールするか、実行可能なプレイヤーの絶対パスを指定してください。" : ""}`;
    state.loading = false;
    state.ready = true;
    renderList();
  }
  function updateSnapshot(snapshot) {
    if (!snapshot) return;
    applySnapshot(snapshot, !state.dirty);
  }
  function isUpdateBusy() { return ["checking", "downloading", "applying"].includes(state.updateStatus?.phase); }
  function stopUpdatePolling() {
    if (state.updatePollTimer !== null) window.clearTimeout(state.updatePollTimer);
    state.updatePollTimer = null;
  }
  function renderUpdaterControls() {
    if (state.capabilities?.updater === false) {
      $("update-status").textContent = "この環境では自動更新は利用できません。Linux版は手動で更新してください。";
      $("update-current-version").textContent = "Linux開発版 — Windows更新パッケージは使用しません";
      $("about-current-version").textContent = "Linux開発版";
      ["update-check", "update-download", "update-cancel", "update-install", "update-later"].forEach((id) => { $(id).disabled = true; $(id).hidden = true; });
      ["update-loading", "update-progress", "update-progress-label", "update-warning", "update-ready-note"].forEach((id) => { $(id).hidden = true; });
      return;
    }
    const status = state.updateStatus;
    const model = updateCardModel(status);
    const locked = !status || state.updateStatusLoading || state.updateActionBusy || model.busy;
    const currentVersion = $("update-current-version");
    if (!currentVersion) return;
    currentVersion.textContent = status?.currentVersion ? `現在のバージョン ${status.currentVersion}` : "現在のバージョンを確認しています";
    $("about-current-version").textContent = status?.currentVersion ? `バージョン ${status.currentVersion}` : "バージョンを確認しています";
    $("update-status").textContent = model.message;
    renderUpdateNotes($("update-notes"), status?.notes);
    const progress = $("update-progress");
    progress.hidden = !model.hasProgress;
    progress.classList.toggle("indeterminate", model.indeterminate);
    if (model.percent === null) progress.removeAttribute("value");
    else progress.value = model.percent;
    $("update-progress-label").textContent = model.hasProgress ? model.progressText : "";
    $("update-check").hidden = !model.check;
    $("update-download").hidden = !model.download;
    $("update-cancel").hidden = !model.cancel;
    $("update-install").hidden = !model.install;
    $("update-later").hidden = !model.later;
    $("update-ready-note").hidden = !model.later;
    $("update-warning").hidden = !model.install;
    $("update-ready-note").textContent = state.updateDeferred
      ? "更新データを取得済みです。アプリの終了前にこの画面から後で適用できます。"
      : "更新データは取得済みです。今すぐ適用せず、後からこの画面で適用できます。";
    $("update-check").disabled = locked;
    $("update-download").disabled = locked;
    $("update-cancel").disabled = state.updateActionBusy || state.updateStatusLoading;
    $("update-install").disabled = locked;
    $("update-later").disabled = state.updateActionBusy;
    $("update-loading").hidden = !state.updateStatusLoading;
  }
  function scheduleUpdatePoll() {
    if (state.capabilities?.updater === false) return;
    stopUpdatePolling();
    if (!state.updateViewActive || state.updateDisposed || document.hidden || !isUpdateBusy()) return;
    state.updatePollTimer = window.setTimeout(pollUpdateStatus, 500);
  }
  async function pollUpdateStatus() {
    if (state.capabilities?.updater === false) return;
    state.updatePollTimer = null;
    if (!state.updateViewActive || state.updateDisposed || document.hidden || !isUpdateBusy() || state.updatePollRequest) return;
    state.updatePollRequest = invoke("update_status").then((status) => {
      state.updateStatus = normalizeUpdateStatus(status);
    }).catch(() => {
      // Keep the last known phase during a transient status request failure.
    }).finally(() => {
      state.updatePollRequest = null;
      renderUpdaterControls();
      scheduleUpdatePoll();
    });
    await state.updatePollRequest;
  }
  async function ensureUpdateStatus() {
    if (state.capabilities?.updater === false) return;
    if (state.updateStatus) { renderUpdaterControls(); scheduleUpdatePoll(); return; }
    if (state.updateStatusLoading) return state.updatePollRequest;
    state.updateStatusLoading = true;
    renderUpdaterControls();
    state.updatePollRequest = invoke("update_status").then((status) => {
      state.updateStatus = normalizeUpdateStatus(status);
    }).catch(() => {
      state.updateStatus = normalizeUpdateStatus({ phase: "error" });
    }).finally(() => {
      state.updateStatusLoading = false;
      state.updatePollRequest = null;
      renderUpdaterControls();
      scheduleUpdatePoll();
    });
    return state.updatePollRequest;
  }
  async function runUpdateAction(command) {
    if (state.capabilities?.updater === false) return;
    if (!state.updateViewActive || state.updateActionBusy || state.updateStatusLoading
      || (isUpdateBusy() && command !== "cancel_update_download")
      || (command === "cancel_update_download" && state.updateStatus?.phase !== "downloading")) return;
    if (command === "install_update" && !window.confirm("更新を適用するとアプリが再起動し、現在の録画・配信は終了します。続行しますか？")) return;
    state.updateActionBusy = true;
    const optimisticPhase = command === "check_update" ? "checking"
      : command === "download_update" ? "downloading"
        : command === "install_update" ? "applying" : state.updateStatus?.phase;
    if (optimisticPhase) state.updateStatus = normalizeUpdateStatus({ ...state.updateStatus, phase: optimisticPhase });
    renderUpdaterControls();
    scheduleUpdatePoll();
    try {
      state.updateStatus = normalizeUpdateStatus(await invoke(command));
      state.updateDeferred = false;
    } catch (_) {
      state.updateStatus = normalizeUpdateStatus({ ...state.updateStatus, phase: "error" });
    } finally {
      state.updateActionBusy = false;
      renderUpdaterControls();
      scheduleUpdatePoll();
    }
  }
  function activate(page) {
    if (!meta[page]) return;
    if (root.querySelector(".nav-item.active")?.dataset.page !== page) {
      setNotice("");
      if (!state.busy && !state.loading) setStatus(state.dirty ? "未保存の変更があります" : "", { persistent: true });
    }
    if (page !== "channel-rules") {
      clearRuleSelection();
      $("saved-rules-disclosure").open = false;
    }
    root.querySelectorAll(".nav-item").forEach((tab) => {
      const active = tab.dataset.page === page;
      tab.classList.toggle("active", active);
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    });
    root.querySelectorAll(".page-content").forEach((panel) => { panel.hidden = panel.id !== `panel-${page}`; });
    $("page-title").textContent = meta[page][0];
    $("page-description").textContent = meta[page][1];
    state.updateViewActive = page === "version";
    if (page === "channel-rules" && state.selectedRule !== null) $("saved-rules-disclosure").open = true;
    if (state.updateViewActive) ensureUpdateStatus();
    else stopUpdatePolling();
    if (page === "yellow-pages") {
      if (!state.youtubeLoaded) void loadYoutubeSettings();
      if (!state.twitchLoaded) void loadTwitchSettings();
    }
  }
  function activateYellowPagesSubtab(name, focus = false) {
    if (state.capabilities?.liveServices === false && name !== "yp") return;
    const tabs = [...root.querySelectorAll("[data-yp-subtab]")];
    const activeTab = tabs.find((tab) => tab.dataset.ypSubtab === name);
    if (!activeTab) return;
    tabs.forEach((tab) => {
      const active = tab === activeTab;
      tab.classList.toggle("active", active);
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    });
    root.querySelectorAll("[data-yp-panel]").forEach((panel) => { panel.hidden = panel.dataset.ypPanel !== name; });
    if (focus) activeTab.focus();
  }
  const ypSubtabs = root.querySelector("[role=tablist][aria-label='イエローページの表示']");
  ypSubtabs?.addEventListener("click", (event) => {
    const tab = event.target.closest("[data-yp-subtab]");
    if (tab) activateYellowPagesSubtab(tab.dataset.ypSubtab);
  });
  ypSubtabs?.addEventListener("keydown", (event) => {
    const tabs = [...ypSubtabs.querySelectorAll("[data-yp-subtab]")].filter((tab) => !tab.disabled);
    const index = tabs.indexOf(event.target.closest("[data-yp-subtab]"));
    if (index < 0) return;
    const nextIndex = event.key === "Home" ? 0
      : event.key === "End" ? tabs.length - 1
        : event.key === "ArrowRight" ? (index + 1) % tabs.length
          : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length : -1;
    if (nextIndex < 0) return;
    event.preventDefault();
    activateYellowPagesSubtab(tabs[nextIndex].dataset.ypSubtab, true);
  });
  $("saved-rules-disclosure").addEventListener("toggle", handleRuleDisclosureToggle);
  async function save() {
    if (state.loading || state.busy || !state.dirty) return;
    commitEditor();
    commitRuleEditor();
    commitPlayerFields();
    commitRecordingFields();
    commitRecordingRuleEditor();
    const limits = {
      maxDirects: Number($("max-directs").value),
      maxRelays: Number($("max-relays").value),
      maxUpstreamRate: Number($("max-upstream-rate").value),
    };
    const formError = $("form-error");
    const invalidPage = state.pages.find((page) => !page.name.trim() || !page.channelsUrl.trim());
    if (!isValidWindowSize(state.window.width, state.window.height)) {
      setNotice("ウィンドウサイズは幅1280〜7680px、高さ720〜4320pxで入力してください。", true);
      activate("appearance");
      return;
    }
    const invalidPreset = state.window.presets.length > 32 || state.window.presets.some((preset, index, presets) =>
      !preset.id.trim() || [...preset.id].length > 80 || !preset.name.trim() || [...preset.name.trim()].length > 40
      || !isValidWindowSize(preset.width, preset.height)
      || presets.findIndex((candidate) => candidate.id === preset.id) !== index
      || presets.findIndex((candidate) => candidate.name.trim().toLowerCase() === preset.name.trim().toLowerCase()) !== index);
    if (invalidPreset) {
      setNotice("登録プリセットの名前、サイズ、重複または登録数を確認してください。", true);
      activate("appearance");
      return;
    }
    if (!Number.isInteger(limits.maxDirects) || limits.maxDirects < 1 || limits.maxDirects > 256 || !Number.isInteger(limits.maxRelays) || limits.maxRelays < 1 || limits.maxRelays > 256 || !Number.isInteger(limits.maxUpstreamRate) || limits.maxUpstreamRate < 1 || limits.maxUpstreamRate > 10000000) {
      setNotice("接続数は1〜256、上り帯域は1〜10000000 kbpsで入力してください。", true);
      return;
    }
    if (state.pages.length > 32 || invalidPage) {
      formError.textContent = state.pages.length > 32 ? "イエローページは32件まで登録できます。" : "名前とチャンネル一覧URLを入力してください。";
      formError.hidden = false;
      activate("yellow-pages");
      return;
    }
    if (!$("use-bundled-player").checked && !state.player.executable) {
      setNotice("同梱プレイヤーを使わない場合は、実行ファイルを選択してください。", true);
      activate("player");
      return;
    }
    const invalidRule = state.rules.find((rule) => rule.name.length > 80 || rule.pattern.length > 2048 || !Array.isArray(rule.fields) || rule.fields.some((field) => !ruleFields.includes(field)) || (rule.enabled && (!rule.pattern.trim() || !rule.fields.length)));
    if (state.rules.length > 128 || invalidRule) {
      $("rules-error").textContent = state.rules.length > 128 ? "条件は128件まで登録できます。" : "ルール名は80文字、検索文字列は2048文字までです。有効な条件には検索文字列と検索対象が必要です。";
      $("rules-error").hidden = false;
      activate("channel-rules");
      return;
    }
    const recordingError = recordingValidationError();
    if (recordingError) {
      $("recording-form-error").textContent = recordingError;
      $("recording-form-error").hidden = false;
      activate("recording");
      return;
    }
    setBusy(true);
    setNotice("");
    setStatus("保存しています…", { persistent: true });
    try {
      const desktop = state.snapshot?.desktop || {};
      const directory = { ...(desktop.directory || { autoRefresh: true, refreshIntervalSecs: 300 }), ...state.directory };
      const snapshot = await invoke("apply_preferences", { limits, yellowPages: state.pages, desktop: { ...desktop, directory, window: { ...state.window }, player: state.player, channelRules: state.rules.map((rule) => normalizedRule(rule)), recording: normalizeRecordingSettings(state.recording) } });
      state.dirty = false;
      state.preferenceFingerprint = null;
      state.savedPlayer = { ...state.player };
      state.savedRules = state.rules.map((rule) => ({ ...rule, fields: [...rule.fields] }));
      state.savedRecording = cloneRecording(state.recording);
      state.selected = state.pages.length ? Math.min(state.selected ?? 0, state.pages.length - 1) : null;
      applySnapshot(snapshot, true);
      setStatus("変更を保存しました");
      setNotice("設定を保存しました。");
      if (typeof onSaved === "function") {
        try { onSaved(snapshot); }
        catch (error) { setNotice(`設定は保存されましたが、画面の更新に失敗しました: ${error.message || error}`, true); }
      }
    } catch (error) {
      setStatus("保存できませんでした");
      setNotice(`設定を保存できませんでした: ${error.message || error}`, true);
    } finally { setBusy(false); }
  }
  async function requestLeave() {
    if (state.busy) {
      setNotice("保存処理中です。完了後にもう一度移動してください。", true);
      return false;
    }
    if (!state.dirty) {
      setNotice("");
      setStatus("");
      setProviderMessage("youtube", "");
      setProviderMessage("twitch", "");
      return true;
    }
    if (state.leavePromise) return state.leavePromise;
    const pending = state.leavePromise = new Promise((resolve) => { state.resolveLeave = resolve; });
    try {
      $("discard-dialog").returnValue = "";
      $("discard-dialog").showModal();
    } catch (error) {
      const resolve = state.resolveLeave;
      state.resolveLeave = null;
      state.leavePromise = null;
      setNotice(`確認画面を表示できませんでした: ${error.message || error}`, true);
      resolve?.(false);
    }
    return pending;
  }

  root.querySelectorAll(".nav-item").forEach((tab) => {
    tab.addEventListener("click", () => activate(tab.dataset.page));
    tab.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const tabs = [...root.querySelectorAll(".nav-item")];
      const index = tabs.indexOf(event.currentTarget);
      const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length;
      tabs[next].focus();
      activate(tabs[next].dataset.page);
    });
  });
  $("update-check").addEventListener("click", () => { void runUpdateAction("check_update"); });
  $("update-download").addEventListener("click", () => { void runUpdateAction("download_update"); });
  $("update-cancel").addEventListener("click", () => { void runUpdateAction("cancel_update_download"); });
  $("update-install").addEventListener("click", () => { void runUpdateAction("install_update"); });
  $("update-later").addEventListener("click", () => { state.updateDeferred = true; renderUpdaterControls(); });
  const onUpdateVisibilityChange = () => {
    if (document.hidden) stopUpdatePolling();
    else if (state.updateViewActive) scheduleUpdatePoll();
  };
  const onUpdatePageHide = () => { state.updateDisposed = true; stopUpdatePolling(); };
  document.addEventListener("visibilitychange", onUpdateVisibilityChange);
  window.addEventListener("pagehide", onUpdatePageHide, { once: true });
  ["max-directs", "max-relays", "max-upstream-rate"].forEach((id) => $(id).addEventListener("input", setDirty));
  $("window-preset").addEventListener("change", () => {
    const key = $("window-preset").value;
    let preset;
    if (key.startsWith("user:")) {
      preset = state.window.presets.find((item) => `user:${item.id}` === key);
      if (!preset) return;
      state.window = applyWindowPreset(state.window, preset);
    } else {
      preset = parseWindowPreset(key);
      if (preset) state.window = applyWindowPreset(state.window, preset);
    }
    state.selectedWindowPreset = key;
    $("window-width").value = String(state.window.width);
    $("window-height").value = String(state.window.height);
    renderControls();
    setDirty();
  });
  ["window-width", "window-height"].forEach((id) => $(id).addEventListener("input", () => {
    state.window.width = Number($("window-width").value);
    state.window.height = Number($("window-height").value);
    state.selectedWindowPreset = windowPresetKeyForSize(state.window);
    renderWindowPresetOptions(state.selectedWindowPreset);
    renderControls();
    setDirty();
  }));
  $("window-preset-add").addEventListener("click", () => {
    const uuid = window.crypto?.randomUUID?.() || `window-preset-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    const result = addWindowPreset(state.window, $("window-preset-name").value, uuid);
    const error = $("window-preset-error");
    error.textContent = result.error || "";
    error.hidden = !result.error;
    if (result.error) return;
    state.window = result.window;
    state.selectedWindowPreset = `user:${result.preset.id}`;
    $("window-preset-name").value = "";
    renderWindowPresetOptions();
    renderControls();
    setDirty();
  });
  $("window-preset-remove").addEventListener("click", () => {
    if (!state.selectedWindowPreset.startsWith("user:")) return;
    const id = state.selectedWindowPreset.slice("user:".length);
    state.window = removeWindowPreset(state.window, id);
    state.selectedWindowPreset = windowPresetKeyForSize(state.window);
    renderWindowPresetOptions();
    renderControls();
    setDirty();
  });
  ["window-minimize-to-tray", "window-launch-at-startup"].forEach((id) => $(id).addEventListener("change", () => {
    state.window.minimizeToTray = $("window-minimize-to-tray").checked;
    state.window.launchAtStartup = $("window-launch-at-startup").checked;
    setDirty();
  }));
  function updateWindowLogicalSize() {
    state.window.logicalSize = $("window-logical-size").checked;
    setDirty();
  }
  $("window-logical-size").addEventListener("change", updateWindowLogicalSize);
  $("youtube-add-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const input = $("youtube-url");
    const url = String(input.value || "").trim();
    if (!url) return;
    void youtubeSettingsMutation("youtube_add", { url }).then((saved) => { if (saved) input.value = ""; });
  });
  $("youtube-subscriptions").addEventListener("click", (event) => {
    const button = event.target.closest("[data-youtube-action]");
    if (!button || state.youtubeBusy) return;
    const id = button.dataset.youtubeId;
    if (!id) return;
    if (button.dataset.youtubeAction === "remove") {
      void youtubeSettingsMutation("youtube_remove", { id });
      return;
    }
    const channel = state.youtubeSnapshot?.channels?.find((entry) => String(entry.id) === String(id));
    if (channel) void youtubeSettingsMutation("youtube_set_enabled", { id, enabled: channel.enabled !== true });
  });
  $("youtube-refresh").addEventListener("click", () => { void youtubeSettingsMutation("youtube_refresh", {}); });
  $("twitch-add-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const input = $("twitch-url");
    const url = String(input.value || "").trim();
    if (!url) return;
    void twitchSettingsMutation("twitch_add", { url }).then((saved) => { if (saved) input.value = ""; });
  });
  $("twitch-subscriptions").addEventListener("click", (event) => {
    const button = event.target.closest("[data-twitch-action]");
    if (!button || state.twitchBusy) return;
    const id = button.dataset.twitchId;
    if (!id) return;
    if (button.dataset.twitchAction === "remove") {
      void twitchSettingsMutation("twitch_remove", { id });
      return;
    }
    const channel = state.twitchSnapshot?.channels?.find((entry) => String(entry.id) === String(id));
    if (channel) void twitchSettingsMutation("twitch_set_enabled", { id, enabled: channel.enabled !== true });
  });
  $("twitch-refresh").addEventListener("click", () => { void twitchSettingsMutation("twitch_refresh", {}); });
  $("page-name").addEventListener("input", () => { commitEditor(); setDirty(); });
  $("page-url").addEventListener("input", () => { commitEditor(); setDirty(); });
  $("add-page").addEventListener("click", () => {
    commitEditor();
    state.pages.push({ name: "", channelsUrl: "" });
    state.selected = state.pages.length - 1;
    setDirty();
    syncPageFields();
    renderList();
    $("page-name").focus();
  });
  $("delete-page").addEventListener("click", () => {
    if (state.selected === null) return;
    state.pages.splice(state.selected, 1);
    state.selected = state.pages.length ? Math.min(state.selected, state.pages.length - 1) : null;
    setDirty();
    syncPageFields();
    renderList();
  });
  $("use-bundled-player").addEventListener("change", () => { commitPlayerFields(); setDirty(); renderControls(); });
  $("player-executable").addEventListener("input", () => { commitPlayerFields(); setDirty(); });
  $("player-arguments").addEventListener("input", () => { commitPlayerFields(); setDirty(); });
  $("choose-player").addEventListener("click", async () => {
    try { const path = await invoke("choose_file", { kind: "player" }); if (path) { $("use-bundled-player").checked = false; $("player-executable").value = String(path); commitPlayerFields(); setDirty(); renderControls(); } }
    catch (error) { setNotice(`プレイヤーを選択できませんでした: ${error.message || error}`, true); }
  });
  root.querySelectorAll("[data-player-tag]").forEach((button) => button.addEventListener("click", () => {
    const input = $("player-arguments");
    const start = input.selectionStart ?? input.value.length, end = input.selectionEnd ?? start;
    input.setRangeText(button.dataset.playerTag, start, end, "end"); input.focus(); commitPlayerFields(); setDirty();
  }));
  $("rules-search").addEventListener("input", () => { state.rulesSearch = $("rules-search").value; state.rulesFingerprint = null; renderRulesList(); });
  $("add-rule").addEventListener("click", () => {
    if (state.rules.length >= 128) return;
    commitRuleEditor(); state.rules.push(newRule()); state.selectedRule = state.rules.length - 1; $("saved-rules-disclosure").open = true; syncRuleFields(); setDirty(); renderRulesList(); $("rule-name").focus();
  });
  $("delete-rule").addEventListener("click", () => {
    if (state.selectedRule === null) return;
    state.rules.splice(state.selectedRule, 1); state.selectedRule = state.rules.length ? Math.min(state.selectedRule, state.rules.length - 1) : null; syncRuleFields(); setDirty(); renderRulesList();
  });
  function moveRule(delta) {
    const index = state.selectedRule;
    if (index === null || state.rulesSearch || index + delta < 0 || index + delta >= state.rules.length) return;
    commitRuleEditor(); [state.rules[index], state.rules[index + delta]] = [state.rules[index + delta], state.rules[index]]; state.selectedRule = index + delta; setDirty(); syncRuleFields(); renderRulesList();
  }
  $("move-rule-up").addEventListener("click", () => moveRule(-1));
  $("move-rule-down").addEventListener("click", () => moveRule(1));
  const ruleEditorIds = ["rule-name", "rule-pattern", "rule-enabled", "rule-regex", "rule-normalize-width", "rule-match-case", "rule-favorite", "rule-hidden", "rule-mute", "rule-text-color-enabled", "rule-text-color", "rule-background-color-enabled", "rule-background-color", "rule-sound-path", ...ruleFields.map((field) => `rule-field-${field}`)];
  ruleEditorIds.forEach((id) => $(id).addEventListener("input", () => { commitRuleEditor(); setDirty(); renderControls(); }));
  ruleEditorIds.filter((id) => $(id).type === "checkbox").forEach((id) => $(id).addEventListener("change", () => { commitRuleEditor(); setDirty(); renderControls(); }));
  $("choose-rule-sound").addEventListener("click", async () => {
    try { const path = await invoke("choose_file", { kind: "sound" }); if (path) { $("rule-sound-path").value = String(path); commitRuleEditor(); setDirty(); renderControls(); } }
    catch (error) { setNotice(`通知音を選択できませんでした: ${error.message || error}`, true); }
  });
  $("preview-rule-sound").addEventListener("click", async () => {
    const rule = ruleValue(); if (!rule?.soundPath || state.previewing) return;
    state.previewing = true; renderControls();
    try { await invoke("preview_sound", { path: rule.soundPath }); }
    catch (error) { setNotice(`通知音を再生できませんでした: ${error.message || error}`, true); }
    finally { state.previewing = false; renderControls(); }
  });
  $("preview-rules").addEventListener("click", async () => {
    commitRuleEditor(); if (state.previewing) return;
    state.previewing = true; $("rules-error").hidden = true; $("rules-preview").textContent = "照合しています…"; renderControls();
    try {
      const result = await invoke("preview_channel_rules", { rules: state.rules.map((rule) => normalizedRule(rule)) });
      const counts = `対象 ${Number(result?.total) || 0}件 / 一致 ${Number(result?.matched) || 0}件 / お気に入り ${Number(result?.favorites) || 0}件 / 非表示 ${Number(result?.hidden) || 0}件`;
      const names = Array.isArray(result?.channels) ? result.channels.slice(0, 8).map((entry) => `${entry.name || "名前なし"}（${entry.isHidden ? "NG" : entry.isFavorite ? "お気に入り" : "通常"}）`).join("、") : "";
      $("rules-preview").textContent = names ? `${counts}\n${names}` : counts;
    } catch (error) { $("rules-error").textContent = `プレビューできませんでした: ${error.message || error}`; $("rules-error").hidden = false; }
    finally { state.previewing = false; renderControls(); }
  });
  const recordingInputIds = ["recording-enabled", "recording-directory", "recording-subfolders", "recording-limit-concurrent", "recording-max-concurrent", "recording-retry-count", "recording-retry-interval", "recording-disconnect-every-enabled", "recording-disconnect-every", "recording-disconnect-on-end", "recording-split-minutes-enabled", "recording-split-minutes", "recording-split-size-enabled", "recording-split-size", "recording-all-day", "recording-watch-start", "recording-watch-end", "recording-monitor-interval"];
  recordingInputIds.forEach((id) => {
    const element = $(id);
    const changed = () => { commitRecordingFields(); $("recording-form-error").hidden = true; setDirty(); renderControls(); };
    element.addEventListener("input", changed);
    element.addEventListener("change", changed);
  });
  $("choose-recording-directory").addEventListener("click", async () => {
    try { const path = await invoke("choose_file", { kind: "directory" }); if (path) { $("recording-directory").value = String(path); commitRecordingFields(); setDirty(); } }
    catch (error) { setNotice(`保存先を選択できませんでした: ${error.message || error}`, true); }
  });
  $("recording-rules-search").addEventListener("input", () => { state.recordingRulesSearch = $("recording-rules-search").value; state.recordingRulesFingerprint = null; renderRecordingRulesList(); });
  $("add-recording-rule").addEventListener("click", () => {
    if (state.recording.rules.length >= 128) return;
    commitRecordingRuleEditor(); state.recording.rules.push(newRecordingFilter()); state.selectedRecordingRule = state.recording.rules.length - 1; syncRecordingRuleFields(); setDirty(); renderRecordingRulesList(); $("recording-rule-name").focus();
  });
  $("duplicate-recording-rule").addEventListener("click", () => {
    const rule = recordingRuleValue(); if (!rule || state.recording.rules.length >= 128) return;
    commitRecordingRuleEditor(); state.recording.rules.splice(state.selectedRecordingRule + 1, 0, newRecordingFilter(rule)); state.selectedRecordingRule += 1; syncRecordingRuleFields(); setDirty(); renderRecordingRulesList();
  });
  $("delete-recording-rule").addEventListener("click", () => {
    if (state.selectedRecordingRule === null) return;
    state.recording.rules.splice(state.selectedRecordingRule, 1); state.selectedRecordingRule = state.recording.rules.length ? Math.min(state.selectedRecordingRule, state.recording.rules.length - 1) : null; syncRecordingRuleFields(); setDirty(); renderRecordingRulesList();
  });
  function moveRecordingRule(delta) {
    const index = state.selectedRecordingRule;
    if (index === null || state.recordingRulesSearch.trim() || index + delta < 0 || index + delta >= state.recording.rules.length) return;
    commitRecordingRuleEditor(); [state.recording.rules[index], state.recording.rules[index + delta]] = [state.recording.rules[index + delta], state.recording.rules[index]]; state.selectedRecordingRule = index + delta; setDirty(); syncRecordingRuleFields(); renderRecordingRulesList();
  }
  $("recording-rule-up").addEventListener("click", () => moveRecordingRule(-1));
  $("recording-rule-down").addEventListener("click", () => moveRecordingRule(1));
  const recordingRuleEditorIds = ["recording-rule-name", "recording-rule-pattern", "recording-rule-enabled", "recording-rule-regex", "recording-rule-normalize-width", "recording-rule-match-case", ...ruleFields.map((field) => `recording-rule-field-${field}`)];
  recordingRuleEditorIds.forEach((id) => {
    const element = $(id); const changed = () => { commitRecordingRuleEditor(); $("recording-rules-error").hidden = true; setDirty(); renderControls(); };
    element.addEventListener("input", changed);
    element.addEventListener("change", changed);
  });
  $("preview-recording-rules").addEventListener("click", async () => {
    commitRecordingRuleEditor(); if (state.previewing) return;
    state.previewing = true; $("recording-rules-error").hidden = true; $("recording-rules-preview").textContent = "照合しています…"; renderControls();
    try {
      const result = await invoke("preview_recording_rules", { rules: state.recording.rules.map((rule) => normalizedRecordingFilter(rule)) });
      const counts = `対象 ${Number(result?.total) || 0}件 / 一致 ${Number(result?.matched) || 0}件`;
      const names = Array.isArray(result?.channels) ? result.channels.slice(0, 8).map((entry) => String(entry.name || "名前なし")).join("、") : "";
      $("recording-rules-preview").textContent = names ? `${counts}\n${names}` : counts;
    } catch (error) { $("recording-rules-error").textContent = `プレビューできませんでした: ${error.message || error}`; $("recording-rules-error").hidden = false; }
    finally { state.previewing = false; renderControls(); }
  });
  $("save-button").addEventListener("click", save);
  $("yp-form").addEventListener("submit", (event) => { event.preventDefault(); save(); });
  $("cancel-button").addEventListener("click", () => {
    if (typeof onBack === "function") onBack();
    else void requestLeave();
  });
  $("youtube-at-bottom").addEventListener("change", () => { state.directory.youtubeAtBottom = $("youtube-at-bottom").checked; setDirty(); });
  $("twitch-at-bottom").addEventListener("change", () => { state.directory.twitchAtBottom = $("twitch-at-bottom").checked; setDirty(); });
  $("discard-dialog").addEventListener("close", () => {
    const discard = $("discard-dialog").returnValue === "discard";
    $("discard-dialog").returnValue = "";
    if (discard) {
      state.dirty = false;
      state.preferenceFingerprint = null;
      if (state.snapshot) applySnapshot(state.snapshot, true);
      setNotice("");
      setStatus("変更を破棄しました");
    }
    const resolve = state.resolveLeave;
    state.resolveLeave = null;
    state.leavePromise = null;
    resolve?.(discard);
  });
  $("discard-and-close").addEventListener("click", (event) => { event.preventDefault(); $("discard-dialog").close("discard"); });
  $("continue-editing").addEventListener("click", (event) => { event.preventDefault(); $("discard-dialog").close("continue"); });


  function renderLiveDependencies() {
    const dependencies = state.capabilities.liveDependencies;
    const panel = $("live-dependencies-panel");
    if (panel) panel.hidden = state.capabilities.platform !== "linux";
    const status = $("settings-live-dependencies-status");
    if (status) status.textContent = dependencies?.message || "ライブ再生の依存関係を確認してください。";
  }
  window.addEventListener("live-dependencies-updated", (event) => {
    state.capabilities.liveDependencies = event.detail;
    renderLiveDependencies();
  });
  $("live-dependencies-recheck")?.addEventListener("click", async () => {
    const button = $("live-dependencies-recheck");
    if (button.disabled) return;
    button.disabled = true;
    try {
      const dependencies = await invoke("live_dependencies_refresh", {});
      window.dispatchEvent(new CustomEvent("live-dependencies-rechecked", { detail: dependencies }));
      state.capabilities.liveDependencies = dependencies;
      renderLiveDependencies();
    } catch (error) {
      $("settings-live-dependencies-status").textContent = `依存関係を確認できませんでした: ${error.message || error}`;
    } finally { button.disabled = false; }
  });
  renderLiveDependencies();

  if (state.capabilities.liveServices === false) {
    ["youtube", "twitch"].forEach((provider) => {
      $("tab-" + provider).disabled = true;
      $("tab-" + provider).title = "このプラットフォームでは未対応です";
      $(provider + "-status").textContent = "このプラットフォームではYouTube/Twitchは未対応です。";
    });
    const note = $("linux-services-note");
    if (note) note.hidden = false;
  }
  if (state.capabilities.platform === "linux") {
    $("twitch-player-note").textContent = "現在のライブをmpvで再生できます。YouTube/Twitchの録画はこの版では対応していません。表示順は「変更を保存」で反映されます。追加・監視切替・解除はすぐに保存されます。";
    $("default-player-label").textContent = "標準の外部プレイヤー（mpv）を使う";
    $("player-editor-status").textContent = "mpvを別途インストールしてください。別のプレイヤーは絶対パスで指定します。";
    $("player-executable").placeholder = "/usr/bin/mpv";
    $("player-platform-note").textContent = 'Linuxの既定引数は "<stream/>" のみです。固定引数の空白は二重引用符で囲みます。シェルは実行されません。';
    $("about-platform-description").textContent = "PeerCastの視聴とP2Pリレーを行うLinux開発版（外部プレイヤー使用）";
  }
  renderControls();
  if (initialSnapshot) applySnapshot(initialSnapshot, true);
  else invoke("snapshot").then((snapshot) => {
    applySnapshot(snapshot, true);
  }).catch((error) => {
    state.loading = false;
    state.ready = false;
    setStatus("バックエンドに接続できません", { persistent: true });
    setNotice(`設定を読み込めませんでした: ${error.message || error}`, true);
    renderControls();
  });
  void loadYoutubeSettings();
  void loadTwitchSettings();
  return {
    requestLeave, updateSnapshot, isDirty: () => state.dirty,
    openYellowPagesSubtab(provider) {
      activate("yellow-pages");
      activateYellowPagesSubtab(provider === "twitch" ? "twitch" : "youtube");
    },
    addChannelRule(channel, kind = "favorite") {
      const rule = newRule(kind === "ng" ? "ng" : "favorite", channel);
      if (state.rules.length >= 128) { activate("channel-rules"); setNotice("条件は128件まで登録できます。", true); return false; }
      state.rules.push(rule); state.selectedRule = state.rules.length - 1; state.rulesSearch = ""; $("rules-search").value = ""; activate("channel-rules"); $("saved-rules-disclosure").open = true; syncRuleFields(); setDirty(); renderRulesList(); return true;
    },
  };
  };
})();
