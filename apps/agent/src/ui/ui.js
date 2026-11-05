(() => {
  "use strict";

  const token = document.querySelector('meta[name="token"]').content;
  const state = {
    scope: "all",
    provider: "x",
    items: [],
    selected: null,
    approvals: [],
    history: [],
    filtered: [],
    filter: "",
    cursor: 0,
    cache: new Map(),
    threadCache: new Map(),
    tab: "thread",
    busy: false,
    pendingApproval: null,
  };

  const CACHE_TTL_MS = 5 * 60 * 1000;

  const scopeTitles = {
    all: "All",
    feed: "Feed",
    mentions: "Mentions",
    notifications: "Notifications",
  };

  const params = new URLSearchParams(window.location.search);
  const $ = (id) => document.getElementById(id);

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  function setStatus(text, busy = false) {
    const node = $("status");
    node.textContent = text;
    node.classList.toggle("is-busy", busy);
  }

  let toastTimer = null;
  function toast(message, isError = false) {
    const node = $("toast");
    node.textContent = message;
    node.classList.toggle("is-error", isError);
    node.classList.add("is-visible");
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => node.classList.remove("is-visible"), 5000);
  }

  async function api(path, options = {}) {
    const method = options.method || "GET";
    const headers = { "X-Herald-Token": token };
    let body;
    if (options.body !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(options.body);
    }
    const response = await fetch(path, { method, headers, body });
    const payload = await response.json().catch(() => null);
    if (!payload) throw new Error(`bad response (${response.status})`);
    if (!payload.ok) {
      throw new Error(payload.error?.message || "request failed");
    }
    return payload.data ?? payload;
  }

  function formatTime(ms) {
    if (!ms) return "—";
    try {
      return new Date(ms).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return "—";
    }
  }

  function shortHash(value) {
    if (!value) return "—";
    return String(value).slice(0, 12) + "…";
  }

  async function loadMeta() {
    try {
      const meta = await api("/api/meta");
      const config = meta.config || {};
      $("cfg-path").textContent = meta.config_path ? `config: ${meta.config_path}` : "config: default";
      if (config["x.handle"]) {
        state.provider = "x";
      } else if (config.default_provider) {
        state.provider = config.default_provider;
      }
      const requestedProvider = params.get("provider");
      if (requestedProvider) state.provider = requestedProvider;
      const requestedScope = params.get("scope");
      if (requestedScope && scopeTitles[requestedScope]) state.scope = requestedScope;
      const select = $("provider");
      if ([...select.options].some((option) => option.value === state.provider)) {
        select.value = state.provider;
      }
    } catch (error) {
      toast(`config: ${error.message}`, true);
    }
  }

  function emptyList(message) {
    const node = $("list");
    node.replaceChildren(el("div", "list-empty", message));
  }

  function applyFilter() {
    const needle = state.filter.trim().toLowerCase();
    state.filtered = needle
      ? state.items.filter((item) =>
          [item.sender, item.subject, item.thread_id, item.id]
            .filter(Boolean)
            .join(" ")
            .toLowerCase()
            .includes(needle),
        )
      : state.items.slice();
    if (state.cursor >= state.filtered.length) state.cursor = 0;
    $("list-count").textContent = needle
      ? `${state.filtered.length}/${state.items.length}`
      : String(state.items.length);
  }

  function renderItems() {
    const list = $("list");
    list.replaceChildren();
    applyFilter();
    if (!state.filtered.length) {
      if (state.items.length) {
        emptyList("Nothing matches this filter.");
      } else if (state.scope === "dms") {
        emptyList("No DMs. X renders DMs in an embedded app Herald does not read yet.");
      } else {
        emptyList("Nothing on this surface.");
      }
      return;
    }
    state.filtered.forEach((item, index) => {
      const button = el("button", "item");
      button.type = "button";
      button.dataset.id = item.id;
      if (item.unread) button.classList.add("is-unread");
      if (state.selected === item.id) button.classList.add("is-selected");
      if (index === state.cursor) button.classList.add("is-cursor");

      button.append(el("span", "dot"));
      const body = el("span");
      const meta = el("span", "item-meta");
      meta.append(
        el("span", "item-sender", item.sender || "unknown"),
        el("span", "item-time", formatTime(item.timestamp_ms)),
      );
      body.append(meta, el("span", "item-subject", item.subject || "(no subject)"));
      body.append(el("span", "item-thread", item.thread_id || item.id));
      button.append(body);
      button.addEventListener("click", () => {
        state.cursor = index;
        openThread(item);
      });
      list.append(button);
    });
  }

  function moveCursor(delta) {
    if (!state.filtered.length) return;
    const next = (state.cursor + delta + state.filtered.length) % state.filtered.length;
    state.cursor = next;
    renderItems();
    document.querySelector(".item.is-cursor")?.scrollIntoView({ block: "nearest" });
  }

  function invalidateReads() {
    state.cache.clear();
    state.threadCache.clear();
  }

  function refreshAll() {
    // Explicit user action: bypass the read cache and check X exactly once.
    loadInbox(true);
    loadApprovals();
    loadAutopost();
    loadHistory();
  }

  function threadError(message) {
    const view = $("thread-view");
    view.replaceChildren();
    const wrap = el("div", "empty");
    wrap.append(el("div", "empty-mark", "!"), el("h3", null, "Cannot read this thread"), el("p", null, message));
    view.append(wrap);
  }

  function renderThread(thread) {
    const view = $("thread-view");
    view.replaceChildren();
    const head = el("div", "thread-head");
    head.append(
      el("div", "kicker", `${thread.provider || state.provider} · ${thread.messages?.length || 0} message(s)`),
      el("h3", null, thread.subject || "Thread"),
      el("span", "tiny", thread.id || ""),
    );
    view.append(head);

    (thread.messages || []).forEach((message, index) => {
      const node = el("article", "message");
      node.style.animationDelay = `${Math.min(index * 30, 240)}ms`;
      const meta = el("div", "message-meta");
      meta.append(
        el("span", "message-sender", message.sender || "unknown"),
        el("span", null, formatTime(message.timestamp_ms)),
      );
      node.append(meta, el("p", "message-body", message.body || ""));
      view.append(node);
    });

    if (!thread.messages || !thread.messages.length) {
      view.append(el("div", "empty", "This thread has no readable messages."));
    }
  }

  async function openThread(item) {
    state.selected = item.id;
    renderItems();
    switchTab("thread");
    const id = item.thread_id || item.id;
    const key = `${state.provider}:${id}`;
    const cached = state.threadCache.get(key);
    if (cached && Date.now() - cached.at < CACHE_TTL_MS) {
      renderThread(cached.thread);
      setStatus(`Cached thread · ${checkedLabel(cached.at)}`);
      return;
    }
    setStatus("Reading thread (one browser run)…", true);
    try {
      const thread = await api(
        `/api/thread?provider=${encodeURIComponent(state.provider)}&id=${encodeURIComponent(id)}`,
      );
      state.threadCache.set(key, { thread, at: Date.now() });
      renderThread(thread);
      setStatus("Thread loaded");
    } catch (error) {
      threadError(error.message);
      setStatus("Thread failed");
      toast(error.message, true);
    }
  }

  function cacheKey() {
    return `${state.provider}:${state.scope}`;
  }

  function checkedLabel(at) {
    const minutes = Math.floor((Date.now() - at) / 60000);
    return minutes < 1 ? "checked just now" : `checked ${minutes}m ago`;
  }

  function renderInboxMeta(at) {
    $("count-line").textContent =
      `${state.items.length} item(s) · ${state.provider} · ${state.scope} · ${checkedLabel(at)}`;
  }

  async function loadInbox(force = false) {
    const key = cacheKey();
    const cached = state.cache.get(key);
    if (!force && cached && Date.now() - cached.at < CACHE_TTL_MS) {
      state.items = cached.items;
      state.cursor = 0;
      renderItems();
      renderInboxMeta(cached.at);
      setStatus("Cached · press Refresh to check X");
      return;
    }

    setStatus(`Checking ${scopeTitles[state.scope].toLowerCase()} (one browser run)…`, true);
    try {
      const data = await api(
        `/api/inbox?scope=${encodeURIComponent(state.scope)}&provider=${encodeURIComponent(state.provider)}&limit=20`,
      );
      state.items = data.messages || [];
      state.cache.set(key, { items: state.items, at: Date.now() });
      state.cursor = 0;
      renderItems();
      renderInboxMeta(Date.now());
      setStatus("Idle");
    } catch (error) {
      state.items = [];
      emptyList("Could not read this surface. " + error.message);
      $("count-line").textContent = "—";
      setStatus("Read failed");
      toast(error.message, true);
    }
  }

  function approvalCard(approval) {
    const card = el("article", "approval");
    const top = el("div", "approval-top");
    top.append(
      el("span", null, `${approval.provider} · ${formatTime(approval.expires_at_ms)}`),
      el("span", null, shortHash(approval.body_hash)),
    );
    card.append(
      top,
      el("p", "approval-target", approval.target || approval.thread_id || ""),
      el("pre", "approval-body", approval.body || ""),
    );
    const actions = el("div", "approval-actions");
    const approve = el("button", "run", "Review");
    approve.type = "button";
    approve.addEventListener("click", () => askApproval(approval));
    const reject = el("button", "ghost danger", "Reject");
    reject.type = "button";
    reject.addEventListener("click", () => rejectApproval(approval));
    actions.append(reject, approve);
    card.append(actions);
    return card;
  }

  function renderApprovals() {
    const queue = $("queue");
    queue.replaceChildren();
    $("queue-count").textContent = state.approvals.length;
    if (!state.approvals.length) {
      const empty = el("div", "empty");
      empty.append(el("div", "empty-mark", "✓"), el("h3", null, "Queue clear"), el("p", null, "Nothing is waiting for a decision."));
      queue.append(empty);
      return;
    }
    state.approvals.forEach((approval) => queue.append(approvalCard(approval)));
  }

  function historyRow(action) {
    const row = el("div", "history-row");
    row.dataset.state = action.state;
    const badge = el("span", "badge state-badge", action.state);
    badge.dataset.state = action.state;
    row.append(badge);

    const main = el("div", "history-main");
    const top = el("div", "history-top");
    top.append(
      el("span", null, `${action.provider} · ${formatTime(action.created_at_ms)}`),
      el("span", null, String(action.action_id || "").slice(0, 8)),
    );
    main.append(
      top,
      el("div", "history-operation", action.operation || "operation"),
      el("div", "history-target", action.target || action.thread_id || ""),
    );

    if (action.state === "uncertain") {
      const actions = el("div", "history-actions");
      const reconcile = el("button", "ghost", "Reconcile");
      reconcile.type = "button";
      reconcile.addEventListener("click", () => reconcileAction(action));
      actions.append(reconcile);
      main.append(actions);
    }
    row.append(main);
    return row;
  }

  function renderHistory() {
    const view = $("history");
    view.replaceChildren();
    if (!state.history.length) {
      const empty = el("div", "empty");
      empty.append(
        el("div", "empty-mark", "·"),
        el("h3", null, "No recorded actions"),
        el("p", null, "Reads, drafts, approvals, and sends appear here after they run."),
      );
      view.append(empty);
      return;
    }
    state.history.forEach((action, index) => {
      const row = historyRow(action);
      row.style.animationDelay = `${Math.min(index * 20, 200)}ms`;
      view.append(row);
    });
  }

  async function loadHistory() {
    try {
      const data = await api("/api/history?limit=20");
      state.history = data.actions || [];
      renderHistory();
    } catch (error) {
      toast(`history: ${error.message}`, true);
    }
  }

  async function reconcileAction(action) {
    setStatus("Reconciling…", true);
    try {
      const data = await api("/api/reconcile", {
        method: "POST",
        body: { id: action.action_id },
      });
      toast(`reconcile: ${data.status || "done"} — ${data.detail || ""}`);
      setStatus("Idle");
      invalidateReads();
      await loadHistory();
    } catch (error) {
      setStatus("Reconcile failed");
      toast(error.message, true);
    }
  }

  async function loadApprovals() {
    try {
      const data = await api("/api/approvals");
      state.approvals = data.approvals || [];
      renderApprovals();
    } catch (error) {
      toast(`approvals: ${error.message}`, true);
    }
  }

  async function loadAutopost() {
    try {
      const data = await api("/api/autopost");
      const badge = $("autopost-badge");
      badge.textContent = data.status === "enabled" ? "armed" : "approval only";
      badge.dataset.state = data.status === "enabled" ? "on" : "off";
      $("autopost-detail").textContent =
        `${data.used_today} of ${data.daily_limit} posts today · quality floor ${data.min_quality}` +
        (data.quiet_hours ? ` · quiet ${data.quiet_now ? "now" : data.quiet_hours}` : "") +
        (data.kill_switch ? " · kill switch on" : "");
      $("autopost-off").disabled = Boolean(data.kill_switch);
    } catch (error) {
      $("autopost-detail").textContent = error.message;
    }
  }

  function askApproval(approval) {
    state.pendingApproval = approval;
    $("dialog-target").textContent = `${approval.provider} · ${approval.target || approval.thread_id}`;
    $("dialog-body").textContent = approval.body || "";
    $("dialog-hash").textContent = approval.body_hash || "—";
    $("dialog-expires").textContent = formatTime(approval.expires_at_ms);
    $("approve-dialog").showModal();
  }

  async function approvePending() {
    const approval = state.pendingApproval;
    if (!approval) return;
    setStatus("Sending…", true);
    try {
      const data = await api("/api/approve", { method: "POST", body: { id: approval.action_id } });
      toast(`approve: ${data.status || "done"} — ${data.detail || ""}`);
      setStatus("Idle");
      state.pendingApproval = null;
      invalidateReads();
      await Promise.all([loadApprovals(), loadInbox(), loadHistory()]);
    } catch (error) {
      setStatus("Send failed");
      toast(error.message, true);
    }
  }

  async function rejectApproval(approval) {
    if (!window.confirm(`Reject ${approval.action_id}? Nothing will be sent.`)) return;
    try {
      await api("/api/reject", { method: "POST", body: { id: approval.action_id } });
      toast("Rejected. Nothing was sent.");
      invalidateReads();
      await Promise.all([loadApprovals(), loadHistory()]);
    } catch (error) {
      toast(error.message, true);
    }
  }

  async function runAsk(event) {
    event.preventDefault();
    const instruction = $("instruction").value.trim();
    if (!instruction) return;
    setStatus("Planning…", true);
    try {
      const data = await api("/api/ask", {
        method: "POST",
        body: {
          instruction,
          provider: state.provider,
          enable_send: $("enable-send").checked,
        },
      });
      const status = data.status || "done";
      toast(`${status}${data.action_id ? ` · ${data.action_id}` : ""}${data.detail ? `\n${data.detail}` : ""}`);
      setStatus(status === "awaiting_approval" ? "Awaiting approval" : "Idle");
      invalidateReads();
      await Promise.all([loadApprovals(), loadInbox(), loadHistory()]);
      if (status === "awaiting_approval") switchTab("queue");
    } catch (error) {
      setStatus("Plan failed");
      toast(error.message, true);
    }
  }

  async function showProviders() {
    const dialog = $("providers-dialog");
    const list = $("providers-list");
    list.replaceChildren(el("div", "tiny", "Checking…"));
    dialog.showModal();
    try {
      const data = await api("/api/providers");
      list.replaceChildren();
      (data.providers || []).forEach((provider) => {
        const row = el("div", "provider-row");
        row.append(
          el("span", null, provider.provider),
          el("span", provider.status === "ready" ? "ok" : "bad", provider.status),
          el("span", null, provider.detail),
        );
        list.append(row);
      });
    } catch (error) {
      list.replaceChildren(el("div", "tiny", error.message));
    }
  }

  function switchTab(tab) {
    state.tab = tab;
    $("tab-thread").classList.toggle("is-active", tab === "thread");
    $("tab-queue").classList.toggle("is-active", tab === "queue");
    $("tab-history").classList.toggle("is-active", tab === "history");
    $("thread-view").classList.toggle("is-hidden", tab !== "thread");
    $("queue-view").classList.toggle("is-hidden", tab !== "queue");
    $("history-view").classList.toggle("is-hidden", tab !== "history");
  }

  function bind() {
    document.querySelectorAll(".scope").forEach((button) => {
      button.addEventListener("click", () => {
        document.querySelectorAll(".scope").forEach((other) => other.classList.remove("is-active"));
        button.classList.add("is-active");
        state.scope = button.dataset.scope;
        $("scope-title").textContent = scopeTitles[state.scope];
        $("scope-kicker").textContent = state.scope === "all" ? "inbox" : state.scope;
        loadInbox();
      });
    });

    $("provider").addEventListener("change", (event) => {
      state.provider = event.target.value;
      loadInbox();
      loadAutopost();
    });

    $("refresh").addEventListener("click", refreshAll);

    $("filter").addEventListener("input", (event) => {
      state.filter = event.target.value;
      state.cursor = 0;
      renderItems();
    });

    $("filter").addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.target.value = "";
        state.filter = "";
        state.cursor = 0;
        renderItems();
        event.target.blur();
      }
    });

    document.addEventListener("keydown", (event) => {
      const tag = event.target.tagName;
      if (["INPUT", "TEXTAREA", "SELECT"].includes(tag) || document.querySelector("dialog[open]")) {
        return;
      }
      switch (event.key) {
        case "j":
          moveCursor(1);
          event.preventDefault();
          break;
        case "k":
          moveCursor(-1);
          event.preventDefault();
          break;
        case "Enter": {
          const item = state.filtered[state.cursor];
          if (item) openThread(item);
          event.preventDefault();
          break;
        }
        case "r":
          refreshAll();
          break;
        case "q":
          switchTab("queue");
          break;
        case "h":
          switchTab("history");
          break;
        case "t":
          switchTab("thread");
          break;
        case "/":
          $("filter").focus();
          event.preventDefault();
          break;
        default:
          break;
      }
    });

    $("providers").addEventListener("click", showProviders);
    $("providers-close").addEventListener("click", () => $("providers-dialog").close());
    $("ask").addEventListener("submit", runAsk);

    $("autopost-off").addEventListener("click", async () => {
      if (!window.confirm("Write the autopost kill switch? Autonomous posts stop immediately.")) return;
      try {
        await api("/api/autopost", { method: "POST", body: { action: "off" } });
        toast("Kill switch written. Autopost is off.");
        loadAutopost();
      } catch (error) {
        toast(error.message, true);
      }
    });

    $("tab-thread").addEventListener("click", () => switchTab("thread"));
    $("tab-queue").addEventListener("click", () => switchTab("queue"));
    $("tab-history").addEventListener("click", () => switchTab("history"));

    const dialog = $("approve-dialog");
    dialog.addEventListener("close", () => {
      if (dialog.returnValue === "confirm") approvePending();
      else state.pendingApproval = null;
    });

    $("instruction").addEventListener("input", (event) => {
      const node = event.target;
      node.style.height = "auto";
      node.style.height = `${Math.min(node.scrollHeight, 140)}px`;
    });

    $("instruction").addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        $("ask").requestSubmit();
      }
    });
  }

  async function boot() {
    bind();
    await loadMeta();
    const select = $("provider");
    select.value = state.provider;
    document.querySelectorAll(".scope").forEach((button) => {
      button.classList.toggle("is-active", button.dataset.scope === state.scope);
    });
    $("scope-title").textContent = scopeTitles[state.scope];
    $("scope-kicker").textContent = state.scope === "all" ? "inbox" : state.scope;
    await Promise.all([loadInbox(), loadApprovals(), loadAutopost(), loadHistory()]);
    const requestedThread = params.get("thread");
    const match = requestedThread && state.items.find((item) => item.thread_id === requestedThread);
    if (match) {
      openThread(match);
    } else if (params.get("tab") === "history") {
      switchTab("history");
    } else if (params.get("tab") === "queue") {
      switchTab("queue");
    } else if (state.approvals.length) {
      switchTab("queue");
    } else {
      switchTab("thread");
    }
  }

  boot().catch((error) => {
    setStatus("UI failed");
    toast(error.message, true);
  });
})();
