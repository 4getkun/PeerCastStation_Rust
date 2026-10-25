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

  window.createSettingsView = (root, { onBack, onSaved, initialSnapshot } = {}) => {
  const invoke = (name, args) => {
    const call = window.__TAURI__?.core?.invoke;
    return typeof call === "function"
      ? call(name, args)
      : Promise.reject(new Error("設定画面はアプリ内でのみ利用できます。バックエンドに接続できません。"));
  };
  const $ = (id) => root.querySelector(`#${id}`);
  const defaultArguments = `"<stream/>" "<channelname/>" "<contact/>"`;
  function normalizeWindowPreferences(value = {}) {
    const width = value.width == null || value.width === "" ? Number.NaN : Number(value.width);
    const height = value.height == null || value.height === "" ? Number.NaN : Number(value.height);
    return {
      width: Number.isFinite(width) ? Math.trunc(width) : 1600,
      height: Number.isFinite(height) ? Math.trunc(height) : 900,
      minimizeToTray: Boolean(value.minimizeToTray),
      launchAtStartup: Boolean(value.launchAtStartup),
    };
  }
  function parseWindowPreset(value) {
    const match = /^(\d{3,4})x(\d{3,4})$/.exec(String(value).trim());
    return match ? { width: Number(match[1]), height: Number(match[2]) } : null;
  }
  function isValidWindowSize(width, height) {
    return Number.isInteger(width) && width >= 1280 && width <= 2560
      && Number.isInteger(height) && height >= 720 && height <= 1440;
  }
  const ruleFields = ["name", "details", "comment", "contactUrl", "yellowPage", "streamUrl", "contentType", "listeners", "bitrate"];
  const recordingDefaults = { enabled: false, directory: "recordings", channelSubfolders: true, limitConcurrent: true, maxConcurrent: 5, retryCount: 30, retryIntervalSecs: 10, disconnectEveryRetries: 5, disconnectOnRetryEnd: true, splitMinutes: null, splitSizeMb: null, allDay: true, watchStart: "00:00", watchEnd: "23:59", monitorIntervalSecs: 60, rules: [] };
  function cloneRecording(value) { return { ...value, rules: (value.rules || []).map((rule) => ({ ...rule, fields: [...rule.fields] })) }; }
  const state = { snapshot: null, limits: null, pages: [], savedPages: [], selected: null, player: { executable: null, arguments: defaultArguments }, savedPlayer: { executable: null, arguments: defaultArguments }, rules: [], savedRules: [], selectedRule: null, rulesSearch: "", recording: cloneRecording(recordingDefaults), savedRecording: cloneRecording(recordingDefaults), window: normalizeWindowPreferences(), selectedRecordingRule: null, recordingRulesSearch: "", recordingRulesFingerprint: null, recordingListFingerprint: null, dirty: false, loading: true, ready: false, busy: false, previewing: false, preferenceFingerprint: null, listFingerprint: null, rulesFingerprint: null, leavePromise: null, resolveLeave: null, updateStatus: null, updateStatusLoading: false, updateActionBusy: false, updatePollTimer: null, updatePollRequest: null, updateViewActive: false, updateDeferred: false, updateDisposed: false };
  const meta = {
    general: ["全般", "現在のバックエンド状態と設定ファイルを確認します。"],
    connection: ["接続", "接続上限を変更します。保存時に実バックエンドへ反映します。"],
    "yellow-pages": ["イエローページ", "チャンネル一覧の取得先を管理します。\nSPへの掲載は、配信画面で掲載を選んだ場合に行われます。"],
    player: ["プレイヤー", "視聴時に起動するプレイヤーと引数を設定します。"],
    "channel-rules": ["NG・お気に入り", "チャンネル情報に一致する条件と表示・通知方法を設定します。"],
    recording: ["録画", "保存先、録画の再試行と自動監視条件を設定します。"],
    appearance: ["外観", "画面の表示方法を設定します。"],
    version: ["バージョン情報", "アプリと使用しているライセンスを確認します。"],
  };

  function setNotice(message, error = false) {
    const notice = $("notice");
    notice.textContent = message || "";
    notice.classList.toggle("error", error);
    notice.hidden = !message;
  }
  function setStatus(message) { $("save-status").textContent = message; }
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
    setStatus("未保存の変更があります");
    renderControls();
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
    const fingerprint = JSON.stringify({
      maxDirects: Number(snapshotLimits.maxDirects) || 1,
      maxRelays: Number(snapshotLimits.maxRelays) || 1,
      maxUpstreamRate: Number(snapshotLimits.maxUpstreamRate) || 1,
      yellowPages: snapshotPages,
      player: snapshotPlayer,
      channelRules: snapshotRules,
      recording: snapshotRecording,
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
      state.selectedRule = state.rules.length ? Math.min(state.selectedRule ?? 0, state.rules.length - 1) : null;
      state.recording = { ...snapshotRecording, rules: snapshotRecording.rules.map((rule) => ({ ...rule, fields: [...rule.fields] })) };
      state.savedRecording = cloneRecording(state.recording);
      state.window = { ...snapshotWindow };
      $("window-width").value = String(state.window.width);
      $("window-height").value = String(state.window.height);
      $("window-preset").value = `${state.window.width}x${state.window.height}`;
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
    $("backend-uptime").textContent = snapshot.running ? `${Number(snapshot.uptimeSecs) || 0} 秒` : "—";
    $("backend-tasks").textContent = snapshot.running ? String(Number(snapshot.activeTasks) || 0) : "—";
    $("config-path").textContent = snapshot.configPath || "—";
    const configuredPlayer = typeof snapshot.desktop?.player?.executable === "string" && snapshot.desktop.player.executable.trim() ? snapshot.desktop.player.executable : null;
    const playerPath = configuredPlayer || (typeof snapshot.playerPath === "string" && snapshot.playerPath.trim() ? snapshot.playerPath : "同梱プレイヤー");
    const playerStatus = typeof snapshot.playerAvailable === "boolean"
      ? snapshot.playerAvailable ? "利用可能" : "見つかりません"
      : "状態を確認できません";
    $("player-status").textContent = `${playerPath}（${playerStatus}）`;
    $("player-editor-status").textContent = `保存済み設定: ${playerPath}（${playerStatus}）。`;
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
    const status = state.updateStatus;
    const model = updateCardModel(status);
    const locked = !status || state.updateStatusLoading || state.updateActionBusy || model.busy;
    const currentVersion = $("update-current-version");
    if (!currentVersion) return;
    currentVersion.textContent = status?.currentVersion ? `現在のバージョン ${status.currentVersion}` : "現在のバージョンを確認しています";
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
    stopUpdatePolling();
    if (!state.updateViewActive || state.updateDisposed || document.hidden || !isUpdateBusy()) return;
    state.updatePollTimer = window.setTimeout(pollUpdateStatus, 500);
  }
  async function pollUpdateStatus() {
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
    if (state.updateViewActive) ensureUpdateStatus();
    else stopUpdatePolling();
  }
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
      setNotice("ウィンドウサイズは幅1280〜2560px、高さ720〜1440pxで入力してください。", true);
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
    setStatus("保存しています…");
    try {
      const desktop = state.snapshot?.desktop || {};
      const snapshot = await invoke("apply_preferences", { limits, yellowPages: state.pages, desktop: { ...desktop, directory: desktop.directory || { autoRefresh: false, refreshIntervalSecs: 300 }, window: { ...state.window }, player: state.player, channelRules: state.rules.map((rule) => normalizedRule(rule)), recording: normalizeRecordingSettings(state.recording) } });
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
    if (!state.dirty) return true;
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
    const preset = parseWindowPreset($("window-preset").value);
    if (preset) { $("window-width").value = String(preset.width); $("window-height").value = String(preset.height); }
    state.window.width = Number($("window-width").value);
    state.window.height = Number($("window-height").value);
    setDirty();
  });
  ["window-width", "window-height"].forEach((id) => $(id).addEventListener("input", () => {
    state.window.width = Number($("window-width").value);
    state.window.height = Number($("window-height").value);
    $("window-preset").value = `${$("window-width").value}x${$("window-height").value}`;
    setDirty();
  }));
  ["window-minimize-to-tray", "window-launch-at-startup"].forEach((id) => $(id).addEventListener("change", () => {
    state.window.minimizeToTray = $("window-minimize-to-tray").checked;
    state.window.launchAtStartup = $("window-launch-at-startup").checked;
    setDirty();
  }));
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
    commitRuleEditor(); state.rules.push(newRule()); state.selectedRule = state.rules.length - 1; syncRuleFields(); setDirty(); renderRulesList(); $("rule-name").focus();
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


  renderControls();
  if (initialSnapshot) applySnapshot(initialSnapshot, true);
  else invoke("snapshot").then((snapshot) => {
    applySnapshot(snapshot, true);
  }).catch((error) => {
    state.loading = false;
    state.ready = false;
    setStatus("バックエンドに接続できません");
    setNotice(`設定を読み込めませんでした: ${error.message || error}`, true);
    renderControls();
  });
  return {
    requestLeave, updateSnapshot, isDirty: () => state.dirty,
    addChannelRule(channel, kind = "favorite") {
      const rule = newRule(kind === "ng" ? "ng" : "favorite", channel);
      if (state.rules.length >= 128) { activate("channel-rules"); setNotice("条件は128件まで登録できます。", true); return false; }
      state.rules.push(rule); state.selectedRule = state.rules.length - 1; state.rulesSearch = ""; $("rules-search").value = ""; activate("channel-rules"); syncRuleFields(); setDirty(); renderRulesList(); return true;
    },
  };
  };
})();
