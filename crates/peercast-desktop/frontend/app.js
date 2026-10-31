(() => {
  "use strict";

  const invoke = (command, args) => {
    const call = window.__TAURI__?.core?.invoke;
    return typeof call === "function"
      ? call(command, args)
      : Promise.reject(new Error("Tauri backendに接続できません。アプリ内で開いてください。"));
  };
  const byId = (id) => document.getElementById(id);
  const YP_REFRESH_COOLDOWN_MS = 30_000;
  const YOUTUBE_STATUS_POLL_MS = 5_000;
  const UPDATE_INITIAL_CHECK_DELAY_MS = 30_000;
  const UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
  const UPDATE_LAST_CHECK_KEY = "peercast-rt.update.last-successful-check";
  const state = {
    capabilities: { platform: "unknown", liveServices: false, updater: false },
    snapshot: null,
    noticeTimer: null,
    noticeToken: 0,
    noticeMessage: "",
    noticeIsError: false,
    noticePersistent: false,
    busy: false,
    broadcastBusy: false,
    stoppingBroadcasts: new Set(),
    broadcastFormInitialized: false,
    broadcastFormDirty: false,
    refreshing: false,
    manualRefreshing: false,
    ypChannels: [],
    ypLoaded: false,
    ypBusy: false,
    ypRefreshing: false,
    ypRefreshTimer: null,
    ypRefreshCooldownUntil: 0,
    ypLastUpdatedAt: null,
    ypRefreshMode: "auto",
    ypRefreshIntervalSecs: 300,
    ypDirectoryDirty: false,
    ypRefreshPending: false,
    ypRefreshForcePending: false,
    ypError: "",
    ypStarted: false,
    ypQuery: "",
    ypSource: "peercast",
    ypScope: "normal",
    ypSourceSignature: "",
    ypDirectoryFlagsFingerprint: "",
    ypSelectionKey: null,
    channelSelectionId: null,
    channelFingerprint: "",
    connectionSelectionId: null,
    connectionTab: "connections",
    connectionFingerprint: "",
    detailKind: null,
    detailKey: null,
    detailTab: "info",
    logQuery: "",
    logFingerprint: "",
    clearingLogs: false,
    logClearGeneration: 0,
    clearedConnectionKeys: new Set(),
    connectionClearGeneration: 0,
    settingsView: null,
    settingsMountPromise: null,
    viewTransition: false,
    closingMain: false,
    ypSeenAt: new Map(),
    ypGeneration: null,
    recordingSelectionId: null,
    recordingListFingerprint: "",
    recordingBusy: false,
    pendingRecordingRemoval: null,
    ypContextChannel: null,
    providerContextStream: null,
    youtubeSnapshot: null,
    twitchSnapshot: null,
    youtubeQueryBusy: false,
    twitchQueryBusy: false,
    youtubeRevision: 0,
    twitchRevision: 0,
    youtubeSource: "all",
  };

  function livePlayerName() {
    return state.capabilities.platform === "linux" ? "mpv" : "PCRPlayer";
  }
  function livePlaybackReady() {
    return state.capabilities.liveServices === true &&
      (state.capabilities.platform !== "linux" || state.capabilities.liveDependencies?.playbackReady === true);
  }
  async function launchLivePlayback(command, args) {
    if (!livePlaybackReady()) return;
    const key = `${command}:${args.videoId || args.streamId}`;
    const pending = state.pendingLivePlayback ||= new Set();
    if (pending.has(key)) return;
    pending.add(key);
    try {
      const generation = await invoke(command, args);
      if (state.capabilities?.platform === "linux") watchLivePlayback(generation);
    } finally { pending.delete(key); }
  }
  function stopLivePlaybackWatch() {
    state.livePlaybackWatchToken = (state.livePlaybackWatchToken || 0) + 1;
    if (state.livePlaybackWatchTimer != null) window.clearTimeout(state.livePlaybackWatchTimer);
    state.livePlaybackWatchTimer = null;
  }
  function watchLivePlayback(generation) {
    if (state.capabilities?.platform !== "linux" || state.livePlaybackWatchDisposed ||
        !Number.isSafeInteger(generation) || generation <= 0 || generation <= (state.livePlaybackGeneration || 0)) return;
    stopLivePlaybackWatch();
    state.livePlaybackGeneration = generation;
    const token = state.livePlaybackWatchToken;
    // No startup-wide polling. Only the latest Linux launch owns this timer.
    const poll = async () => {
      if (token !== state.livePlaybackWatchToken) return;
      state.livePlaybackWatchTimer = null;
      try {
        const status = await invoke("live_playback_status", {});
        if (token !== state.livePlaybackWatchToken) return;
        if (!status || status.generation !== generation) {
          stopLivePlaybackWatch(); // A newer backend launch superseded this one.
          return;
        }
        if (status.state === "failed") {
          stopLivePlaybackWatch();
          setNotice(status.message || "mpv の再生に失敗しました。依存関係と公開ライブの状態を確認してください。", true);
          return;
        }
        if (status.state === "exited" || status.state === "idle") {
          stopLivePlaybackWatch();
          return;
        }
        if (status.state !== "starting" && status.state !== "running") {
          stopLivePlaybackWatch();
          return;
        }
      } catch (_) {
        if (token !== state.livePlaybackWatchToken) return;
        stopLivePlaybackWatch();
        setNotice("mpv の終了状態を確認できませんでした。再生状態はプレイヤーで確認してください。", true);
        return;
      }
      if (token === state.livePlaybackWatchToken) state.livePlaybackWatchTimer = window.setTimeout(poll, 1000);
    };
    state.livePlaybackWatchTimer = window.setTimeout(poll, 1000);
  }
  window.addEventListener("pagehide", () => {
    state.livePlaybackWatchDisposed = true;
    stopLivePlaybackWatch();
  });
  function applyLiveDependencies(dependencies) {
    if (!dependencies) return;
    const changed = JSON.stringify(state.capabilities.liveDependencies) !== JSON.stringify(dependencies);
    state.capabilities.liveDependencies = dependencies;
    const note = byId("live-dependencies-status");
    if (note) {
      note.hidden = dependencies.reasonCode === "ready";
      note.textContent = dependencies.message;
    }
    text("provider-context-play", `${livePlayerName()}で再生`);
    if (changed) {
      window.dispatchEvent(new CustomEvent("live-dependencies-updated", { detail: dependencies }));
      if (currentView() === "yp") renderYpDirectory();
    }
  }
  async function fetchLiveDependencies() {
    if (state.liveDependenciesBusy) return;
    state.liveDependenciesBusy = true;
    try {
      const capabilities = await invoke("platform_capabilities");
      applyLiveDependencies(capabilities?.liveDependencies);
    } catch (_) { /* Keep the last known state; backend validates every launch. */ }
    finally { state.liveDependenciesBusy = false; }
  }
  window.addEventListener("live-dependencies-rechecked", (event) => applyLiveDependencies(event.detail));

  const viewTitles = { yp: "チャンネル", channels: "配信", connections: "接続", recordings: "録画", logs: "ログ", settings: "設定" };
  const internalTestId = "0123456789abcdef0123456789abcdef";
  const logRanks = { error: 0, fatal: 0, warn: 1, warning: 1, info: 2, debug: 3, trace: 4 };

  function text(id, value) {
    const element = byId(id);
    if (element) element.textContent = value == null ? "" : String(value);
  }

  function setNotice(message, isError = false, { persistent = false } = {}) {
    const element = byId("notice");
    if (!element) return;
    const nextMessage = message == null ? "" : String(message);
    if (nextMessage && nextMessage === state.noticeMessage && isError === state.noticeIsError && persistent === state.noticePersistent) return;
    if (state.noticeTimer !== null) window.clearTimeout(state.noticeTimer);
    state.noticeTimer = null;
    const token = ++state.noticeToken;
    state.noticeMessage = nextMessage;
    state.noticeIsError = Boolean(isError);
    state.noticePersistent = Boolean(persistent);
    element.textContent = nextMessage;
    element.hidden = !nextMessage;
    element.classList.toggle("notice-error", isError);
    if (nextMessage && !persistent) {
      state.noticeTimer = window.setTimeout(() => {
        if (state.noticeToken !== token) return;
        state.noticeTimer = null;
        state.noticeMessage = "";
        state.noticeIsError = false;
        state.noticePersistent = false;
        element.textContent = "";
        element.hidden = true;
        element.classList.toggle("notice-error", false);
      }, isError ? 12000 : 5000);
    }
  }

  function errorText(error) {
    return String(error && error.message ? error.message : error);
  }

  async function checkForApplicationUpdate() {
    if (state.capabilities?.updater === false) return;
    try {
      const status = await invoke("check_update");
      try { localStorage.setItem(UPDATE_LAST_CHECK_KEY, String(Date.now())); } catch { /* Updating still works without WebView storage. */ }
      if (status?.phase === "available") {
        setNotice(`バージョン ${status.version || "最新版"} を利用できます。設定の「バージョン情報」から更新できます。`);
      }
    } catch {
      // Startup checks are best-effort. Manual checks in Settings show actionable errors.
    }
  }

  function scheduleApplicationUpdateChecks() {
    if (state.capabilities?.updater === false) return;
    let lastSuccessfulCheck = 0;
    try { lastSuccessfulCheck = Number(localStorage.getItem(UPDATE_LAST_CHECK_KEY) || 0); } catch { /* Ignore unavailable storage. */ }
    const elapsed = Number.isFinite(lastSuccessfulCheck) ? Date.now() - lastSuccessfulCheck : UPDATE_CHECK_INTERVAL_MS;
    const delay = lastSuccessfulCheck > 0 && elapsed < UPDATE_CHECK_INTERVAL_MS
      ? UPDATE_CHECK_INTERVAL_MS - elapsed
      : UPDATE_INITIAL_CHECK_DELAY_MS;
    window.setTimeout(() => {
      void checkForApplicationUpdate();
      window.setInterval(() => { void checkForApplicationUpdate(); }, UPDATE_CHECK_INTERVAL_MS);
    }, delay);
  }

  function setBusy(busy) {
    state.busy = busy;
    ["probe-button", "refresh-button", "restart-button", "stop-button"].forEach((id) => {
      const button = byId(id);
      if (button) button.disabled = busy;
    });
    updateYpControls();
    updateRecordingControls();
  }

  function updateRefreshControls() {
    const ypCooldownSecs = Math.ceil(Math.max(0, state.ypRefreshCooldownUntil - performance.now()) / 1000);
    [["yp-refresh", state.ypRefreshing, state.busy || state.ypBusy || ypCooldownSecs > 0],
      ["refresh-button", state.manualRefreshing, state.busy || state.manualRefreshing]].forEach(([id, refreshing, disabled]) => {
      const button = byId(id);
      if (!button) return;
      if (button.disabled !== disabled) button.disabled = disabled;
      button.classList.toggle("is-refreshing", refreshing);
      button.setAttribute("aria-busy", String(refreshing));
      const label = button.querySelector(".refresh-label");
      const remaining = id === "yp-refresh" ? ypCooldownSecs : 0;
      const nextLabel = refreshing ? "更新中…" : remaining > 0 ? `更新（${remaining}秒後）` : "更新";
      if (label && label.textContent !== nextLabel) label.textContent = nextLabel;
      if (id === "yp-refresh") button.title = remaining > 0 ? `サーバー負荷を抑えるため、あと${remaining}秒待ってから更新できます` : "";
    });
  }

  async function finishRefreshFeedback(startedAt) {
    const remaining = 400 - (performance.now() - startedAt);
    if (remaining > 0) await new Promise((resolve) => window.setTimeout(resolve, remaining));
  }

  function formatBytes(value) {
    const bytes = Number(value);
    if (!Number.isFinite(bytes) || bytes < 0) return "—";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KiB`;
    if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
    return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
  }

  function formatDuration(seconds) {
    const value = Number(seconds);
    if (!Number.isFinite(value) || value < 0) return "—";
    const total = Math.floor(value);
    const days = Math.floor(total / 86400);
    const hours = Math.floor((total % 86400) / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const secs = total % 60;
    return days ? `${days}日 ${hours}時間` : hours ? `${hours}時間 ${minutes}分` : minutes ? `${minutes}分 ${secs}秒` : `${secs}秒`;
  }

  function formatAppUptime(seconds) {
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

  function displayValue(value) {
    if (value === null || value === undefined || value === "") return "—";
    return String(value);
  }

  function isDirectFavoriteRule(rule, channelName) {
    const name = String(channelName || "").trim().normalize("NFKC");
    if (!name || !rule || !String(rule.id || "").startsWith("quick-") || rule.enabled === false
      || rule.name !== Array.from(name).slice(0, 80).join("")
      || rule.fields?.length !== 1 || rule.fields[0] !== "name"
      || rule.useRegex !== true || rule.normalizeWidth !== true || rule.matchCase !== false
      || rule.favorite !== true || rule.hidden !== false || rule.mute !== false
      || rule.textColor != null || rule.backgroundColor != null || rule.soundPath != null) return false;
    const pattern = String(rule.pattern || "");
    if (!pattern.startsWith("^") || !pattern.endsWith("$")) return false;
    let literal = "";
    for (let index = 1; index < pattern.length - 1; index += 1) {
      const character = pattern[index];
      if (character === "\\") {
        const escaped = pattern[index + 1];
        if (!escaped || /[a-z0-9]/i.test(escaped)) return false;
        literal += escaped;
        index += 1;
      } else {
        if (/[.*+?()[\]{}|^$]/.test(character)) return false;
        literal += character;
      }
    }
    return literal === name;
  }

  function directFavoriteRuleIndex(rules, channelName) {
    return Array.isArray(rules) ? rules.findIndex((rule) => isDirectFavoriteRule(rule, channelName)) : -1;
  }

  function removeDirectFavoriteRule(rules, channelName) {
    if (!Array.isArray(rules)) return null;
    const remaining = rules.filter((rule) => !isDirectFavoriteRule(rule, channelName));
    return remaining.length === rules.length ? null : remaining;
  }

  function counter(value) {
    const number = ypViewerCount({ listeners: value });
    return number === null ? "—" : String(number);
  }

  function isCountableYpChannel(channel) {
    const channelId = String(channel?.channelId || "");
    const tracker = String(channel?.tracker || "").trim();
    const bitrate = Number(channel?.bitrate);
    const hasBitrate = channel?.bitrate !== null && channel?.bitrate !== undefined && String(channel.bitrate).trim() !== "";
    return /^[0-9a-f]{32}$/i.test(channelId)
      && !/^0{32}$/i.test(channelId)
      && Boolean(tracker)
      && (!hasBitrate || !Number.isFinite(bitrate) || bitrate > 0);
  }
  function isPlayableYpChannel(channel) {
    return isCountableYpChannel(channel);
  }
  function countableYpChannelCount(channels) {
    return channels.filter(isCountableYpChannel).length;
  }
  function isBroadcastingChannel(channel) {
    return channel?.isBroadcasting === true;
  }
  function channelAnnotation(channel) { return channel?.annotation && typeof channel.annotation === "object" ? channel.annotation : channel || {}; }

  function ypViewerCount(channel) {
    const value = channel?.listeners;
    if (typeof value === "number") return Number.isFinite(value) && value >= 0 ? value : null;
    if (typeof value === "string" && value.trim() !== "") {
      const parsed = Number(value);
      return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
    }
    return null;
  }

  function orderYpChannels(channels) {
    return channels
      .map((channel, index) => ({
        channel,
        index,
        favorite: Boolean(channelAnnotation(channel).isFavorite),
        playable: isPlayableYpChannel(channel),
        viewers: ypViewerCount(channel),
      }))
      .sort((a, b) => {
        if (a.favorite !== b.favorite) return a.favorite ? -1 : 1;
        if (a.favorite) {
          if (a.viewers !== null && b.viewers !== null) return b.viewers - a.viewers || a.index - b.index;
          if (a.viewers !== null) return -1;
          if (b.viewers !== null) return 1;
          return a.index - b.index;
        }
        if (a.playable !== b.playable) return a.playable ? -1 : 1;
        if (!a.playable) return a.index - b.index;
        if (a.viewers !== null && b.viewers !== null) return b.viewers - a.viewers || a.index - b.index;
        if (a.viewers !== null) return -1;
        if (b.viewers !== null) return 1;
        return a.index - b.index;
      })
      .map(({ channel }) => channel);
  }

  function ypChannelKey(channel) {
    return `${String(channel.channelId || "").toLowerCase()}\u0000${String(channel.yellowPage || "")}`;
  }

  function advanceYpGeneration(previous, channels, succeeded = true) {
    if (!succeeded) return previous;
    const knownKeys = new Set(channels.map(ypChannelKey));
    if (!previous?.initialized) {
      return { initialized: true, knownKeys, currentNewKeys: new Set(), previousNewKeys: new Set() };
    }
    const currentNewKeys = new Set([...knownKeys].filter((key) => !previous.knownKeys.has(key)));
    return {
      initialized: true,
      knownKeys,
      currentNewKeys,
      previousNewKeys: new Set(previous.currentNewKeys),
    };
  }

  function ypNewcomerIndicator(key, generation) {
    if (generation?.currentNewKeys?.has(key)) {
      return { src: "assets/bulb-new.svg", label: "今回の更新で追加" };
    }
    if (generation?.previousNewKeys?.has(key)) {
      return { src: "assets/bulb-previous.svg", label: "前回の更新で追加" };
    }
    return null;
  }

  function currentYpChannel() {
    if (!state.ypSelectionKey) return null;
    return state.ypChannels.find((channel) => ypChannelKey(channel) === state.ypSelectionKey) || null;
  }

  function currentView() {
    return document.querySelector("[data-view].active")?.dataset.view || "yp";
  }

  function directoryChannelFor(id) {
    if (!id) return null;
    const normalized = String(id).toLowerCase();
    const selected = currentYpChannel();
    if (selected && String(selected.channelId || "").toLowerCase() === normalized) return selected;
    return state.ypChannels.find((channel) => String(channel.channelId || "").toLowerCase() === normalized) || null;
  }

  function currentSelectedDirectoryChannel() {
    if (state.detailKind === "yp") return currentYpChannel();
    if (state.detailKind === "channel") return directoryChannelFor(state.channelSelectionId);
    return null;
  }
  function ypContextChannelFor(target, channels) {
    if (!target?.channelId || !Array.isArray(channels)) return null;
    return channels.find((channel) => String(channel.channelId) === String(target.channelId)
      && String(channel.yellowPage || "") === String(target.yellowPage || "")) || null;
  }
  function currentYpContextChannel() {
    return ypContextChannelFor(state.ypContextChannel, state.ypChannels);
  }

  function sourcePages() {
    return Array.isArray(state.snapshot?.yellowPages) ? state.snapshot.yellowPages : [];
  }

  function updateYpSources() {
    const select = byId("yp-source");
    if (!select) return false;
    const pages = sourcePages();
    const signature = JSON.stringify(pages.map((page) => [page.name || "", page.channelsUrl || ""]));
    if (signature === state.ypSourceSignature) return false;
    const previous = select.value || state.ypSource;
    state.ypSourceSignature = signature;
    select.replaceChildren();
    for (const [value, label] of [["peercast", "Peercast"], ["all", "すべてのチャンネル"], ["youtube", "YouTube"], ["twitch", "Twitch"]]) {
      const option = document.createElement("option");
      if (state.capabilities?.liveServices === false && ["youtube", "twitch"].includes(value)) continue;
      option.value = value;
      option.textContent = label;
      select.append(option);
    }
    const values = new Set(state.capabilities?.liveServices === false ? ["peercast", "all"] : ["peercast", "all", "youtube", "twitch"]);
    state.ypSource = values.has(previous) ? previous : "peercast";
    select.value = state.ypSource;
    return true;
  }

  function visibleYpChannels() {
    const query = state.ypQuery.normalize("NFKC").toLocaleLowerCase("ja-JP").trim();
    return orderYpChannels(state.ypChannels).filter((channel) => {
      const annotation = channelAnnotation(channel);
      if (state.ypScope === "normal" && annotation.isHidden) return false;
      if (state.ypScope === "favorite" && (!annotation.isFavorite || annotation.isHidden)) return false;
      if (state.ypSource === "youtube" || state.ypSource === "twitch") return false;
      if (!query) return true;
      return ["name", "genre", "description", "comment"].some((field) =>
        String(channel[field] ?? "").normalize("NFKC").toLocaleLowerCase("ja-JP").includes(query));
    });
  }

  function directoryProviderAtBottom(provider) {
    return state.snapshot?.desktop?.directory?.[`${provider}AtBottom`] === true;
  }

  function allSourcesDisplayOrder(youtubeAtBottom, twitchAtBottom) {
    return [
      ...(!youtubeAtBottom ? ["youtube"] : []),
      ...(!twitchAtBottom ? ["twitch"] : []),
      "yp-playable",
      ...(youtubeAtBottom ? ["youtube"] : []),
      ...(twitchAtBottom ? ["twitch"] : []),
      "yp-notice",
    ];
  }

  function youtubeStateLabel(channel, stream) {
    if (channel.error) return "確認できず";
    if (channel.enabled !== true) return "監視無効";
    if (stream?.status === "live") return "配信中";
    if (stream?.status === "upcoming") return "予約";
    if (channel.lastCheckedMs == null) return "確認待ち";
    return "配信なし";
  }

  function ensureVisibleYpSelection(channels) {
    if (!channels.some((channel) => ypChannelKey(channel) === state.ypSelectionKey)) {
      state.ypSelectionKey = channels.length ? ypChannelKey(channels[0]) : null;
    }
    if (currentView() === "yp" && (state.detailKind === "yp" || !state.detailKind)) {
      state.detailKind = state.ypSelectionKey ? "yp" : null;
      state.detailKey = state.ypSelectionKey;
    }
  }

  function appendText(parent, tag, className, value) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    element.textContent = value;
    parent.append(element);
    return element;
  }

  function youtubeChannels() {
    return Array.isArray(state.youtubeSnapshot?.channels) ? state.youtubeSnapshot.channels : [];
  }

  function twitchChannels() {
    return Array.isArray(state.twitchSnapshot?.channels) ? state.twitchSnapshot.channels : [];
  }

  function youtubeSnapshotsEqual(left, right) {
    if (left === right) return true;
    if (left === null || right === null || typeof left !== "object" || typeof right !== "object") return false;
    if (Array.isArray(left) || Array.isArray(right)) {
      return Array.isArray(left) && Array.isArray(right) && left.length === right.length
        && left.every((value, index) => youtubeSnapshotsEqual(value, right[index]));
    }
    const leftKeys = Object.keys(left).sort();
    const rightKeys = Object.keys(right).sort();
    return leftKeys.length === rightKeys.length && leftKeys.every((key, index) =>
      key === rightKeys[index] && youtubeSnapshotsEqual(left[key], right[key]));
  }

  function youtubeStreamRows() {
    const query = state.ypQuery.normalize("NFKC").toLocaleLowerCase("ja-JP").trim();
    return youtubeChannels().flatMap((channel) => {
      const streams = channel.enabled === true && !channel.error && Array.isArray(channel.streams) ? channel.streams : [];
      const entries = streams.filter((stream) => stream && stream.status === "live").map((stream) => ({ channel, stream }));
      return entries.filter(({ channel: owner, stream }) => !query || [owner.name, stream?.title].some((value) =>
        String(value || "").normalize("NFKC").toLocaleLowerCase("ja-JP").includes(query)));
    });
  }

  function appendYoutubeRows(root) {
    const source = state.ypSource;
    if (source !== "all" && source !== "youtube") return 0;
    const entries = youtubeStreamRows();
    entries.forEach(({ channel, stream }) => {
      const card = document.createElement("article");
      card.className = `youtube-stream-card${stream?.status === "live" ? " is-live" : ""}`;
      card.dataset.provider = "youtube";
      card.dataset.streamId = String(stream?.videoId || stream?.streamId || stream?.id || "");
      const badge = appendText(card, "span", "youtube-badge", "YouTube");
      badge.setAttribute("aria-label", "YouTube配信");
      const body = document.createElement("div");
      body.className = "youtube-stream-copy";
      appendText(body, "strong", "youtube-stream-channel", channel.name || "登録チャンネル");
      if (stream) {
        appendText(body, "span", "youtube-stream-title", stream.title || "タイトル未設定");
        if (stream.status === "upcoming" && stream.scheduledAtMs != null) {
          const date = new Date(Number(stream.scheduledAtMs));
          appendText(body, "small", "youtube-stream-schedule", Number.isNaN(date.getTime()) ? "配信予定" : `配信予定 ${date.toLocaleString("ja-JP")}`);
        }
      } else {
        const stateLabel = channel.enabled !== true ? "監視は無効です" : channel.error ? "確認できませんでした" : channel.lastCheckedMs == null ? "まもなく配信状況を確認します" : "配信はありません";
        appendText(body, "span", `youtube-stream-state${channel.error ? " has-error" : ""}`, stateLabel);
      }
      card.append(body);
      const stateLabel = youtubeStateLabel(channel, stream);
      appendText(card, "span", `youtube-stream-status${stream?.status === "live" ? " status-live" : ""}`, stateLabel);
      if (stream?.status === "live" && stream.id) {
        const actions = document.createElement("div");
        actions.className = "youtube-stream-actions";
        const play = appendText(actions, "button", "button button-outline youtube-open", `${livePlayerName()}で再生`);
        play.type = "button";
        play.disabled = !livePlaybackReady();
        play.title = play.disabled ? state.capabilities.liveDependencies?.message || "再生の準備ができていません" : "";
        play.dataset.youtubePlay = String(stream.id);
        play.setAttribute("aria-label", `${stream.title || "配信"}を${livePlayerName()}で再生`);
        const browser = appendText(actions, "button", "button button-outline youtube-open", "ブラウザで開く");
        browser.type = "button";
        browser.dataset.youtubeBrowser = String(stream.id);
        browser.setAttribute("aria-label", `${stream.title || "配信"}をブラウザで開く`);
        card.append(actions);
      }
      root.append(card);
    });
    return entries.length;
  }

  function twitchStreamRows() {
    const query = state.ypQuery.normalize("NFKC").toLocaleLowerCase("ja-JP").trim();
    return twitchChannels().flatMap((channel) => {
      if (channel.enabled !== true || channel.error || !Array.isArray(channel.streams)) return [];
      return channel.streams.filter((stream) => stream && stream.status === "live" && (stream.streamId || stream.id))
        .filter((stream) => !query || [channel.name, stream.title].some((value) =>
          String(value || "").normalize("NFKC").toLocaleLowerCase("ja-JP").includes(query)))
        .map((stream) => ({ channel, stream }));
    });
  }

  function appendTwitchRows(root) {
    if (state.ypSource !== "all" && state.ypSource !== "twitch") return 0;
    const entries = twitchStreamRows();
    entries.forEach(({ channel, stream }) => {
      const streamId = String(stream.streamId || stream.id);
      const card = document.createElement("article");
      card.className = "youtube-stream-card twitch-stream-card is-live";
      card.dataset.provider = "twitch";
      card.dataset.streamId = streamId;
      const badge = appendText(card, "span", "youtube-badge twitch-badge", "Twitch");
      badge.setAttribute("aria-label", "Twitch配信");
      const body = document.createElement("div");
      body.className = "youtube-stream-copy";
      appendText(body, "strong", "youtube-stream-channel", channel.name || channel.url || "登録チャンネル");
      appendText(body, "span", "youtube-stream-title", stream.title || "タイトル未設定");
      card.append(body);
      appendText(card, "span", "youtube-stream-status status-live", "配信中");
      const actions = document.createElement("div");
      actions.className = "youtube-stream-actions";
      const play = appendText(actions, "button", "button button-outline youtube-open", `${livePlayerName()}で再生`);
      play.type = "button";
      play.disabled = !livePlaybackReady();
      play.title = play.disabled ? state.capabilities.liveDependencies?.message || "再生の準備ができていません" : "";
      play.dataset.twitchPlay = streamId;
      play.setAttribute("aria-label", `${stream.title || channel.name || "配信"}を${livePlayerName()}で再生`);
      const browser = appendText(actions, "button", "button button-outline youtube-open", "ブラウザで開く");
      browser.type = "button";
      browser.dataset.twitchBrowser = streamId;
      browser.setAttribute("aria-label", `${stream.title || channel.name || "配信"}をブラウザで開く`);
      card.append(actions);
      root.append(card);
    });
    return entries.length;
  }

  async function fetchYoutubeStatus() {
    if (state.capabilities?.liveServices === false) return;
    if (state.youtubeQueryBusy || document.visibilityState === "hidden") return;
    state.youtubeQueryBusy = true;
    const revision = state.youtubeRevision;
    try {
      const snapshot = await invoke("youtube_status", {});
      if (revision !== state.youtubeRevision) return;
      if (snapshot && Array.isArray(snapshot.channels)) {
        const changed = !youtubeSnapshotsEqual(state.youtubeSnapshot, snapshot);
        state.youtubeSnapshot = snapshot;
        window.dispatchEvent(new CustomEvent("youtube-snapshot-polled", { detail: snapshot }));
        if (changed && currentView() === "yp") renderYpDirectory();
      }
    } catch (error) {
      console.warn("YouTubeの状態を確認できませんでした:", errorText(error));
      window.dispatchEvent(new CustomEvent("youtube-snapshot-error", { detail: errorText(error) }));
    } finally {
      state.youtubeQueryBusy = false;
    }
  }

  async function fetchTwitchStatus() {
    if (state.capabilities?.liveServices === false) return;
    if (state.twitchQueryBusy || document.visibilityState === "hidden") return;
    state.twitchQueryBusy = true;
    const revision = state.twitchRevision;
    try {
      const snapshot = await invoke("twitch_status", {});
      if (revision !== state.twitchRevision) return;
      if (snapshot && Array.isArray(snapshot.channels)) {
        const changed = !youtubeSnapshotsEqual(state.twitchSnapshot, snapshot);
        state.twitchSnapshot = snapshot;
        window.dispatchEvent(new CustomEvent("twitch-snapshot-polled", { detail: snapshot }));
        if (changed && currentView() === "yp") renderYpDirectory();
      }
    } catch (error) {
      console.warn("Twitchの状態を確認できませんでした:", errorText(error));
      window.dispatchEvent(new CustomEvent("twitch-snapshot-error", { detail: errorText(error) }));
    } finally {
      state.twitchQueryBusy = false;
    }
  }

  function renderYpDirectory() {
    const root = byId("yp-results");
    if (!root) return;
    const filtered = visibleYpChannels();
    ensureVisibleYpSelection(filtered);
    root.replaceChildren();
    const peercastOnly = state.ypSource === "peercast";
    const allSources = state.ypSource === "all";
    const youtubeAtBottom = directoryProviderAtBottom("youtube");
    const twitchAtBottom = directoryProviderAtBottom("twitch");
    let youtubeCount = 0;
    let twitchCount = 0;
    if (state.ypSource === "youtube") youtubeCount = appendYoutubeRows(root);
    else if (state.ypSource === "twitch") twitchCount = appendTwitchRows(root);
    const youtubeLiveCount = youtubeStreamRows().filter(({ stream }) => stream?.status === "live").length;
    const twitchLiveCount = twitchStreamRows().length;
    renderYpHeaderCount(filtered, youtubeLiveCount, twitchLiveCount);
    const playableChannels = filtered.filter(isPlayableYpChannel);
    const noticeChannels = filtered.filter((channel) => !isPlayableYpChannel(channel));
    if (peercastOnly) {
      appendYpChannelRows(root, filtered);
    } else if (allSources) {
      const displayOrder = allSourcesDisplayOrder(youtubeAtBottom, twitchAtBottom);
      for (const entry of displayOrder) {
        if (entry === "youtube") youtubeCount += appendYoutubeRows(root);
        else if (entry === "twitch") twitchCount += appendTwitchRows(root);
        else if (entry === "yp-playable") appendYpChannelRows(root, playableChannels);
        else if (entry === "yp-notice") appendYpChannelRows(root, noticeChannels);
      }
    }
    const empty = byId("yp-empty");
    if (empty) {
      empty.hidden = filtered.length > 0 || youtubeCount > 0 || twitchCount > 0;
      empty.textContent = state.ypSource === "twitch" && !twitchChannels().length
        ? "設定のイエローページでTwitchチャンネルを登録すると、配信中のカードがここに表示されます。"
        : state.ypSource === "youtube" && !youtubeChannels().length
          ? "設定のイエローページでYouTubeチャンネルを登録すると、配信中のカードがここに表示されます。"
        : state.ypError
          ? "一覧を取得できませんでした。更新して再試行してください。"
          : state.ypLoaded && !state.ypChannels.length && !sourcePages().length
          ? "設定画面でチャンネル一覧を取得するYellow Pageを登録してください。"
          : state.ypLoaded
            ? state.ypSource === "youtube"
              ? "現在ライブ中の配信はありません。"
              : "条件に一致する掲載情報はありません。"
            : "一覧を取得しています…";
    }
    const displayedCount = countableYpChannelCount(filtered) + youtubeCount + twitchCount;
    const externalOnly = state.ypSource === "youtube" || state.ypSource === "twitch";
    text("yp-status", externalOnly ? `${displayedCount}件を表示中` : state.ypBusy ? "チャンネル一覧を取得しています…" : state.ypError ? state.ypError : state.ypLoaded ? `${displayedCount}件を表示中` : "一覧をまだ取得していません。");
    renderYpSelectionStyles();
    renderSelectedDetail();
    updateYpControls();
  }

  function appendYpChannelRows(root, channels) {
    channels.forEach((channel) => {
      const index = state.ypChannels.indexOf(channel);
      const key = ypChannelKey(channel);
      const row = document.createElement("article");
      row.className = "yp-row";
      row.dataset.ypIndex = String(index);
      row.classList.toggle("selected", key === state.ypSelectionKey);
      if (!isPlayableYpChannel(channel)) row.classList.add("notice-row");
      const annotation = channelAnnotation(channel);
      row.classList.toggle("is-favorite", Boolean(annotation.isFavorite));
      row.classList.toggle("is-ng", Boolean(annotation.isHidden));
      const textColor = safeRuleColor(annotation.ruleTextColor);
      const backgroundColor = safeRuleColor(annotation.ruleBackgroundColor);
      row.classList.toggle("has-rule-style", Boolean(textColor || backgroundColor));
      if (textColor) row.style.setProperty("--rule-text-color", textColor);
      if (backgroundColor) row.style.setProperty("--rule-background-color", backgroundColor);
      const play = document.createElement("button");
      play.type = "button";
      play.className = "yp-play-button";
      play.dataset.ypIndex = String(index);
      play.disabled = state.busy || state.ypBusy || !state.snapshot?.running || !isPlayableYpChannel(channel) || state.snapshot?.playerAvailable === false;
      play.setAttribute("aria-label", `${displayValue(channel.name)} をプレイヤーで開く`);
      const icon = document.createElement("img");
      icon.src = "assets/tabler/player-play.svg";
      icon.alt = "";
      play.append(icon);
      row.append(play);

      const copy = document.createElement("div");
      copy.className = "yp-row-copy";
      const titleLine = document.createElement("div");
      titleLine.className = "yp-row-title-line";
      const titleButton = document.createElement("button");
      titleButton.type = "button";
      titleButton.className = "yp-row-select";
      titleButton.dataset.ypIndex = String(index);
      titleButton.textContent = displayValue(channel.name);
      titleButton.setAttribute("aria-label", `${displayValue(channel.name)}${annotation.isFavorite ? "、お気に入り" : ""}${annotation.isHidden ? "、NG条件に一致" : ""}`);
      if (annotation.isFavorite) {
        const star = document.createElement("span"); star.className = "icon icon-star yp-favorite-star"; star.setAttribute("aria-label", "お気に入り"); titleButton.append(star);
      }
      if (annotation.isHidden && state.ypScope === "all") appendText(titleButton, "span", "yp-ng-badge", "NG");
      titleLine.append(titleButton);
      const newcomer = ypNewcomerIndicator(key, state.ypGeneration);
      if (newcomer) {
        const indicator = document.createElement("img");
        indicator.className = "yp-row-newcomer";
        indicator.src = newcomer.src;
        indicator.alt = newcomer.label;
        indicator.title = newcomer.label;
        indicator.setAttribute("aria-label", newcomer.label);
        titleLine.append(indicator);
      }
      copy.append(titleLine);
      const summary = [channel.genre, channel.description].filter(Boolean).map(String).join(" · ");
      if (summary) appendText(copy, "p", "", summary);
      if (channel.comment) appendText(copy, "p", "", String(channel.comment));
      row.append(copy);
      const audience = appendText(row, "div", "yp-row-audience", `${counter(channel.listeners)} / ${counter(channel.relays)}`);
      audience.setAttribute("aria-label", `視聴 ${counter(channel.listeners)}、リレー ${counter(channel.relays)}`);
      const rate = counter(channel.bitrate);
      const format = document.createElement("div");
      format.className = "yp-row-format";
      appendText(format, "span", "", displayValue(channel.contentType));
      appendText(format, "span", "", rate === "—" ? "—" : `${rate} kbps`);
      appendText(format, "span", "", formatDuration(channel.uptime));
      format.setAttribute("aria-label", "形式、ビットレート、配信時間");
      row.append(format);
      root.append(row);
    });
  }
  function ypDirectoryHeaderCount(source, ypCount, youtubeLiveCount, twitchLiveCount) {
    const visibleYpCount = source === "youtube" || source === "twitch" ? 0 : ypCount;
    const visibleYoutubeCount = source === "all" || source === "youtube" ? youtubeLiveCount : 0;
    const visibleTwitchCount = source === "all" || source === "twitch" ? twitchLiveCount : 0;
    return visibleYpCount + visibleYoutubeCount + visibleTwitchCount;
  }
  function renderYpHeaderCount(filtered, youtubeLiveCount = 0, twitchLiveCount = 0) {
    const channelCount = ypDirectoryHeaderCount(state.ypSource, countableYpChannelCount(filtered), youtubeLiveCount, twitchLiveCount);
    text("yp-header-count", `${channelCount} 件の配信`);
  }
  function safeRuleColor(value) { return typeof value === "string" && /^#[0-9a-fA-F]{6}$/.test(value) ? value : ""; }

  function renderYpSelectionStyles() {
    const root = byId("yp-results");
    if (!root) return;
    root.querySelectorAll(".yp-row").forEach((row) => {
      const index = Number(row.dataset.ypIndex);
      const channel = state.ypChannels[index];
      row.classList.toggle("selected", Boolean(channel && ypChannelKey(channel) === state.ypSelectionKey));
    });
  }

  function updateYpControls() {
    syncYpFooterCountVisibility();
    updateRefreshControls();
    const source = byId("yp-source");
    if (source) source.disabled = state.busy || state.ypBusy;
    const rowButtons = byId("yp-results")?.querySelectorAll(".yp-play-button") || [];
    rowButtons.forEach((button) => {
      const channel = state.ypChannels[Number(button.dataset.ypIndex)];
      button.disabled = state.busy || state.ypBusy || !state.snapshot?.running || !channel || !isPlayableYpChannel(channel) || state.snapshot?.playerAvailable === false;
    });
    updateRecordingControls();
  }
  function syncYpFooterCountVisibility() {
    const hidden = currentView() === "yp";
    const count = byId("yp-count");
    const divider = byId("status-count-divider");
    if (count) count.hidden = hidden;
    if (divider) divider.hidden = hidden;
  }

  function selectYpChannel(channel, redraw = false) {
    state.ypSelectionKey = ypChannelKey(channel);
    state.detailKind = "yp";
    state.detailKey = state.ypSelectionKey;
    state.channelSelectionId = null;
    if (redraw) renderYpDirectory();
    else {
      renderYpSelectionStyles();
      renderSelectedDetail();
      updateYpControls();
    }
  }

  async function fetchYpChannels(refresh = false, manual = false) {
    if (!sourcePages().length) {
      state.ypLoaded = true;
      state.ypChannels = [];
      state.ypError = "";
      renderYpDirectory();
      return;
    }
    if (manual && (state.busy || state.ypBusy || state.ypRefreshCooldownUntil > performance.now())) {
      updateRefreshControls();
      return;
    }
    if (!beginYpFetch(state, refresh)) return;
    state.ypRefreshing = true;
    const previousSelection = state.ypSelectionKey;
    state.ypError = "";
    text("yp-status", "チャンネル一覧を取得しています…");
    updateYpControls();
    try {
      const channels = await invoke("get_yp_channels", { refresh: Boolean(refresh), notifyFavorites: Boolean(refresh || !state.ypLoaded) });
      if (!Array.isArray(channels)) throw new Error("一覧の応答形式が正しくありません。");
      state.ypGeneration = advanceYpGeneration(state.ypGeneration, channels);
      state.ypChannels = channels;
      state.ypLoaded = true;
      state.ypLastUpdatedAt = Date.now();
      state.ypSelectionKey = previousSelection;
      if (!state.ypRefreshPending) notifyNewYpChannels(channels);
    } catch (error) {
      state.ypError = errorText(error);
      state.ypLoaded = true;
    } finally {
      state.ypRefreshing = false;
      state.ypBusy = false;
      state.ypRefreshCooldownUntil = performance.now() + YP_REFRESH_COOLDOWN_MS;
      renderYpDirectory();
      flushPendingYpRefresh();
      scheduleYpAutoRefresh();
    }
  }

  function beginYpFetch(state, refresh) {
    if (state.ypBusy || state.busy) {
      state.ypRefreshPending = true;
      state.ypRefreshForcePending ||= Boolean(refresh);
      return false;
    }
    state.ypBusy = true;
    return true;
  }

  function scheduleYpAutoRefresh() {
    if (state.ypRefreshTimer !== null) window.clearTimeout(state.ypRefreshTimer);
    state.ypRefreshTimer = null;
    if (state.ypRefreshMode !== "auto") return;
    state.ypRefreshTimer = window.setTimeout(() => {
      state.ypRefreshTimer = null;
      void fetchYpChannels(true);
    }, state.ypRefreshIntervalSecs * 1000);
  }

  function applyDirectoryPreferences(directory = {}, syncForm = false) {
    const nextMode = directory.autoRefresh === false ? "manual" : "auto";
    const interval = Number(directory.refreshIntervalSecs);
    const nextInterval = Number.isFinite(interval) ? Math.min(3600, Math.max(60, Math.ceil(interval / 60) * 60)) : 300;
    const changed = state.ypRefreshMode !== nextMode || state.ypRefreshIntervalSecs !== nextInterval;
    const flagsFingerprint = JSON.stringify([directory.youtubeAtBottom === true, directory.twitchAtBottom === true]);
    const flagsChanged = flagsFingerprint !== state.ypDirectoryFlagsFingerprint;
    state.ypDirectoryFlagsFingerprint = flagsFingerprint;
    state.ypRefreshMode = nextMode;
    state.ypRefreshIntervalSecs = nextInterval;
    if (syncForm) state.ypDirectoryDirty = false;
    if (!state.ypDirectoryDirty || syncForm) {
      const radio = document.querySelector(`input[name="yp-refresh-mode"][value="${state.ypRefreshMode}"]`);
      if (radio) radio.checked = true;
      const field = byId("yp-refresh-interval");
      if (field) field.value = String(state.ypRefreshIntervalSecs / 60);
    }
    if (changed) scheduleYpAutoRefresh();
    return flagsChanged;
  }

  async function saveDirectoryPreferences() {
    const field = byId("yp-refresh-interval");
    const mode = document.querySelector('input[name="yp-refresh-mode"]:checked')?.value !== "manual";
    const interval = Math.min(60, Math.max(1, Math.trunc(Number(field?.value) || 5))) * 60;
    const current = state.snapshot || {};
    const desktop = current.desktop || {};
    const directory = { ...(desktop.directory || {}), autoRefresh: mode, refreshIntervalSecs: interval };
    try {
      const snapshot = await invoke("apply_preferences", { limits: current.limits || {}, yellowPages: current.yellowPages || [], desktop: { ...desktop, directory, window: desktop.window || { width: 1600, height: 900, minimizeToTray: false, launchAtStartup: false }, player: desktop.player || { executable: null, arguments: '\"<stream/>\" \"<channelname/>\" \"<contact/>\"' }, channelRules: desktop.channelRules || [], recording: desktop.recording || {} } });
      state.snapshot = snapshot;
      applyDirectoryPreferences(snapshot.desktop?.directory || directory, true);
      setNotice("一覧の更新設定を保存しました。");
    } catch (error) {
      setNotice(`一覧の更新設定を保存できませんでした: ${errorText(error)}`, true);
    }
  }

  function refreshYpAfterRuntimeChange(force = true) {
    if (!state.ypStarted && !state.ypLoaded && !state.ypBusy) return;
    state.ypRefreshPending = true;
    state.ypRefreshForcePending ||= Boolean(force);
    flushPendingYpRefresh();
  }

  function refreshYpAfterPreferencesChange(before, after) {
    const runtimePrefsChanged = JSON.stringify([before?.limits || {}, before?.yellowPages || []]) !== JSON.stringify([after?.limits || {}, after?.yellowPages || []]);
    if (runtimePrefsChanged) {
      refreshYpAfterRuntimeChange(true);
      return;
    }
    const channelRulesChanged = JSON.stringify(before?.desktop?.channelRules || []) !== JSON.stringify(after?.desktop?.channelRules || []);
    if (channelRulesChanged) refreshYpAfterRuntimeChange(false);
  }

  function flushPendingYpRefresh() {
    if (!state.ypRefreshPending || state.ypBusy || state.busy) return;
    const force = state.ypRefreshForcePending;
    state.ypRefreshPending = false;
    state.ypRefreshForcePending = false;
    void fetchYpChannels(force);
  }
  function notifyNewYpChannels(channels) {
    const notices = [];
    channels.forEach((channel) => {
      const annotation = channelAnnotation(channel);
      if (channel.favoriteNotificationSent && annotation.isFavorite && !annotation.isHidden && !annotation.ruleMute) {
        notices.push(displayValue(channel.name));
      }
    });
    if (notices.length) {
      setNotice(`お気に入りの新着: ${notices.slice(0, 5).join("、")}${notices.length > 5 ? `、ほか${notices.length - 5}件` : ""}`);
    }
  }

  function runtimeChannelFor(id) {
    const channels = Array.isArray(state.snapshot?.channels) ? state.snapshot.channels : [];
    return channels.find((channel) => String(channel.id || "").toLowerCase() === String(id || "").toLowerCase()) || null;
  }

  function safeHttpUrl(value) {
    try {
      const url = new URL(String(value || ""));
      return ["http:", "https:"].includes(url.protocol) ? url.href : "";
    } catch {
      return "";
    }
  }

  function setDetailMetadata(id, values) {
    const root = byId(id);
    if (!root) return;
    root.replaceChildren();
    values.forEach(([label, value]) => {
      const dt = document.createElement("dt");
      dt.textContent = label;
      const dd = document.createElement("dd");
      dd.textContent = displayValue(value);
      root.append(dt, dd);
    });
  }

  function renderSelectedDetail() {
    if (currentView() === "channels") {
      renderBroadcastDetail();
      return;
    }
    const broadcastDetailContent = byId("broadcast-detail-content");
    if (broadcastDetailContent) broadcastDetailContent.hidden = true;
    const empty = byId("detail-empty");
    const content = byId("detail-content");
    const isYp = state.detailKind === "yp";
    const selected = isYp ? currentYpChannel() : state.detailKind === "channel" ? runtimeChannelFor(state.channelSelectionId) : null;
    const directory = selected && !isYp ? directoryChannelFor(selected.id) : selected;
    if (!selected) {
      if (empty) empty.hidden = false;
      if (content) content.hidden = true;
      state.detailKind = null;
      state.detailKey = null;
      text("detail-title", "");
      text("detail-source", "");
      text("detail-description", "");
      text("detail-comment", "");
      text("detail-genre", "");
      setDetailMetadata("detail-metadata", []);
      setDetailMetadata("detail-extra-metadata", []);
      updateDetailActions(null);
      updateConnectionSummary(null);
      return;
    }
    if (empty) empty.hidden = true;
    if (content) content.hidden = false;
    text("detail-title", displayValue(directory?.name || selected.name));
    text("detail-source", directory ? displayValue(directory.yellowPage) : isYp ? displayValue(selected.yellowPage) : "このアプリのチャンネル");
    text("detail-description", displayValue(directory?.description));
    text("detail-comment", displayValue(directory?.comment));
    text("detail-genre", displayValue(directory?.genre));
    const info = directory || {};
    const rate = counter(info.bitrate);
    setDetailMetadata("detail-metadata", [
      ["形式", info.contentType], ["ビットレート", rate === "—" ? "—" : `${rate} kbps`],
      ["配信時間", formatDuration(info.uptime)], ["視聴 / リレー", `${counter(info.listeners)} / ${counter(info.relays)}`],
    ]);
    setDetailMetadata("detail-extra-metadata", [
      ["チャンネルID", info.channelId || selected.id], ["トラック", info.trackTitle], ["アルバム", info.album],
      ["アーティスト", info.creator], ["トラックURL", info.trackUrl],
    ]);
    updateDetailActions(directory);
    updateConnectionSummary(isYp ? runtimeChannelFor(selected.channelId) : selected);
  }

  function updateDetailActions(channel) {
    const contact = byId("copy-contact");
    if (contact) contact.disabled = !channel || !safeHttpUrl(channel.contactUrl);
    const stream = byId("copy-stream");
    const exists = Boolean(channel && runtimeChannelFor(channel.channelId));
    if (stream) stream.disabled = !state.snapshot?.running || !exists || !channel?.channelId;
    const addFavorite = byId("add-favorite-rule");
    const addNg = byId("add-ng-rule");
    if (addFavorite) addFavorite.disabled = !channel || !channel.name || state.busy;
    if (addNg) addNg.disabled = !channel || !channel.name || state.busy;
  }

  function updateConnectionSummary(channel) {
    const summary = byId("connection-summary");
    if (!summary) return;
    summary.replaceChildren();
    const values = channel
      ? [["状態", channel.status], ["バッファ", channel.sessionId ? `${formatBytes(channel.bufferedBytes)} / ${counter(channel.bufferedPackets)} パケット` : null], ["接続試行", counter(channel.connectionAttempts)]]
      : [["状態", "未接続"]];
    values.forEach(([label, value]) => {
      const term = document.createElement("dt");
      term.textContent = label;
      const detail = document.createElement("dd");
      detail.textContent = displayValue(value);
      summary.append(term, detail);
    });
  }

  function activateView(view) {
    if (!viewTitles[view]) return;
    if (view !== currentView()) setNotice("");
    document.querySelectorAll("[data-view]").forEach((button) => {
      const active = button.dataset.view === view;
      button.classList.toggle("active", active);
      if (active) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    });
    const directoryTools = byId("directory-tools");
    if (directoryTools) directoryTools.hidden = view !== "yp";
    const ypRefresh = byId("yp-refresh");
    if (ypRefresh) ypRefresh.hidden = view !== "yp";
    const snapshotRefresh = byId("refresh-button");
    if (snapshotRefresh) snapshotRefresh.hidden = view === "yp" || view === "settings";
    ["yp", "channels", "connections", "recordings", "logs", "settings"].forEach((name) => {
      const element = byId(`view-${name}`);
      if (element) element.hidden = name !== view;
    });
    const workspace = document.querySelector(".workspace");
    if (workspace) {
      workspace.classList.toggle("logs-view", view === "logs");
      workspace.classList.toggle("recordings-view", view === "recordings");
      workspace.classList.toggle("connections-view-active", view === "connections");
      workspace.hidden = view === "settings";
    }
    const broadcast = byId("header-broadcast");
    if (broadcast) broadcast.hidden = view === "settings" || view === "recordings";
    const settingsView = byId("view-settings");
    if (settingsView) settingsView.hidden = view !== "settings";
    text("view-title", viewTitles[view]);
    text("titlebar-view", viewTitles[view]);
    syncYpFooterCountVisibility();
    if (view === "recordings") text("yp-count", `${Array.isArray(state.snapshot?.recordings) ? state.snapshot.recordings.length : 0} 件の録画`);
    else if (view === "yp") text("yp-count", `${countableYpChannelCount(visibleYpChannels())} 件の配信`);
    else if (view === "channels") text("yp-count", `${broadcastingChannels(state.snapshot).length} 件の配信`);
    else if (view === "connections") text("yp-count", `${allRuntimeChannels(state.snapshot).length} 件の接続チャンネル`);
    if (view === "yp") {
      ensureVisibleYpSelection(visibleYpChannels());
      state.detailKind = state.ypSelectionKey ? "yp" : null;
      state.detailKey = state.ypSelectionKey;
      renderSelectedDetail();
    } else if (view === "channels") {
      const channels = broadcastingChannels(state.snapshot);
      if (!channels.some((channel) => channel.id === state.channelSelectionId)) state.channelSelectionId = channels[0]?.id || null;
      state.detailKind = state.channelSelectionId ? "channel" : null;
      state.detailKey = state.channelSelectionId;
      renderChannels(state.snapshot || { channels: [] });
      renderSelectedDetail();
    } else if (view === "connections") {
      const channels = allRuntimeChannels(state.snapshot);
      if (!channels.some((channel) => connectionChannelKey(channel) === String(state.connectionSelectionId))) {
        state.connectionSelectionId = channels[0] ? connectionChannelKey(channels[0]) : null;
      }
      renderConnections(state.snapshot || { channels: [] });
    } else if (view === "recordings") renderRecordings(state.snapshot || { recordings: [] });
    else if (view === "logs") renderLogs();
    if (view === "yp" && !state.ypLoaded && !state.ypBusy) fetchYpChannels(false);
  }

  async function mountSettings() {
    if (state.settingsView) return;
    if (state.settingsMountPromise) return state.settingsMountPromise;
    state.settingsMountPromise = (async () => {
      await state.capabilitiesReady;
      const host = byId("settings-host");
      if (!host) throw new Error("設定画面の表示領域が見つかりません。");
      if (typeof window.createSettingsView !== "function") throw new Error("設定画面を初期化できません。");
      const sourceUrl = new URL("settings.html", window.location.href);
      const response = await fetch(sourceUrl);
      if (!response.ok) throw new Error(`設定画面を読み込めません (${response.status})。`);
      const parsed = new DOMParser().parseFromString(await response.text(), "text/html");
      const shadow = host.shadowRoot || host.attachShadow({ mode: "open" });
      shadow.replaceChildren();
      ["settings.css", "settings-embedded.css"].forEach((path) => {
        const link = document.createElement("link");
        link.rel = "stylesheet";
        link.href = new URL(path, window.location.href).href;
        shadow.append(link);
      });
      [...parsed.body.childNodes].forEach((node) => shadow.append(document.importNode(node, true)));
      const settingsView = window.createSettingsView(shadow, {
        platformCapabilities: state.capabilities,
        initialSnapshot: state.snapshot,
        onBack: () => requestView("yp"),
        onSaved: (snapshot) => {
          const before = state.snapshot || {};
          renderSnapshot(snapshot);
          refreshYpAfterPreferencesChange(before, snapshot);
        },
      });
      state.settingsView = settingsView;
    })();
    try {
      await state.settingsMountPromise;
    } catch (error) {
      state.settingsMountPromise = null;
      throw error;
    }
  }

  async function requestView(view) {
    if (!viewTitles[view] || state.viewTransition || view === currentView()) return false;
    state.viewTransition = true;
    try {
      if (state.settingsView && !(await state.settingsView.requestLeave())) return false;
      if (view === "settings") await mountSettings();
      activateView(view);
      return true;
    } catch (error) {
      setNotice(`画面を切り替えられませんでした: ${errorText(error)}`, true);
      return false;
    } finally {
      state.viewTransition = false;
    }
  }

  async function closeMainWindow() {
    if (state.closingMain) return false;
    state.closingMain = true;
    try {
      if (state.settingsMountPromise) await state.settingsMountPromise;
      if (state.settingsView && !(await state.settingsView.requestLeave())) return false;
      await invoke("close_window");
      return true;
    } catch (error) {
      setNotice(`終了できませんでした: ${errorText(error)}`, true);
      return false;
    } finally {
      state.closingMain = false;
    }
  }
  window.requestMainClose = closeMainWindow;

  function activateDetailTab(name) {
    if (!["info", "connections", "tree"].includes(name)) return;
    state.detailTab = name;
    document.querySelectorAll("[data-detail-tab]").forEach((button) => {
      const active = button.dataset.detailTab === name;
      button.classList.toggle("active", active);
      button.setAttribute("aria-selected", String(active));
      button.tabIndex = active ? 0 : -1;
    });
    ["info", "connections", "tree"].forEach((panel) => {
      const element = byId(`detail-${panel}`);
      if (element) element.hidden = panel !== name;
    });
  }

  function activateConnectionTab(name) {
    if (!["connections", "info", "tree"].includes(name)) return;
    state.connectionTab = name;
    document.querySelectorAll("[data-connection-tab]").forEach((button) => {
      const active = button.dataset.connectionTab === name;
      button.classList.toggle("active", active);
      button.setAttribute("aria-selected", String(active));
      button.tabIndex = active ? 0 : -1;
    });
    ["connections", "info", "tree"].forEach((panel) => {
      const element = byId(panel === "connections" ? "connections-panel" : `connections-${panel}-panel`);
      if (element) element.hidden = panel !== name;
    });
  }

  function allRuntimeChannels(snapshot) {
    return Array.isArray(snapshot?.channels) ? snapshot.channels : [];
  }

  function networkStatusLabel(status) {
    return ({ open: "リレー可能", closed: "ポート未開放", unknown: "ポート未確認", unavailable: "利用不可" })[status] || "ポート未確認";
  }

  function networkOverallStatus(network = {}) {
    const statuses = [network.ipv4, network.ipv6];
    if (statuses.includes("open")) return "open";
    if (statuses.includes("closed") && statuses.every((status) => status === "closed" || status === "unavailable")) return "closed";
    if (statuses.every((status) => status === "unavailable")) return "unavailable";
    return "unknown";
  }

  function connectionPortLabel(portOpen) {
    return portOpen === true ? "開放" : portOpen === false ? "閉鎖" : "未確認";
  }

  function connectionKindLabel(kind) {
    return ({ source: "上流ソース", relay: "子リレー", direct: "直接接続", ingest: "入力", announce: "YP掲載", metadata: "メタデータ", interface: "管理接続" })[kind] || "接続";
  }

  function connectionStateLabel(status) {
    return ({ connected: "接続済み", connecting: "接続中", idle: "待機", error: "接続エラー" })[status] || displayValue(status);
  }

  function connectionChannelKey(channel) {
    return `${channel.id}:${channel.sessionId || ""}`;
  }

  function hasValidConnectionClearKey(channel) {
    return Boolean(String(channel?.id || "").trim() && String(channel?.sessionId || "").trim());
  }

  function isClearableDisconnectedConnection(channel) {
    return hasValidConnectionClearKey(channel)
      && channel.canReconnect === true
      && channel.canDisconnect === false
      && channel.isBroadcasting !== true;
  }

  function filterClearedConnectionSnapshot(snapshot, existingKeys, preserveKeys = false) {
    const channels = Array.isArray(snapshot?.channels) ? snapshot.channels : [];
    const clearedConnectionKeys = new Set();
    if (preserveKeys) {
      existingKeys.forEach((key) => clearedConnectionKeys.add(key));
    } else {
      const channelsByKey = new Map(channels
        .filter(hasValidConnectionClearKey)
        .map((channel) => [connectionChannelKey(channel), channel]));
      existingKeys.forEach((key) => {
        const channel = channelsByKey.get(key);
        if (channel && isClearableDisconnectedConnection(channel)) clearedConnectionKeys.add(key);
      });
    }
    const visibleChannels = channels.filter((channel) => !clearedConnectionKeys.has(connectionChannelKey(channel)));
    return {
      snapshot: { ...snapshot, channels: visibleChannels },
      clearedConnectionKeys,
    };
  }

  function clearDisconnectedConnections(snapshot, existingKeys) {
    const channels = Array.isArray(snapshot?.channels) ? snapshot.channels : [];
    const clearedConnectionKeys = new Set(existingKeys);
    channels.forEach((channel) => {
      if (isClearableDisconnectedConnection(channel)) clearedConnectionKeys.add(connectionChannelKey(channel));
    });
    return {
      snapshot: { ...snapshot, channels: channels.filter((channel) => !clearedConnectionKeys.has(connectionChannelKey(channel))) },
      clearedConnectionKeys,
    };
  }

  function recordingIsActive(job) {
    return Boolean(job && ["queued", "connecting", "recording", "retrying"].includes(job.status));
  }
  function displayRecordingPath(value) {
    const path = String(value ?? "");
    if (/^\\\\\?\\UNC\\/i.test(path)) return path.replace(/^\\\\\?\\UNC\\/i, "\\\\");
    if (/^\\\\\?\\[A-Za-z]:\\/.test(path)) return path.slice(4);
    return path;
  }
  function canUseRecordingFile(job, action, busy) {
    if (!job?.currentFile || busy) return false;
    return action !== "trash" || !recordingIsActive(job);
  }

  function renderConnections(snapshot) {
    const channels = allRuntimeChannels(snapshot);
    const list = byId("connections-channel-list");
    if (!list) return;
    const fingerprint = JSON.stringify(channels.map((channel) => [connectionChannelKey(channel), channel.purpose, channel.name, channel.status, channel.isBroadcasting, channel.listeners, channel.relays]));
    if (fingerprint !== state.connectionFingerprint) {
      state.connectionFingerprint = fingerprint;
      if (!channels.some((channel) => connectionChannelKey(channel) === String(state.connectionSelectionId))) {
        state.connectionSelectionId = channels[0] ? connectionChannelKey(channels[0]) : null;
      }
      list.replaceChildren();
      if (!channels.length) appendText(list, "p", "connections-list-empty", "接続チャンネルはありません。");
      channels.forEach((channel) => {
        const id = connectionChannelKey(channel);
        const selected = id === String(state.connectionSelectionId ?? "");
        const row = document.createElement("button");
        row.type = "button";
        row.className = "connection-channel-row";
        row.dataset.connectionChannelId = id;
        row.classList.toggle("selected", selected);
        row.setAttribute("aria-current", String(selected));
        appendText(row, "strong", "connection-row-name", displayValue(channel.name));
        appendText(row, "span", `connection-row-badge${channel.isBroadcasting ? " is-source" : ""}`, channel.purpose === "recording" ? "録画" : channel.isBroadcasting ? "配信" : "視聴 / リレー");
        appendText(row, "span", "connection-row-state", displayValue(channel.status));
        appendText(row, "span", "connection-row-counts", `視聴 ${counter(channel.listeners)}　リレー ${counter(channel.relays)}`);
        list.append(row);
      });
    } else {
      list.querySelectorAll("[data-connection-channel-id]").forEach((row) => {
        const selected = row.dataset.connectionChannelId === String(state.connectionSelectionId ?? "");
        row.classList.toggle("selected", selected);
        row.setAttribute("aria-current", String(selected));
      });
    }
    text("connections-count", String(channels.length));
    renderConnectionDetails(channels.find((channel) => connectionChannelKey(channel) === String(state.connectionSelectionId)) || null);
  }

  function renderConnectionDetails(channel) {
    const empty = byId("connections-empty");
    const detail = byId("connections-detail");
    if (empty) empty.hidden = Boolean(channel);
    if (detail) detail.hidden = !channel;
    if (!channel) return;

    text("connections-channel-name", displayValue(channel.name));
    text("connections-channel-status", displayValue(channel.status));
    text("connections-channel-type", channel.purpose === "recording" ? "録画" : channel.isBroadcasting ? "配信" : "視聴 / リレー");

    const network = channel.network || {};
    const networkRoot = byId("connections-network-status");
    if (networkRoot) {
      networkRoot.replaceChildren();
      const overall = networkOverallStatus(network);
      appendText(networkRoot, "span", `network-state network-${overall}`, overall === "closed" ? "リレー不可 · ポート未開放" : networkStatusLabel(overall));
      if (Number.isFinite(Number(network.relayPort)) && Number(network.relayPort) > 0) appendText(networkRoot, "span", "network-port", `ポート ${network.relayPort}`);
      appendText(networkRoot, "span", "network-family", `IPv4 ${networkStatusLabel(network.ipv4)} · IPv6 ${networkStatusLabel(network.ipv6)}`);
    }

    const connections = Array.isArray(channel.connections) ? channel.connections : [];
    const rowsRoot = byId("connection-rows");
    if (rowsRoot) {
      rowsRoot.replaceChildren();
      if (!connections.length) appendText(rowsRoot, "p", "connection-empty", channel.available === false ? "接続の詳細情報を取得できませんでした。" : "接続先はありません。");
      connections.forEach((connection) => {
        const row = document.createElement("article");
        row.className = "connection-row";
        const header = document.createElement("div");
        header.className = "connection-row-heading";
        appendText(header, "strong", "", connectionKindLabel(connection.kind));
        appendText(header, "span", `connection-state connection-${connection.status || "unknown"}`, connectionStateLabel(connection.status));
        row.append(header);
        appendText(row, "p", "connection-endpoint", displayValue(connection.endpoint));
        const metrics = document.createElement("div");
        metrics.className = "connection-row-metrics";
        appendText(metrics, "span", "", `受信 ${connection.measurementsAvailable === false ? "—" : formatBytes(connection.receivedBytes)}`);
        appendText(metrics, "span", "", `送信 ${connection.measurementsAvailable === false ? "—" : formatBytes(connection.sentBytes)}`);
        appendText(metrics, "span", "", `ポート ${connectionPortLabel(connection.portOpen)}`);
        appendText(metrics, "span", "", connection.measurementsAvailable === false ? "接続時間 —" : formatDuration(connection.uptimeSecs));
        appendText(metrics, "span", "", `Agent ${displayValue(connection.agent)}`);
        appendText(metrics, "span", "", `Session ${displayValue(connection.sessionId)}`);
        row.append(metrics);
        rowsRoot.append(row);
      });
    }

    const metadata = byId("connections-metadata");
    if (metadata) {
      metadata.replaceChildren();
      const values = [
        ["チャンネルID", channel.id], ["セッションID", channel.sessionId], ["状態", channel.status], ["種別", channel.purpose === "recording" ? "録画" : channel.isBroadcasting ? "配信" : "視聴 / リレー"],
        ["形式", channel.contentType], ["ジャンル", channel.genre], ["ビットレート", channel.bitrate == null ? null : `${counter(channel.bitrate)} kbps`],
        ["視聴者", channel.listeners], ["リレー数", channel.relays], ["リレーポート", network.relayPort], ["観測IPv4", network.observedIpv4], ["観測IPv6", network.observedIpv6],
        ["バッファ", channel.sessionId ? `${formatBytes(channel.bufferedBytes)} / ${counter(channel.bufferedPackets)} パケット` : null], ["稼働時間", formatDuration(channel.uptimeSecs)],
        ["コンタクトURL", channel.contactUrl], ["コメント", channel.comment],
      ];
      values.forEach(([label, value]) => {
        const wrapper = document.createElement("div");
        appendText(wrapper, "dt", "", label);
        appendText(wrapper, "dd", "", displayValue(value));
        metadata.append(wrapper);
      });
      text("connections-description", displayValue(channel.description));
    }

    const tree = byId("connection-tree");
    if (tree) {
      tree.replaceChildren();
      const root = document.createElement("div");
      root.className = "connection-tree-root";
      appendText(root, "strong", "", displayValue(channel.name));
      appendText(root, "span", "", "このチャンネル");
      appendText(root, "small", "", `Session ${displayValue(channel.sessionId)}`);
      const rootPortState = networkOverallStatus(channel.network || {});
      const rootPortLabel = rootPortState === "closed"
        ? "× リレー不可 · ポート未開放"
        : rootPortState === "open"
          ? "✓ リレー可能"
          : rootPortState === "unavailable" ? "利用不可" : "ポート未確認";
      appendText(root, "span", `connection-port-state connection-port-${rootPortState}`, rootPortLabel);
      tree.append(root);
      const children = connections.filter((connection) => connection.kind === "relay");
      children.forEach((connection) => {
        const child = document.createElement("div");
        child.className = "connection-tree-child";
        appendText(child, "strong", "", displayValue(connection.agent || connection.endpoint));
        appendText(child, "span", "", `${connectionKindLabel(connection.kind)} · ${connectionStateLabel(connection.status)}`);
        appendText(child, "small", "", displayValue(connection.endpoint));
        if (connection.sessionId) appendText(child, "small", "", `Session ${displayValue(connection.sessionId)}`);
        const portState = connection.portOpen === false ? "closed" : connection.portOpen === true ? "open" : "unknown";
        const portLabel = portState === "closed" ? "× ポート未開放" : portState === "open" ? "✓ ポート開放" : "ポート未確認";
        appendText(child, "span", `connection-port-state connection-port-${portState}`, portLabel);
        tree.append(child);
      });
      const sources = connections.filter((connection) => connection.kind === "source");
      sources.forEach((connection) => {
        const source = document.createElement("div");
        source.className = "connection-tree-source";
        appendText(source, "strong", "", connectionKindLabel(connection.kind));
        appendText(source, "span", "", `${connectionStateLabel(connection.status)} · ${displayValue(connection.endpoint)}`);
        tree.insertBefore(source, root);
      });
      if (!children.length && !sources.length) appendText(tree, "p", "connection-empty", "リレー接続はありません。");
    }
    activateConnectionTab(state.connectionTab);
  }

  function renderYpSourcesAndMaybeRefresh(changed) {
    if (changed && state.ypLoaded) renderYpDirectory();
  }

  function renderChannels(snapshot) {
    const root = byId("channels-list");
    if (!root) return;
    const channels = broadcastingChannels(snapshot);
    const fingerprint = JSON.stringify(channels.map((channel) => [channel.id, channel.name, channel.status, channel.listeners, channel.relays, channel.isBroadcasting]));
    if (fingerprint === state.channelFingerprint) {
      syncChannelSelection();
      if (state.detailKind === "channel") renderSelectedDetail();
      return;
    }
    state.channelFingerprint = fingerprint;
    if (state.channelSelectionId && !channels.some((channel) => channel.id === state.channelSelectionId)) {
      state.channelSelectionId = null;
      if (state.detailKind === "channel") {
        state.detailKind = null;
        renderSelectedDetail();
      }
    }
    if (currentView() === "channels" && !state.channelSelectionId && channels.length) {
      state.channelSelectionId = channels[0].id;
      state.detailKind = "channel";
      state.detailKey = state.channelSelectionId;
    }
    root.replaceChildren();
    if (!channels.length) appendText(root, "p", "empty", "配信中のチャンネルはありません。");
    channels.forEach((channel) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "channel-row";
      row.dataset.channelId = String(channel.id || "");
      row.classList.toggle("selected", channel.id === state.channelSelectionId);
      appendText(row, "span", "channel-row-name", displayValue(channel.name));
      const listeners = channel.listeners != null && Number.isFinite(Number(channel.listeners)) ? Number(channel.listeners) : "—";
      const relays = channel.relays != null && Number.isFinite(Number(channel.relays)) ? Number(channel.relays) : "—";
      appendText(row, "span", "channel-row-status", `${displayValue(channel.status)} · 視聴 ${listeners} · リレー ${relays}`);
      root.append(row);
    });
    syncChannelSelection();
    text("channel-count", String(channels.length));
  }

  function broadcastingChannels(snapshot) {
    return Array.isArray(snapshot?.channels)
      ? snapshot.channels.filter((channel) => isBroadcastingChannel(channel) && String(channel.id || "").toLowerCase() !== internalTestId)
      : [];
  }

  function renderBroadcastDetail() {
    const empty = byId("detail-empty");
    const generic = byId("detail-content");
    const detail = byId("broadcast-detail-content");
    const channel = broadcastingChannels(state.snapshot).find((entry) => String(entry.id || "") === String(state.channelSelectionId || ""));
    const broadcasts = Array.isArray(state.snapshot?.broadcasts) ? state.snapshot.broadcasts : [];
    const broadcast = broadcasts.find((entry) => String(entry.channelId || "") === String(state.channelSelectionId || "")) || channel;
    if (generic) generic.hidden = true;
    if (!channel || !broadcast) {
      if (detail) detail.hidden = true;
      if (empty) empty.hidden = false;
      const heading = empty?.querySelector("strong");
      const description = empty?.querySelector("p");
      if (heading) heading.textContent = "配信を選択";
      if (description) description.textContent = "配信一覧からチャンネルを選ぶと、入力URLや掲載状態を確認できます。";
      return;
    }
    if (empty) empty.hidden = true;
    if (detail) detail.hidden = false;
    text("broadcast-detail-name", displayValue(broadcast.name || channel.name));
    text("broadcast-detail-status", displayValue(broadcast.status || channel.status));
    text("broadcast-detail-manager", broadcast.managedByRt ? "Peercast RT" : (state.snapshot?.backendLabel || "PeerCastStation C#"));
    text("broadcast-detail-input-url", displayValue(broadcast.inputUrl));
    text("broadcast-detail-stream-url", displayValue(broadcast.streamUrl));
    text("broadcast-detail-announcement", displayValue(broadcast.announcementStatus || "掲載していません"));
    const stop = byId("broadcast-stop-button");
    if (stop) stop.disabled = state.broadcastBusy || state.stoppingBroadcasts.has(String(broadcast.channelId || channel.id));
  }

  function updateBroadcastSourceFields() {
    const push = byId("broadcast-source-mode")?.value !== "pull";
    const ingest = byId("broadcast-ingest-field");
    const source = byId("broadcast-source-field");
    if (ingest) ingest.hidden = !push;
    if (source) source.hidden = push;
    const sourceUri = byId("broadcast-source-uri");
    if (sourceUri) sourceUri.required = !push;
  }

  function restoreBroadcastForm(request) {
    if (!request || state.broadcastFormDirty) return false;
    const values = {
      name: request.name,
      contentType: request.contentType === "WMV" ? "WMV" : "FLV",
      genre: request.genre,
      description: request.description,
      comment: request.comment,
      contactUrl: request.contactUrl,
      bitrate: request.bitrate,
      sourceMode: request.sourceMode === "pull" ? "pull" : "push",
      sourceUri: request.sourceUri,
      ingestPort: request.ingestPort ?? 8080,
      relayPort: request.relayPort ?? 7144,
    };
    for (const [name, value] of Object.entries(values)) {
      const field = byId(`broadcast-${name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`);
      if (field && value != null) field.value = String(value);
    }
    const announceSp = byId("broadcast-announce-sp");
    if (announceSp) announceSp.checked = request.announceSp === true;
    updateBroadcastSourceFields();
    return true;
  }

  function restoreLastBroadcastOnce(snapshot) {
    if (state.broadcastFormInitialized) return false;
    state.broadcastFormInitialized = true;
    return restoreBroadcastForm(snapshot?.desktop?.lastBroadcast);
  }

  function updateBroadcastFormBusy() {
    const form = byId("broadcast-start-form");
    if (form) form.querySelectorAll("input, select, textarea, button").forEach((control) => { control.disabled = state.broadcastBusy; });
    const start = byId("broadcast-start-button");
    if (start) start.disabled = state.broadcastBusy;
    updateBroadcastBackendFields();
    renderBroadcastDetail();
  }

  function updateBroadcastBackendFields() {
    const csharpBackend = state.snapshot?.backendKind === "csharp";
    const relayPort = byId("broadcast-relay-port");
    const note = byId("broadcast-relay-port-note");
    if (relayPort) relayPort.disabled = state.broadcastBusy || csharpBackend;
    if (note) {
      note.hidden = !csharpBackend;
      note.textContent = csharpBackend ? "C#版では既存のリレーポート設定を使用します。この値は適用されません。" : "";
    }
  }

  function createBroadcastRequest(values) {
    const sourceMode = values.sourceMode === "pull" ? "pull" : "push";
    return {
      name: String(values.name || "").trim(),
      genre: String(values.genre || "").trim(),
      description: String(values.description || "").trim(),
      comment: String(values.comment || "").trim(),
      contactUrl: String(values.contactUrl || "").trim(),
      contentType: values.contentType === "WMV" ? "WMV" : "FLV",
      bitrate: Number(values.bitrate),
      sourceMode,
      sourceUri: sourceMode === "pull" ? String(values.sourceUri || "").trim() : "",
      ingestPort: values.ingestPort === "" || values.ingestPort == null ? 8080 : Number(values.ingestPort),
      relayPort: values.relayPort === "" || values.relayPort == null ? 7144 : Number(values.relayPort),
      announceSp: Boolean(values.announceSp),
    };
  }

  function validateBroadcastRequest(request, isHttpUrl) {
    if (!request.name) return "配信名を入力してください。";
    if (!Number.isInteger(request.bitrate) || request.bitrate < 1 || request.bitrate > 1000000) return "ビットレートは1〜1000000 kbpsの整数で入力してください。";
    if (!Number.isInteger(request.relayPort) || request.relayPort < 1 || request.relayPort > 65535) return "リレーポートは1〜65535で入力してください。";
    if (request.sourceMode === "push" && (!Number.isInteger(request.ingestPort) || request.ingestPort < 1 || request.ingestPort > 65535)) return "受信ポートは1〜65535で入力してください。";
    if (request.sourceMode === "pull" && !isHttpUrl(request.sourceUri)) return "入力URLはHTTPまたはHTTPSで入力してください。";
    if (request.contactUrl && !isHttpUrl(request.contactUrl)) return "コンタクトURLはHTTPまたはHTTPSで入力してください。";
    return "";
  }

  function beginBroadcastStart(state) {
    if (state.broadcastBusy) return false;
    state.broadcastBusy = true;
    return true;
  }
  function endBroadcastStart(state) {
    state.broadcastBusy = false;
  }
  function beginBroadcastStop(state, channelId) {
    const id = String(channelId || "");
    if (!id || state.broadcastBusy || state.stoppingBroadcasts.has(id)) return false;
    state.stoppingBroadcasts.add(id);
    return true;
  }
  function endBroadcastStop(state, channelId) {
    state.stoppingBroadcasts.delete(String(channelId || ""));
  }

  async function startBroadcast(event) {
    event.preventDefault();
    if (state.broadcastBusy) return;
    const form = byId("broadcast-start-form");
    if (!form?.reportValidity()) return;
    const value = (name) => String(form.elements.namedItem(name)?.value || "").trim();
    const sourceMode = value("sourceMode") === "pull" ? "pull" : "push";
    const sourceUri = value("sourceUri");
    const contactUrl = value("contactUrl");
    const request = createBroadcastRequest({
      name: value("name"),
      genre: value("genre"),
      description: value("description"),
      comment: value("comment"),
      contactUrl,
      contentType: value("contentType"),
      bitrate: value("bitrate"),
      sourceMode,
      sourceUri,
      ingestPort: value("ingestPort"),
      relayPort: value("relayPort"),
      announceSp: Boolean(byId("broadcast-announce-sp")?.checked),
    });
    const validationError = validateBroadcastRequest(request, (url) => Boolean(safeHttpUrl(url)));
    if (validationError) {
      setNotice(validationError, true);
      return;
    }
    if (!beginBroadcastStart(state)) return;
    const previousIds = new Set((state.snapshot?.broadcasts || []).map((broadcast) => String(broadcast.channelId || "")));
    text("broadcast-form-status", "配信を開始しています…");
    updateBroadcastFormBusy();
    try {
      const snapshot = await invoke("start_broadcast", { request });
      const created = (snapshot.broadcasts || []).find((broadcast) => !previousIds.has(String(broadcast.channelId || "")));
      if (created?.channelId) state.channelSelectionId = String(created.channelId);
      state.detailKind = state.channelSelectionId ? "channel" : null;
      state.detailKey = state.channelSelectionId;
      renderSnapshot(snapshot);
      state.broadcastFormDirty = false;
      const details = byId("broadcast-form");
      if (details) details.open = false;
      setNotice("配信を開始しました。配信情報の入力URLへエンコーダーから送信してください。");
      text("broadcast-form-status", "");
    } catch (error) {
      const message = errorText(error);
      text("broadcast-form-status", message);
      setNotice(`配信を開始できませんでした: ${message}`, true);
    } finally {
      endBroadcastStart(state);
      updateBroadcastFormBusy();
    }
  }

  async function stopBroadcast(channelId) {
    const id = String(channelId || "");
    if (!beginBroadcastStop(state, id)) return;
    renderBroadcastDetail();
    try {
      renderSnapshot(await invoke("stop_broadcast", { channelId: id }));
      setNotice("配信を停止しました。");
    } catch (error) {
      setNotice(`配信を停止できませんでした: ${errorText(error)}`, true);
    } finally {
      endBroadcastStop(state, id);
      renderBroadcastDetail();
    }
  }

  function syncChannelSelection() {
    const root = byId("channels-list");
    if (!root) return;
    root.querySelectorAll("[data-channel-id]").forEach((row) => {
      const selected = row.dataset.channelId === state.channelSelectionId;
      row.classList.toggle("selected", selected);
      if (selected) row.setAttribute("aria-current", "true");
      else row.removeAttribute("aria-current");
    });
  }

  const recordingStatusLabels = { queued: "待機中", connecting: "接続中", recording: "録画中", retrying: "再試行中", stopped: "停止", completed: "完了", failed: "失敗" };
  function recordingMonitorLabel(snapshot) {
    const config = snapshot?.desktop?.recording;
    if (!config?.enabled) return "自動監視: 設定オフ";
    if (!snapshot.running) return "自動監視: バックエンド停止中";
    if (!config.allDay) {
      const toMinutes = (value) => /^([01]\d|2[0-3]):[0-5]\d$/.test(String(value || "")) ? Number(value.slice(0, 2)) * 60 + Number(value.slice(3, 5)) : null;
      const start = toMinutes(config.watchStart), end = toMinutes(config.watchEnd);
      const now = new Date(), current = now.getHours() * 60 + now.getMinutes();
      if (start === null || end === null) return "自動監視: 時刻設定を確認してください";
      const within = start <= end ? current >= start && current <= end : current >= start || current <= end;
      if (!within) return "自動監視: 設定時間外";
    }
    return "自動監視: 監視中";
  }
  function renderRecordings(snapshot) {
    const root = byId("recordings-list");
    if (!root) return;
    const jobs = Array.isArray(snapshot?.recordings) ? snapshot.recordings : [];
    if (!jobs.some((job) => job.id === state.recordingSelectionId)) state.recordingSelectionId = jobs[0]?.id || null;
    const fingerprint = JSON.stringify(jobs.map((job) => String(job.id)));
    if (fingerprint !== state.recordingListFingerprint) {
      state.recordingListFingerprint = fingerprint;
      root.replaceChildren();
      jobs.forEach((job) => {
        const row = document.createElement("tr"); row.dataset.recordingId = String(job.id); row.tabIndex = 0; row.setAttribute("aria-selected", "false");
        const status = document.createElement("td"); status.className = "recording-status-cell";
        const channel = document.createElement("td"); channel.className = "recording-channel-cell";
        const name = document.createElement("strong"); name.className = "recording-channel-name";
        const source = document.createElement("small"); source.className = "recording-channel-source"; channel.append(name, source);
        const bytes = document.createElement("td"); bytes.className = "recording-bytes-cell";
        const segments = document.createElement("td"); segments.className = "recording-segments-cell";
        const retries = document.createElement("td"); retries.className = "recording-retries-cell";
        const file = document.createElement("td"); file.className = "recording-file-cell";
        const error = document.createElement("td"); error.className = "recording-error-cell";
        row.append(status, channel, bytes, segments, retries, file, error); root.append(row);
      });
    }
    const rows = new Map([...root.querySelectorAll("tr[data-recording-id]")].map((row) => [row.dataset.recordingId, row]));
    jobs.forEach((job) => {
      const row = rows.get(String(job.id)); if (!row) return;
      const status = Object.hasOwn(recordingStatusLabels, String(job.status || "")) ? String(job.status) : "unknown";
      row.classList.toggle("selected", job.id === state.recordingSelectionId);
      row.setAttribute("aria-selected", String(job.id === state.recordingSelectionId));
      const statusCell = row.querySelector(".recording-status-cell");
      statusCell.textContent = recordingStatusLabels[status] || displayValue(job.status);
      statusCell.className = `recording-status-cell recording-status-${status}`;
      row.querySelector(".recording-channel-name").textContent = displayValue(job.name);
      row.querySelector(".recording-channel-source").textContent = `${displayValue(job.yellowPage)} · ${displayValue(job.channelId)}`;
      row.querySelector(".recording-bytes-cell").textContent = formatBytes(job.bytesWritten);
      row.querySelector(".recording-segments-cell").textContent = String(Number(job.segments) || 0);
      row.querySelector(".recording-retries-cell").textContent = String(Number(job.retries) || 0);
      row.querySelector(".recording-file-cell").textContent = displayRecordingPath(job.currentFile);
      row.querySelector(".recording-error-cell").textContent = displayValue(job.error);
    });
    byId("recordings-empty").hidden = jobs.length > 0;
    const monitoring = byId("recording-monitor-status");
    if (monitoring) monitoring.textContent = recordingMonitorLabel(snapshot);
    const selected = jobs.find((job) => job.id === state.recordingSelectionId);
    const summary = byId("recording-selection-summary");
    if (summary) {
      if (!selected) summary.textContent = "録画を選択してください。";
      else {
        const started = Number.isFinite(Number(selected.startedAtMs)) && selected.startedAtMs !== null ? new Date(Number(selected.startedAtMs)).toLocaleString("ja-JP") : "未開始";
        summary.textContent = `${recordingStatusLabels[selected.status] || displayValue(selected.status)} · 開始 ${started} · キープ ${selected.keep ? "オン" : "オフ"}${selected.currentFile ? ` · ${displayRecordingPath(selected.currentFile)}` : ""}${selected.error ? ` · ${selected.error}` : ""}`;
      }
    }
    const keep = byId("recording-keep");
    if (keep) keep.textContent = selected?.keep ? "キープ: オン" : "キープ: オフ";
    updateRecordingControls();
  }

  function updateRecordingControls() {
    const jobs = Array.isArray(state.snapshot?.recordings) ? state.snapshot.recordings : [];
    const index = jobs.findIndex((job) => job.id === state.recordingSelectionId);
    const job = index < 0 ? null : jobs[index];
    const busy = state.busy || state.recordingBusy;
    const active = recordingIsActive(job);
    const set = (id, disabled) => { const control = byId(id); if (control) control.disabled = Boolean(disabled); };
    set("recording-start", busy || !state.snapshot?.running || !job || Boolean(active));
    set("recording-stop", busy || !active);
    set("recording-reconnect", busy || !state.snapshot?.running || !job);
    set("recording-keep", busy || !job);
    set("recording-up", busy || !job || index <= 0);
    set("recording-down", busy || !job || index >= jobs.length - 1);
    set("recording-remove", busy || !job);
    set("open-recording-folder", busy);
    const selectedChannel = currentSelectedDirectoryChannel();
    set("selected-record", busy || state.ypBusy || !state.snapshot?.running || !selectedChannel?.yellowPage || !isPlayableYpChannel(selectedChannel));
  }

  async function controlRecording(action) {
    const job = (state.snapshot?.recordings || []).find((entry) => entry.id === state.recordingSelectionId);
    if (!job || state.recordingBusy || state.busy) return false;
    state.recordingBusy = true; updateRecordingControls();
    try {
      const snapshot = await invoke("control_recording", { jobId: String(job.id), action });
      renderSnapshot(snapshot);
      return true;
    } catch (error) { setNotice(`録画を操作できませんでした: ${errorText(error)}`, true); return false; }
    finally { state.recordingBusy = false; updateRecordingControls(); }
  }

  async function enqueueSelectedRecording(channel = currentSelectedDirectoryChannel()) {
    if (!channel?.channelId || !channel.yellowPage || !isPlayableYpChannel(channel) || state.recordingBusy || state.busy) return;
    state.recordingBusy = true; updateRecordingControls();
    try {
      const jobId = await invoke("enqueue_recording", { channelId: String(channel.channelId), yellowPage: String(channel.yellowPage), keep: false });
      state.recordingSelectionId = String(jobId);
      const snapshot = await invoke("snapshot");
      renderSnapshot(snapshot);
      setNotice(`${displayValue(channel.name)} を録画一覧に追加しました。`);
      await requestView("recordings");
    } catch (error) { setNotice(`録画を追加できませんでした: ${errorText(error)}`, true); }
    finally { state.recordingBusy = false; updateRecordingControls(); }
  }

  async function openRecordingFolder() {
    if (state.recordingBusy || state.busy) return;
    state.recordingBusy = true; updateRecordingControls();
    try { await invoke("open_recording_folder", { jobId: state.recordingSelectionId || null }); }
    catch (error) { setNotice(`保存先を開けませんでした: ${errorText(error)}`, true); }
    finally { state.recordingBusy = false; updateRecordingControls(); }
  }

  function filteredLogs() {
    const entries = Array.isArray(state.snapshot?.logs) ? state.snapshot.logs : [];
    const query = state.logQuery.normalize("NFKC").toLocaleLowerCase("ja-JP").trim();
    const levelFilter = byId("log-level-filter")?.value || "all";
    const filterRank = logRanks[levelFilter];
    return entries.filter((entry) => {
      const rank = logRanks[String(entry.level || "info").toLowerCase()];
      if (levelFilter !== "all" && (rank === undefined || rank > filterRank)) return false;
      if (!query) return true;
      return `${entry.target || ""} ${entry.message || ""}`.normalize("NFKC").toLocaleLowerCase("ja-JP").includes(query);
    });
  }

  function renderLogs() {
    const visible = filteredLogs();
    const copyButton = byId("copy-logs");
    if (copyButton) copyButton.disabled = visible.length === 0;
    if (currentView() !== "logs") return;
    const root = byId("logs-list");
    if (!root) return;
    const fingerprint = JSON.stringify([visible, state.logQuery, byId("log-level-filter")?.value || "all", Boolean(byId("log-wrap")?.checked)]);
    if (fingerprint === state.logFingerprint) return;
    state.logFingerprint = fingerprint;
    root.replaceChildren();
    visible.forEach((entry) => {
      const row = document.createElement("article");
      row.className = "log-entry";
      appendText(row, "time", "log-time", Number.isFinite(Number(entry.timestampMs)) ? new Date(Number(entry.timestampMs)).toLocaleTimeString("ja-JP") : "時刻不明");
      appendText(row, "span", `log-level level-${String(entry.level || "info").toLowerCase()}`, displayValue(entry.level));
      appendText(row, "span", "log-target", displayValue(entry.target));
      appendText(row, "p", "log-message", displayValue(entry.message));
      root.append(row);
    });
    root.classList.toggle("no-wrap", !Boolean(byId("log-wrap")?.checked));
    if (!visible.length) appendText(root, "p", "empty", "該当するログはありません。");
  }

  function renderProbe(probe) {
    const status = byId("probe-status");
    if (status) {
      status.className = `status-pill ${probe ? (probe.passed ? "status-live" : "status-fail") : "status-idle"}`;
      status.textContent = probe ? (probe.passed ? "成功" : "失敗") : "未実行";
    }
    text("probe-bytes", probe ? formatBytes(probe.bytes) : "—");
    text("probe-elapsed", probe ? `${Number(probe.elapsedMs || 0).toFixed(1)} ms` : "—");
  }

  function renderSnapshot(snapshot, { initial = false } = {}) {
    if (!snapshot || typeof snapshot !== "object") throw new Error("snapshotの形式が正しくありません。");
    const preserveClearedKeys = snapshot.channels === state.snapshot?.channels;
    const cleared = filterClearedConnectionSnapshot(snapshot, state.clearedConnectionKeys, preserveClearedKeys);
    snapshot = cleared.snapshot;
    state.clearedConnectionKeys = cleared.clearedConnectionKeys;
    state.snapshot = snapshot;
    restoreLastBroadcastOnce(snapshot);
    state.settingsView?.updateSnapshot(snapshot);
    const running = Boolean(snapshot.running);
    const dot = byId("state-dot");
    if (dot) dot.className = `state-dot ${running ? "is-running" : "is-stopped"}`;
    const backendLabel = snapshot.backendLabel || (snapshot.backendKind === "csharp" ? "PeerCastStation (C#)" : "Peercast RT");
    text("state-label", `${backendLabel} ${running ? "稼働中" : "停止中"}`);
    text("endpoint", snapshot.endpoint || "停止中");
    text("uptime", running ? formatAppUptime(snapshot.uptimeSecs) : "停止中");
    text("active-tasks", String(Number(snapshot.activeTasks) || 0));
    text("last-updated", `最終更新 ${state.ypLastUpdatedAt === null ? "—" : new Date(state.ypLastUpdatedAt).toLocaleTimeString("ja-JP")}`);
    text("limit-directs", String(Number(snapshot.limits?.maxDirects) || 0));
    text("limit-relays", String(Number(snapshot.limits?.maxRelays) || 0));
    text("limit-rate", `${Number(snapshot.limits?.maxUpstreamRate) || 0} kbps`);
    const sourcesChanged = updateYpSources();
    renderChannels(snapshot);
    renderConnections(snapshot);
    if (currentView() === "channels") text("yp-count", `${broadcastingChannels(snapshot).length} 件の配信`);
    if (currentView() === "connections") text("yp-count", `${allRuntimeChannels(snapshot).length} 件の接続チャンネル`);
    if (currentView() === "logs") renderLogs();
    renderProbe(snapshot.lastProbe || null);
    const csharpBackend = snapshot.backendKind === "csharp";
    updateBroadcastBackendFields();
    const probeButton = byId("probe-button");
    if (probeButton) {
      probeButton.disabled = state.busy || csharpBackend;
      probeButton.title = csharpBackend ? "C#版へテストデータを送らないため、この診断は利用できません" : "";
    }
    const backendActionsNote = byId("backend-actions-note");
    if (backendActionsNote) {
      backendActionsNote.hidden = !csharpBackend;
      backendActionsNote.textContent = csharpBackend
        ? "C#版へテストデータを送らないため、送受信診断は無効です。接続上限はC#版側で設定してください。"
        : "";
    }
    renderRecordings(snapshot);
    const directoryFlagsChanged = applyDirectoryPreferences(snapshot.desktop?.directory || {});
    if (currentView() === "recordings") text("yp-count", `${Array.isArray(snapshot.recordings) ? snapshot.recordings.length : 0} 件の録画`);
    if (state.detailKind === "channel") renderSelectedDetail();
    else updateConnectionSummary(state.detailKind === "yp" ? runtimeChannelFor(currentYpChannel()?.channelId) : null);
    updateDetailActions(currentSelectedDirectoryChannel());
    updateYpControls();
    renderYpSourcesAndMaybeRefresh(sourcesChanged || directoryFlagsChanged);
    if (initial && !state.ypStarted) {
      state.ypStarted = true;
      const activeView = document.querySelector("[data-view].active")?.dataset.view || "yp";
      activateView(activeView);
      activateDetailTab(state.detailTab);
      fetchYpChannels(false);
    }
  }

  async function action(label, command, args) {
    if (state.busy) return;
    setBusy(true);
    setNotice(`${label}を実行中…`, false, { persistent: true });
    let succeeded = false;
    try {
      const result = await invoke(command, args);
      renderSnapshot(result);
      setNotice(`${label}が完了しました。`);
      succeeded = true;
      return true;
    } catch (error) {
      setNotice(`${label}に失敗しました: ${errorText(error)}`, true);
      return false;
    } finally {
      setBusy(false);
      if (command === "restart_backend" && succeeded) refreshYpAfterRuntimeChange();
      else flushPendingYpRefresh();
    }
  }

  async function refresh(showMessage = false) {
    if (state.busy) return;
    if (state.refreshing) {
      if (showMessage) {
        state.manualRefreshing = true;
        updateRefreshControls();
      }
      return;
    }
    state.refreshing = true;
    state.manualRefreshing = showMessage;
    const startedAt = performance.now();
    updateRefreshControls();
    try {
      const logGeneration = state.logClearGeneration;
      const connectionGeneration = state.connectionClearGeneration;
      let snapshot = await invoke("snapshot");
      if (logGeneration !== state.logClearGeneration) snapshot.logs = state.snapshot?.logs || [];
      if (connectionGeneration !== state.connectionClearGeneration) {
        snapshot = { ...snapshot, channels: state.snapshot?.channels || [] };
      }
      renderSnapshot(snapshot, { initial: !state.snapshot });
      if (state.manualRefreshing) setNotice("状態を更新しました。");
    } catch (error) {
      setNotice(`状態の取得に失敗しました: ${errorText(error)}`, true);
    } finally {
      if (state.manualRefreshing) await finishRefreshFeedback(startedAt);
      state.manualRefreshing = false;
      state.refreshing = false;
      updateRefreshControls();
    }
  }

  async function playYpChannel(channel) {
    if (!state.snapshot?.running || !channel || state.busy || state.ypBusy || !isPlayableYpChannel(channel)) return;
    state.ypBusy = true;
    updateYpControls();
    setNotice(`${displayValue(channel.name)} の配信元に接続しています…`, false, { persistent: true });
    try {
      await invoke("play_yp_channel", { channelId: String(channel.channelId), yellowPage: String(channel.yellowPage || "") });
      setNotice("設定したプレイヤーを起動しました。");
      await refresh(false);
    } catch (error) {
      setNotice(`再生を開始できませんでした: ${errorText(error)}`, true);
    } finally {
      state.ypBusy = false;
      updateYpControls();
      flushPendingYpRefresh();
    }
  }

  async function copyText(value, label) {
    if (!value) {
      setNotice(`${label}を利用できません。`, true);
      return;
    }
    try {
      await navigator.clipboard.writeText(value);
      setNotice(`${label}をコピーしました。`);
    } catch (error) {
      setNotice(`${label}をコピーできませんでした: ${errorText(error)}`, true);
    }
  }

  async function copyVisibleLogs() {
    const value = filteredLogs().map((entry) => {
      const timestamp = Number(entry.timestampMs);
      const date = Number.isFinite(timestamp) ? new Date(timestamp).toLocaleString("ja-JP") : "時刻不明";
      return [date, displayValue(entry.level), displayValue(entry.target), displayValue(entry.message)].join("\t");
    }).join("\n");
    if (value) await copyText(value, "表示中のログ");
  }

  function selectChannel(id) {
    state.channelSelectionId = id;
    state.detailKind = "channel";
    state.detailKey = id;
    renderChannels(state.snapshot || { channels: [] });
    renderSelectedDetail();
  }

  function bind(id, event, handler) {
    const element = byId(id);
    if (element) element.addEventListener(event, handler);
  }

  async function toggleMainWindowMaximize() {
    try {
      const maximized = await invoke("toggle_main_window_maximize");
      const button = byId("window-maximize");
      const icon = button?.querySelector(".icon");
      if (icon) icon.className = `icon ${maximized ? "icon-copy" : "icon-square"}`;
      if (button) {
        button.setAttribute("aria-label", maximized ? "元のサイズに戻す" : "最大化");
        button.title = maximized ? "元のサイズに戻す" : "最大化";
      }
    } catch (error) {
      setNotice(`ウィンドウサイズを変更できませんでした: ${errorText(error)}`, true);
    }
  }

  document.querySelectorAll("[data-view]").forEach((button) => {
    button.addEventListener("click", () => requestView(button.dataset.view));
  });
  document.querySelectorAll("[data-detail-tab]").forEach((button) => {
    button.addEventListener("click", () => activateDetailTab(button.dataset.detailTab));
    button.addEventListener("keydown", (event) => {
      const tabs = [...document.querySelectorAll("[data-detail-tab]")];
      const index = tabs.indexOf(button);
      let next = index;
      if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
      else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
      else if (event.key === "Home") next = 0;
      else if (event.key === "End") next = tabs.length - 1;
      else return;
      event.preventDefault();
      tabs[next].focus();
      activateDetailTab(tabs[next].dataset.detailTab);
    });
  });
  document.querySelectorAll("[data-connection-tab]").forEach((button) => {
    button.addEventListener("click", () => activateConnectionTab(button.dataset.connectionTab));
    button.addEventListener("keydown", (event) => {
      const tabs = [...document.querySelectorAll("[data-connection-tab]")];
      const index = tabs.indexOf(button);
      let next = index;
      if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
      else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
      else if (event.key === "Home") next = 0;
      else if (event.key === "End") next = tabs.length - 1;
      else return;
      event.preventDefault();
      tabs[next].focus();
      activateConnectionTab(tabs[next].dataset.connectionTab);
    });
  });
  bind("about-button", "click", () => {
    byId("about-dialog")?.showModal();
    void invoke("update_status").then((status) => {
      text("about-version", status?.currentVersion ? `バージョン ${status.currentVersion}` : "バージョン情報を取得できませんでした");
    }).catch(() => text("about-version", "バージョン情報を取得できませんでした"));
  });
  bind("close-about", "click", () => byId("about-dialog")?.close());
  bind("about-dialog", "click", (event) => {
    const dialog = byId("about-dialog");
    if (event.target !== dialog) return;
    const bounds = dialog.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) dialog.close();
  });
  bind("quit-button", "click", () => closeMainWindow());
  bind("window-minimize", "click", async () => {
    try { await invoke("minimize_main_window"); }
    catch (error) { setNotice(`ウィンドウを最小化できませんでした: ${errorText(error)}`, true); }
  });
  bind("window-maximize", "click", toggleMainWindowMaximize);
  bind("window-close", "click", () => closeMainWindow());
  bind("titlebar", "mousedown", (event) => {
    if (event.button !== 0 || event.target.closest(".window-controls")) return;
    event.preventDefault();
    if (event.detail === 2) {
      void toggleMainWindowMaximize();
      return;
    }
    invoke("start_main_window_drag").catch((error) => {
      setNotice(`ウィンドウを移動できませんでした: ${errorText(error)}`, true);
    });
  });
  bind("refresh-button", "click", () => refresh(true));
  bind("probe-button", "click", () => action("ローカル送受信テスト", "run_probe"));
  bind("restart-button", "click", () => action("バックエンド再起動", "restart_backend"));
  bind("stop-button", "click", () => action("バックエンド停止", "stop_backend"));
  bind("header-broadcast", "click", async () => {
    if (currentView() !== "channels" && !(await requestView("channels"))) return;
    const form = byId("broadcast-form");
    if (form) form.open = true;
    byId("broadcast-name")?.focus();
  });
  bind("broadcast-start-form", "submit", startBroadcast);
  const broadcastForm = byId("broadcast-start-form");
  if (broadcastForm) {
    broadcastForm.addEventListener("input", () => { state.broadcastFormDirty = true; });
    broadcastForm.addEventListener("change", () => { state.broadcastFormDirty = true; });
  }
  bind("broadcast-source-mode", "change", updateBroadcastSourceFields);
  bind("broadcast-stop-button", "click", () => stopBroadcast(state.channelSelectionId));
  updateBroadcastSourceFields();

  bind("yp-search", "input", () => {
    state.ypQuery = byId("yp-search").value;
    renderYpDirectory();
  });
  bind("yp-source", "change", () => {
    state.ypSource = byId("yp-source").value || "peercast";
    renderYpDirectory();
  });
  window.addEventListener("youtube-snapshot-updated", (event) => {
    const snapshot = event.detail;
    if (!snapshot || !Array.isArray(snapshot.channels)) return;
    state.youtubeRevision += 1;
    state.youtubeSnapshot = snapshot;
    if (currentView() === "yp") renderYpDirectory();
  });
  window.addEventListener("twitch-snapshot-updated", (event) => {
    const snapshot = event.detail;
    if (!snapshot || !Array.isArray(snapshot.channels)) return;
    state.twitchRevision += 1;
    state.twitchSnapshot = snapshot;
    if (currentView() === "yp") renderYpDirectory();
  });
  window.addEventListener("youtube-snapshot-requested", () => {
    if (state.youtubeSnapshot) window.dispatchEvent(new CustomEvent("youtube-snapshot-polled", { detail: state.youtubeSnapshot }));
  });
  window.addEventListener("twitch-snapshot-requested", () => {
    if (state.twitchSnapshot) window.dispatchEvent(new CustomEvent("twitch-snapshot-polled", { detail: state.twitchSnapshot }));
  });
  bind("yp-scope", "change", () => { state.ypScope = byId("yp-scope").value || "normal"; renderYpDirectory(); });
  bind("yp-refresh", "click", () => fetchYpChannels(true, true));
  bind("yp-refresh-save", "click", saveDirectoryPreferences);
  document.querySelectorAll('input[name="yp-refresh-mode"]').forEach((input) => input.addEventListener("change", () => { state.ypDirectoryDirty = true; }));
  bind("yp-refresh-interval", "input", () => { state.ypDirectoryDirty = true; });

  function handleYpResultsClick(event) {
    const youtubePlay = event.target.closest("[data-youtube-play]");
    if (youtubePlay) {
      if (!livePlaybackReady() || youtubePlay.disabled) return;
      void launchLivePlayback("youtube_open", { videoId: youtubePlay.dataset.youtubePlay }).catch((error) => {
        text("yp-status", `${livePlayerName()}で再生できませんでした: ${errorText(error)}`);
      });
      return;
    }
    const youtubeBrowser = event.target.closest("[data-youtube-browser]");
    if (youtubeBrowser) {
      void invoke("youtube_open_browser", { videoId: youtubeBrowser.dataset.youtubeBrowser }).catch((error) => {
        text("yp-status", `ブラウザで開けませんでした: ${errorText(error)}`);
      });
      return;
    }
    const twitchPlay = event.target.closest("[data-twitch-play]");
    if (twitchPlay) {
      if (!livePlaybackReady() || twitchPlay.disabled) return;
      void launchLivePlayback("twitch_open", { streamId: twitchPlay.dataset.twitchPlay }).catch((error) => {
        text("yp-status", `${livePlayerName()}で再生できませんでした: ${errorText(error)}`);
      });
      return;
    }
    const twitchBrowser = event.target.closest("[data-twitch-browser]");
    if (twitchBrowser) {
      void invoke("twitch_open_browser", { streamId: twitchBrowser.dataset.twitchBrowser }).catch((error) => {
        text("yp-status", `ブラウザで開けませんでした: ${errorText(error)}`);
      });
      return;
    }
    const row = event.target.closest(".yp-row");
    if (!row) return;
    const channel = state.ypChannels[Number(row.dataset.ypIndex)];
    if (!channel) return;
    if (event.target.closest(".yp-play-button")) {
      event.stopPropagation();
      playYpChannel(channel);
    } else {
      selectYpChannel(channel);
    }
  }

  function handleYpResultsContextMenu(event) {
    if (isEditableContextTarget(event.target)) return;
    const providerCard = event.target.closest(".youtube-stream-card[data-provider]");
    if (providerCard) {
      const provider = providerCard.dataset.provider;
      const streamId = providerCard.dataset.streamId;
      const menu = byId("provider-context-menu");
      if (!menu || !streamId || (provider !== "youtube" && provider !== "twitch")) return;
      event.preventDefault();
      closeAllContextMenus();
      state.providerContextStream = { provider, streamId };
      byId("provider-context-play").disabled = !livePlaybackReady() || state.busy;
      placeContextMenu(menu, event.clientX, event.clientY);
      byId("provider-context-play")?.focus();
      return;
    }
    const row = event.target.closest(".yp-row");
    if (!row) return;
    const channel = state.ypChannels[Number(row.dataset.ypIndex)];
    const menu = byId("yp-context-menu");
    if (!channel?.name || !menu) return;
    event.preventDefault();
    closeAllContextMenus();
    selectYpChannel(channel);
    state.ypContextChannel = channel;
    const canPlay = Boolean(state.snapshot?.running && !state.busy && !state.ypBusy && isPlayableYpChannel(channel) && state.snapshot?.playerAvailable !== false);
    const canRecord = Boolean(state.snapshot?.running && !state.busy && !state.recordingBusy && !state.ypBusy && channel.yellowPage && isPlayableYpChannel(channel));
    byId("yp-context-play").disabled = !canPlay;
    byId("yp-context-record").disabled = !canRecord;
    const directFavorite = directFavoriteRuleIndex(state.snapshot?.desktop?.channelRules, channel.name) >= 0;
    text("yp-context-favorite-label", directFavorite ? "お気に入りから外す" : "お気に入りに入れる");
    placeContextMenu(menu, event.clientX, event.clientY);
    byId(canPlay ? "yp-context-play" : canRecord ? "yp-context-record" : "yp-context-favorite")?.focus();
  }

  const ypRoot = byId("yp-results");
  if (ypRoot) ypRoot.addEventListener("click", handleYpResultsClick);
  function closeYpContextMenu() {
    const menu = byId("yp-context-menu");
    if (menu) menu.hidden = true;
    state.ypContextChannel = null;
  }
  function closeAllContextMenus() {
    ["yp-context-menu", "provider-context-menu", "runtime-context-menu", "recording-context-menu"].forEach((id) => { const menu = byId(id); if (menu) menu.hidden = true; });
    state.ypContextChannel = null;
    state.providerContextStream = null;
    state.runtimeContextKey = null;
    state.recordingContextId = null;
  }
  function isEditableContextTarget(target) {
    return Boolean(target?.closest?.("input, textarea, select, [contenteditable='true'], [contenteditable='']"));
  }
  function placeContextMenu(menu, x, y) {
    menu.hidden = false;
    const rect = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
  }
  if (ypRoot) ypRoot.addEventListener("contextmenu", handleYpResultsContextMenu);
  async function runProviderContextAction(action) {
    const target = state.providerContextStream;
    closeAllContextMenus();
    if (!target) return;
    if (action === "manage") {
      const moved = await requestView("settings");
      if (moved || currentView() === "settings") state.settingsView?.openYellowPagesSubtab(target.provider);
      return;
    }
    if (action === "play" && !livePlaybackReady()) return;
    const command = target.provider === "twitch"
      ? action === "browser" ? "twitch_open_browser" : "twitch_open"
      : action === "browser" ? "youtube_open_browser" : "youtube_open";
    const args = target.provider === "twitch" ? { streamId: target.streamId } : { videoId: target.streamId };
    try {
      if (action === "browser") await invoke(command, args);
      else await launchLivePlayback(command, args);
    }
    catch (error) { setNotice(`${action === "browser" ? "ブラウザで開けませんでした" : `${livePlayerName()}で再生できませんでした`}: ${errorText(error)}`, true); }
  }
  bind("provider-context-play", "click", () => { void runProviderContextAction("play"); });
  bind("provider-context-browser", "click", () => { void runProviderContextAction("browser"); });
  bind("provider-context-manage", "click", () => { void runProviderContextAction("manage"); });
  async function setContextChannelPreference(kind) {
    const channel = state.ypContextChannel;
    closeYpContextMenu();
    if (!channel?.name || state.busy || state.ypBusy) return;
    const current = state.snapshot || {};
    const desktop = current.desktop || {};
    const updatedRules = kind === "favorite"
      ? removeDirectFavoriteRule(desktop.channelRules, channel.name)
      : null;
    state.ypBusy = true;
    updateYpControls();
    try {
      const snapshot = updatedRules
        ? await invoke("apply_preferences", {
          limits: current.limits || {},
          yellowPages: current.yellowPages || [],
          desktop: { ...desktop, channelRules: updatedRules },
        })
        : await invoke("set_channel_preference", { channelName: String(channel.name), kind });
      renderSnapshot(snapshot);
      const channels = await invoke("get_yp_channels", { refresh: false });
      state.ypChannels = Array.isArray(channels) ? channels : [];
      renderYpDirectory();
      setNotice(updatedRules
        ? `${displayValue(channel.name)} の個別登録を解除しました。ほかの条件は引き続き適用されます。`
        : `${displayValue(channel.name)} を${kind === "ng" ? "NG" : "お気に入り"}に入れました。`);
    } catch (error) {
      setNotice(`チャンネルを登録できませんでした: ${errorText(error)}`, true);
    } finally {
      state.ypBusy = false;
      updateYpControls();
    }
  }
  function playYpContextChannel() {
    const channel = currentYpContextChannel();
    closeYpContextMenu();
    if (channel) void playYpChannel(channel);
  }
  function recordYpContextChannel() {
    const channel = currentYpContextChannel();
    closeYpContextMenu();
    if (channel) void enqueueSelectedRecording(channel);
  }
  bind("yp-context-play", "click", playYpContextChannel);
  bind("yp-context-record", "click", recordYpContextChannel);
  bind("yp-context-favorite", "click", () => { void setContextChannelPreference("favorite"); });
  bind("yp-context-ng", "click", () => { void setContextChannelPreference("ng"); });
  document.addEventListener("click", (event) => {
    if (!event.target.closest(".context-menu")) closeAllContextMenus();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeAllContextMenus();
  });
  document.addEventListener("scroll", closeAllContextMenus, true);
  window.addEventListener("blur", closeAllContextMenus);
  function suppressNativeContextMenu(event) {
    if (!isEditableContextTarget(event.target)) event.preventDefault();
  }
  document.addEventListener("contextmenu", suppressNativeContextMenu);
  const channelsRoot = byId("channels-list");
  if (channelsRoot) channelsRoot.addEventListener("click", (event) => {
    const row = event.target.closest("[data-channel-id]");
    if (row) selectChannel(row.dataset.channelId);
  });
  const connectionList = byId("connections-channel-list");
  if (connectionList) connectionList.addEventListener("click", (event) => {
    const row = event.target.closest("[data-connection-channel-id]");
    if (!row) return;
    state.connectionSelectionId = row.dataset.connectionChannelId;
    renderConnections(state.snapshot || { channels: [] });
  });
  async function controlRuntimeChannel(action) {
    const targetKey = state.runtimeContextKey;
    const channel = allRuntimeChannels(state.snapshot).find((entry) => connectionChannelKey(entry) === String(targetKey));
    const allowed = action === "reconnect" ? channel?.canReconnect === true : channel?.canDisconnect === true;
    closeAllContextMenus();
    if (!channel || !allowed || state.recordingBusy || state.busy || state.clearingLogs) return false;
    state.recordingBusy = true;
    updateRecordingControls();
    try {
      const snapshot = await invoke("control_relay_channel", { channelId: channel.id, sessionId: channel.sessionId, action });
      renderSnapshot(snapshot);
      return true;
    } catch (error) { setNotice(`接続を操作できませんでした: ${errorText(error)}`, true); return false; }
    finally { state.recordingBusy = false; updateRecordingControls(); }
  }
  const connectionMenu = byId("runtime-context-menu");
  function openRuntimeContextMenu(event) {
    if (isEditableContextTarget(event.target)) return;
    const row = event.target.closest("[data-connection-channel-id]");
    if (row) state.connectionSelectionId = row.dataset.connectionChannelId;
    const channel = allRuntimeChannels(state.snapshot).find((entry) => connectionChannelKey(entry) === String(state.connectionSelectionId));
    if (!connectionMenu) return;
    event.preventDefault(); closeAllContextMenus();
    state.runtimeContextKey = channel ? connectionChannelKey(channel) : null;
    renderConnections(state.snapshot || { channels: [] });
    byId("runtime-context-reconnect").disabled = channel?.canReconnect !== true || state.recordingBusy || state.busy || state.clearingLogs;
    byId("runtime-context-disconnect").disabled = channel?.canDisconnect !== true || state.recordingBusy || state.busy || state.clearingLogs;
    byId("runtime-context-clear").disabled = Boolean(state.clearingLogs || state.busy || state.recordingBusy);
    placeContextMenu(connectionMenu, event.clientX, event.clientY);
    const firstAction = ["runtime-context-reconnect", "runtime-context-disconnect", "runtime-context-clear"].map(byId).find((button) => !button.disabled);
    firstAction?.focus();
  }
  async function clearMessagesAndLogs() {
    closeAllContextMenus();
    if (state.clearingLogs || state.busy || state.recordingBusy) return false;
    state.clearingLogs = true;
    byId("runtime-context-clear").disabled = true;
    try {
      await invoke("clear_logs");
      state.logClearGeneration++;
      state.connectionClearGeneration++;
      if (state.snapshot) state.snapshot.logs = [];
      if (state.snapshot) {
        const cleared = clearDisconnectedConnections(state.snapshot, state.clearedConnectionKeys);
        state.snapshot = cleared.snapshot;
        state.clearedConnectionKeys = cleared.clearedConnectionKeys;
        renderConnections(state.snapshot);
        if (currentView() === "connections") text("yp-count", `${allRuntimeChannels(state.snapshot).length} 件の接続チャンネル`);
      }
      state.logFingerprint = "";
      setNotice("");
      renderLogs();
      return true;
    } catch (error) {
      setNotice(`クリアできませんでした: ${errorText(error)}`, true);
      return false;
    } finally {
      state.clearingLogs = false;
      byId("runtime-context-clear").disabled = Boolean(state.busy || state.recordingBusy);
    }
  }
  byId("view-connections")?.addEventListener("contextmenu", openRuntimeContextMenu);
  byId("notice")?.addEventListener("contextmenu", (event) => {
    if (currentView() === "connections") openRuntimeContextMenu(event);
  });
  byId("runtime-context-reconnect")?.addEventListener("click", () => { void controlRuntimeChannel("reconnect"); });
  byId("runtime-context-disconnect")?.addEventListener("click", () => { void controlRuntimeChannel("disconnect"); });
  byId("runtime-context-clear")?.addEventListener("click", () => { void clearMessagesAndLogs(); });
  bind("selected-record", "click", () => { void enqueueSelectedRecording(); });
  async function addSelectedChannelRule(kind) {
    const channel = currentSelectedDirectoryChannel();
    if (!channel?.name) return;
    const moved = await requestView("settings");
    if (moved || currentView() === "settings") state.settingsView?.addChannelRule(channel, kind);
  }
  bind("add-favorite-rule", "click", () => { void addSelectedChannelRule("favorite"); });
  bind("add-ng-rule", "click", () => { void addSelectedChannelRule("ng"); });
  bind("open-recording-folder", "click", () => { void openRecordingFolder(); });
  bind("recording-start", "click", () => { void controlRecording("start"); });
  bind("recording-stop", "click", () => { void controlRecording("stop"); });
  bind("recording-reconnect", "click", () => { void controlRecording("reconnect"); });
  bind("recording-keep", "click", () => { void controlRecording("toggle_keep"); });
  bind("recording-up", "click", () => { void controlRecording("up"); });
  bind("recording-down", "click", () => { void controlRecording("down"); });
  bind("recording-remove", "click", () => {
    if (!state.recordingSelectionId || state.recordingBusy || state.busy) return;
    state.pendingRecordingRemoval = state.recordingSelectionId;
    try { byId("remove-recording-dialog").showModal(); }
    catch (error) { state.pendingRecordingRemoval = null; setNotice(`確認画面を表示できませんでした: ${errorText(error)}`, true); }
  });
  bind("cancel-remove-recording", "click", () => byId("remove-recording-dialog")?.close());
  bind("confirm-remove-recording", "click", () => {
    const id = state.pendingRecordingRemoval; state.pendingRecordingRemoval = null;
    if (!id) return;
    state.recordingSelectionId = id;
    byId("remove-recording-dialog")?.close();
    void controlRecording("remove");
  });
  bind("remove-recording-dialog", "close", () => { state.pendingRecordingRemoval = null; });
  const recordingsRoot = byId("recordings-list");
  async function recordingFileAction(action) {
    const targetId = state.recordingContextId;
    const job = (state.snapshot?.recordings || []).find((entry) => String(entry.id) === String(targetId));
    closeAllContextMenus();
    if (!canUseRecordingFile(job, action, state.recordingBusy || state.busy)) return false;
    state.recordingBusy = true; updateRecordingControls();
    try {
      await invoke("recording_file_action", { jobId: String(job.id), action });
      await refresh();
      return true;
    } catch (error) { setNotice(`録画ファイルを操作できませんでした: ${errorText(error)}`, true); return false; }
    finally { state.recordingBusy = false; updateRecordingControls(); }
  }
  function openRecordingContextMenu(event) {
    if (isEditableContextTarget(event.target)) return;
    const row = event.target.closest("tr[data-recording-id]");
    if (row) { state.recordingSelectionId = row.dataset.recordingId; renderRecordings(state.snapshot || { recordings: [] }); }
    const job = (state.snapshot?.recordings || []).find((entry) => String(entry.id) === String(state.recordingSelectionId));
    const menu = byId("recording-context-menu");
    if (!job || !menu) return;
    event.preventDefault(); closeAllContextMenus();
    state.recordingContextId = String(job.id);
    for (const action of ["play", "reveal", "trash"]) byId(`recording-context-${action}`).disabled = !canUseRecordingFile(job, action, state.recordingBusy || state.busy);
    placeContextMenu(menu, event.clientX, event.clientY);
    (canUseRecordingFile(job, "play", state.recordingBusy || state.busy) ? byId("recording-context-play") : byId("recording-context-reveal"))?.focus();
  }
  recordingsRoot?.addEventListener("contextmenu", openRecordingContextMenu);
  byId("recording-selection-summary")?.addEventListener("contextmenu", openRecordingContextMenu);
  byId("recording-context-play")?.addEventListener("click", () => { void recordingFileAction("play"); });
  byId("recording-context-reveal")?.addEventListener("click", () => { void recordingFileAction("reveal"); });
  byId("recording-context-trash")?.addEventListener("click", () => { void recordingFileAction("trash"); });
  if (recordingsRoot) {
    recordingsRoot.addEventListener("click", (event) => {
      const row = event.target.closest("tr[data-recording-id]"); if (!row) return;
      state.recordingSelectionId = row.dataset.recordingId; renderRecordings(state.snapshot || { recordings: [] });
    });
    recordingsRoot.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      const row = event.target.closest("tr[data-recording-id]"); if (!row) return;
      event.preventDefault(); state.recordingSelectionId = row.dataset.recordingId; renderRecordings(state.snapshot || { recordings: [] });
    });
  }
  bind("copy-contact", "click", () => copyText(safeHttpUrl(currentSelectedDirectoryChannel()?.contactUrl), "コンタクトURL"));
  bind("copy-stream", "click", () => {
    const channel = state.detailKind === "yp" ? currentYpChannel() : runtimeChannelFor(state.channelSelectionId);
    const id = channel?.channelId || state.channelSelectionId;
    const endpoint = String(state.snapshot?.endpoint || "").replace(/\/$/, "");
    const localChannel = runtimeChannelFor(id);
    copyText(state.snapshot?.running && localChannel && endpoint ? `${endpoint}/stream/${encodeURIComponent(id)}` : "", "ストリームURL");
  });
  bind("log-search", "input", () => { state.logQuery = byId("log-search").value; renderLogs(); });
  bind("log-level-filter", "change", renderLogs);
  bind("log-wrap", "change", renderLogs);
  bind("copy-logs", "click", copyVisibleLogs);

  // Resolve native capabilities before any optional-service or updater request.
  // Fail closed if the platform command is unavailable; PeerCast remains usable.
  async function startApplication() {
    try {
      state.capabilitiesReady = invoke("platform_capabilities").catch(() => null);
      const capabilities = await state.capabilitiesReady;
      if (capabilities && typeof capabilities.platform === "string") state.capabilities = capabilities;
      else throw new Error("プラットフォーム情報がありません");
    } catch (error) {
      setNotice(`プラットフォーム情報を取得できません。ライブサービスと自動更新は無効です: ${errorText(error)}`, true);
    }
    applyLiveDependencies(state.capabilities.liveDependencies);
    if (state.capabilities.platform === "linux") {
      text("about-main-platform", "配信の視聴とP2Pリレーを行うLinux開発版です。");
      text("about-main-player", "ライブ再生: 外部mpv（別途配置が必要）");
    }
    await refresh();
    scheduleApplicationUpdateChecks();
    window.setInterval(() => refresh(false), 1000);
    if (state.capabilities.liveServices === true) {
      void fetchYoutubeStatus();
      void fetchTwitchStatus();
      window.setInterval(() => {
        if (document.visibilityState !== "hidden") {
          void fetchLiveDependencies();
          void fetchYoutubeStatus();
          void fetchTwitchStatus();
        }
      }, YOUTUBE_STATUS_POLL_MS);
    }
  }
  void startApplication();
})();
