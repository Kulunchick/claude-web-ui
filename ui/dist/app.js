(() => {
  const desktop = Object.hasOwn(window, "__CWU_DESKTOP_ACCESS_TOKEN");

  const state = {
    initialized: true,
    screen: "login",
    accessToken: desktop ? window.__CWU_DESKTOP_ACCESS_TOKEN : localStorage.getItem("cwu_access_token"),
    refreshToken: desktop ? null : localStorage.getItem("cwu_refresh_token"),
    user: null,
    projects: [],
    activeProjectId: null,
    projectCli: null,
    tab: "overview",
    adminUsers: [],
    adminInvites: [],
    adminAccounts: [],
    aclProjectId: null,
    aclMembers: [],
    aclList: [],
    addAccountModal: null,
    asmScope: "project",
    asmProvider: "claude_code",
    asmSubTab: "agents",
    asmRawMode: false,
    asmAgents: [],
    asmSkills: [],
    asmMcpConfig: null,
    asmPresets: [],
    asmSelectedAgent: null,
    asmSelectedSkill: null,
    asmGraphifyResult: null,
    ptySocket: null,
    ptyTerminal: null,
    projectTab: "chat",
    chatSocket: null,
    nativeTranscriptSocket: null,
    nativeTranscriptSessionId: null,
    sessions: [],
    activeSessionId: null,
    activeSessionDetail: null,
    sessionSearch: "",
    showProjectTerminal: false,
    terminalMode: "shell",
    schedulerStatus: null,
    respectGitignore: true,
    fileTree: [],
    collapsedDirs: {},
    activeFile: null,
    editorText: "",
    editorDirty: false,
    gitStatus: null,
    gitDiff: null,
    diffMode: "unified",
    diffPath: null,
    diffStaged: false,
    error: null,
    notice: null,
    messengerPairing: null,
  };

  const app = document.getElementById("app");
  let leptos;

  if (desktop) {
    window.addEventListener("cwu:desktop-token", () => {
      state.accessToken = window.__CWU_DESKTOP_ACCESS_TOKEN;
      if (!leptos) return;
      if (state.user) {
        connectChatStream(state.activeProjectId);
        if (state.projectTab === "cron") render();
      } else {
        boot();
      }
    });
  }

  async function api(path, options = {}) {
    const headers = { "Content-Type": "application/json", ...(options.headers || {}) };
    if (state.accessToken) {
      headers["Authorization"] = `Bearer ${state.accessToken}`;
    }
    const res = await fetch(path, { ...options, headers });
    const text = await res.text();
    let data = null;
    try { data = text ? JSON.parse(text) : null; } catch { data = text; }
    if (!res.ok) {
      const msg = data && data.error ? data.error : `HTTP ${res.status}`;
      throw new Error(msg);
    }
    return data;
  }

  function setPtyLive(live) {
    try { if (leptos && leptos.set_oauth_terminal_live) leptos.set_oauth_terminal_live(live); } catch {}
  }

  // Teardown callbacks for the live PTY (ResizeObserver, host listeners, ...).
  // Run by closeActivePty, which every render() and every (re)launch goes through.
  let ptyCleanups = [];

  function closeActivePty() {
    setPtyLive(false);
    const cleanups = ptyCleanups;
    ptyCleanups = [];
    for (const fn of cleanups) {
      try { fn(); } catch {}
    }
    if (state.ptySocket) {
      const ws = state.ptySocket;
      state.ptySocket = null;
      ws.onopen = null;
      ws.onmessage = null;
      ws.onclose = null;
      ws.onerror = null;
      try { ws.close(); } catch {}
    }
    if (state.ptyTerminal) {
      try { state.ptyTerminal.dispose(); } catch {}
      state.ptyTerminal = null;
    }
  }

  function closeChatSocket() {
    closeNativeTranscript();
    if (state.chatSocket) {
      try { state.chatSocket.close(); } catch {}
      state.chatSocket = null;
    }
  }

  function saveTokens(auth) {
    state.accessToken = auth.access_token;
    state.refreshToken = auth.refresh_token;
    state.user = auth.user;
    if (!desktop) {
      localStorage.setItem("cwu_access_token", auth.access_token);
      localStorage.setItem("cwu_refresh_token", auth.refresh_token);
    }
  }

  function clearSession() {
    closeActivePty();
    closeChatSocket();
    state.accessToken = null;
    state.refreshToken = null;
    state.user = null;
    state.messengerPairing = null;
    state.projects = [];
    state.projectCli = null;
    state.adminAccounts = [];
    state.addAccountModal = null;
    state.sessions = [];
    state.activeSessionId = null;
    state.activeSessionDetail = null;
    state.nativeTranscriptSessionId = null;
    state.showProjectTerminal = false;
    localStorage.removeItem("cwu_access_token");
    localStorage.removeItem("cwu_refresh_token");
  }

  async function boot() {
    try {
      const status = await api("/api/auth/status");
      state.initialized = status.initialized;
      if (!status.initialized) {
        clearSession();
        state.screen = "bootstrap";
        render();
        return;
      }
      if (state.accessToken) {
        try {
          await loadAuthenticatedData();
          return;
        } catch {
          clearSession();
        }
      }
      const params = new URLSearchParams(window.location.search);
      state.screen = params.get("invite") ? "invite" : "login";
    } catch (err) {
      state.error = err.message;
    }
    render();
  }

  async function loadProjectCli() {
    if (!state.activeProjectId) {
      state.projectCli = null;
      state.sessions = [];
      state.activeSessionId = null;
      state.activeSessionDetail = null;
      return;
    }
    try {
      state.projectCli = await api(`/api/projects/${state.activeProjectId}/cli`);
    } catch {
      state.projectCli = null;
    }
    await loadProjectSessions();
  }

  async function loadProjectSessions() {
    if (!state.activeProjectId) {
      state.sessions = [];
      state.activeSessionDetail = null;
      closeNativeTranscript();
      return;
    }
    try {
      const q = state.sessionSearch ? `?q=${encodeURIComponent(state.sessionSearch)}` : "";
      state.sessions = await api(`/api/projects/${state.activeProjectId}/sessions${q}`);
      if (state.activeSessionId && !state.sessions.some((s) => s.id === state.activeSessionId)) {
        state.activeSessionId = state.sessions.length > 0 ? state.sessions[0].id : null;
      } else if (!state.activeSessionId && state.sessions.length > 0) {
        state.activeSessionId = state.sessions[0].id;
      }
      if (state.activeSessionId) {
        state.activeSessionDetail = await api(`/api/projects/${state.activeProjectId}/sessions/${state.activeSessionId}`);
      } else {
        state.activeSessionDetail = null;
      }
      state.schedulerStatus = await api(`/api/projects/${state.activeProjectId}/scheduler`);
    } catch {
      state.sessions = [];
      state.activeSessionDetail = null;
    } finally {
      connectNativeTranscript();
    }
  }

  function connectChatStream(projectId) {
    closeChatSocket();
    if (!projectId || !state.accessToken) return;
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${proto}//${window.location.host}/api/projects/${encodeURIComponent(projectId)}/chat/ws?token=${encodeURIComponent(state.accessToken)}`;
    const ws = new WebSocket(wsUrl);
    state.chatSocket = ws;
    connectNativeTranscript();
    ws.onmessage = async (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.type === "scheduler_status") {
          state.schedulerStatus = msg.status;
          return;
        }
        if (msg.session_id === state.activeSessionId) {
          await loadProjectSessions();
          if (state.projectTab === "chat") render();
        }
      } catch {}
    };
  }


  function closeNativeTranscript() {
    const current = state.nativeTranscriptSocket;
    state.nativeTranscriptSocket = null;
    if (current) current.socket.close();
  }

  function displaysNativeTranscript(detail) {
    return Boolean(detail?.session.transcript_path) && (!detail.turns?.length
      || state.nativeTranscriptSessionId === detail.session.id);
  }

  function connectNativeTranscript() {
    const detail = state.activeSessionDetail;
    const projectId = state.activeProjectId;
    const sessionId = state.activeSessionId;
    const current = state.nativeTranscriptSocket;
    if (!displaysNativeTranscript(detail) || !projectId || !sessionId || !state.accessToken) {
      closeNativeTranscript();
      return;
    }
    if (current?.projectId === projectId && current.sessionId === sessionId) return;
    closeNativeTranscript();

    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const sessionPath = `/api/projects/${encodeURIComponent(projectId)}/sessions/${encodeURIComponent(sessionId)}`;
    const socket = new WebSocket(`${proto}//${window.location.host}${sessionPath}/transcript/ws?token=${encodeURIComponent(state.accessToken)}`);
    const connection = { projectId, sessionId, socket };
    state.nativeTranscriptSocket = connection;
    const isCurrent = () => state.nativeTranscriptSocket === connection
      && state.activeProjectId === projectId && state.activeSessionId === sessionId;
    let refreshPending = false;
    let refreshing = false;

    socket.onclose = () => {
      if (state.nativeTranscriptSocket === connection) state.nativeTranscriptSocket = null;
    };
    socket.onmessage = async (message) => {
      const update = JSON.parse(message.data);
      if (!isCurrent() || update.session_id !== sessionId) return;
      if (update.type !== "native_transcript_ready" && update.type !== "native_transcript_appended") return;
      // Refresh the canonical native snapshot so records already loaded over HTTP are not appended twice.
      refreshPending = true;
      if (refreshing) return;
      refreshing = true;
      try {
        while (refreshPending && isCurrent()) {
          refreshPending = false;
          const latest = await api(sessionPath);
          if (!isCurrent() || !displaysNativeTranscript(latest) || !displaysNativeTranscript(state.activeSessionDetail)) return;
          state.activeSessionDetail = latest;
          const feed = state.tab === "overview" && state.projectTab === "chat" && app.querySelector(".chat-feed");
          if (feed) {
            leptos.mount_native_feed(feed, JSON.stringify(latest.native_messages || []));
          }
        }
      } catch {
        // Ordinary session refresh remains available if the native file disappears.
      } finally {
        refreshing = false;
      }
    };
  }

  async function loadAclProjectData(projectId) {
    if (!projectId || state.user?.role !== "admin") {
      state.aclMembers = [];
      state.aclList = [];
      return;
    }
    state.aclMembers = await api(`/api/admin/projects/${projectId}/members`);
    state.aclList = await api(`/api/admin/projects/${projectId}/account-acls`);
  }

  async function loadAuthenticatedData() {
    const me = await api("/api/auth/me");
    state.user = me.user;
    state.projects = me.projects || [];
    if (!state.activeProjectId && state.projects.length > 0) {
      state.activeProjectId = state.projects[0].id;
    }
    await loadProjectCli();
    try {
      state.asmPresets = await api("/api/mcp/presets");
    } catch {
      state.asmPresets = [];
    }
    if (state.tab === "agents_skills_mcp") {
      await loadAsmData();
    }
    if (state.user.role === "admin") {
      state.adminUsers = await api("/api/admin/users");
      state.adminInvites = await api("/api/admin/invites");
      state.adminAccounts = await api("/api/admin/accounts");
      if (!state.aclProjectId && state.projects.length > 0) {
        state.aclProjectId = state.projects[0].id;
      }
      if (state.aclProjectId) {
        await loadAclProjectData(state.aclProjectId);
      }
    }
    if (state.activeProjectId) {
      connectChatStream(state.activeProjectId);
      await loadProjectWorkspace();
    }
    render();
  }

  async function loadProjectWorkspace() {
    if (!state.activeProjectId) return;
    const pid = encodeURIComponent(state.activeProjectId);
    try {
      const [tree, gitStatus] = await Promise.all([
        api(`/api/projects/${pid}/files/tree?respect_gitignore=${state.respectGitignore}`),
        api(`/api/projects/${pid}/git/status`),
      ]);
      state.fileTree = tree.entries || [];
      state.gitStatus = gitStatus;
      if (gitStatus.is_git_repo) {
        await loadGitDiff(state.diffPath, state.diffStaged, false);
      } else {
        state.gitDiff = null;
      }
    } catch (err) {
      state.error = err.message;
    }
  }

  async function openWorkspaceFile(path) {
    if (!state.activeProjectId) return;
    const pid = encodeURIComponent(state.activeProjectId);
    state.error = null;
    state.notice = null;
    try {
      const file = await api(`/api/projects/${pid}/files/content?path=${encodeURIComponent(path)}`);
      state.activeFile = file;
      state.editorText = file.content || "";
      state.editorDirty = false;
      render();
    } catch (err) {
      state.error = err.message;
      render();
    }
  }

  async function saveWorkspaceFile() {
    if (!state.activeProjectId || !state.activeFile) return;
    const pid = encodeURIComponent(state.activeProjectId);
    state.error = null;
    state.notice = null;
    try {
      await api(`/api/projects/${pid}/files/content`, {
        method: "PUT",
        body: JSON.stringify({ path: state.activeFile.path, content: state.editorText }),
      });
      state.editorDirty = false;
      state.notice = `Saved ${state.activeFile.path}`;
      await loadProjectWorkspace();
      render();
    } catch (err) {
      state.error = err.message;
      render();
    }
  }

  async function loadGitDiff(path = null, staged = false, shouldRender = true) {
    if (!state.activeProjectId) return;
    const pid = encodeURIComponent(state.activeProjectId);
    state.diffPath = path;
    state.diffStaged = staged;
    const params = new URLSearchParams();
    if (path) params.set("path", path);
    params.set("staged", staged ? "true" : "false");
    params.set("mode", state.diffMode);
    try {
      state.gitDiff = await api(`/api/projects/${pid}/git/diff?${params.toString()}`);
      if (shouldRender) render();
    } catch (err) {
      state.error = err.message;
      if (shouldRender) render();
    }
  }

  function esc(str) {
    return String(str ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function linkifyUrls(text) {
    return esc(text).replace(
      /(https?:\/\/[^\s<>"']+)/g,
      '<a href="$1" target="_blank" rel="noopener noreferrer">$1</a>'
    );
  }

  function render() {
    leptos.unmount_gateway_view();
    if (!state.user) {
      renderAuth();
    } else {
      renderDashboard();
    }
  }

  window.addEventListener("cwu:auth-screen", ({ detail: screen }) => {
    queueMicrotask(() => {
      if (state.user || app.querySelector(".auth-card button[type='submit']:disabled")) return;
      state.error = null;
      state.screen = screen;
      render();
    });
  });

  window.addEventListener("cwu:switch-project-cli", async ({ detail }) => {
    const projectId = state.activeProjectId;
    if (!state.user || !projectId || !state.projectCli?.can_switch) return;
    state.error = null;
    state.notice = null;
    try {
      const projectCli = await api(`/api/projects/${encodeURIComponent(projectId)}/cli`, { method: "PUT", body: detail });
      if (state.activeProjectId !== projectId) return;
      state.projectCli = projectCli;
    } catch (error) {
      if (state.activeProjectId !== projectId) return;
      state.error = error.message;
    }
    render();
  });

  async function openProjectSession(sessionId, source) {
    if (!state.user || state.projectTab !== source) return;
    state.activeSessionId = sessionId;
    state.sessionSearch = "";
    state.projectTab = "chat";
    await loadProjectSessions();
    render();
  }

  window.addEventListener("cwu:open-cron-session", ({ detail }) => openProjectSession(detail, "cron"));
  window.addEventListener("cwu:open-kanban-session", ({ detail }) => openProjectSession(detail, "kanban"));

  let changingProject = false;
  window.addEventListener("cwu:select-project", async ({ detail: projectId }) => {
    if (!state.user || changingProject || !state.projects.some((proj) => proj.id === projectId)) return;
    changingProject = true;
    try {
      state.activeProjectId = projectId;
      state.projectTab = "chat";
      state.activeSessionId = null;
      state.tab = "overview";
      state.activeFile = null;
      state.editorText = "";
      state.editorDirty = false;
      state.diffPath = null;
      await loadProjectCli();
      connectChatStream(state.activeProjectId);
      await loadProjectWorkspace();
      render();
    } finally {
      changingProject = false;
    }
  });

  app.addEventListener("submit", async (event) => {
    const form = event.target;
    if (state.user || !form.matches(".auth-card")) return;
    event.preventDefault();
    const submit = form.querySelector("button[type='submit']");
    if (submit.disabled) return;
    submit.disabled = true;
    try {
      const fields = JSON.stringify(Object.fromEntries(new FormData(form)));
      const auth = JSON.parse(await leptos.submit_auth(state.screen, fields));
      saveTokens(auth);
      await loadAuthenticatedData();
    } catch (error) {
      state.error = String(error.message || error);
      render();
    }
  });

  function renderAuth() {
    const invite = new URLSearchParams(window.location.search).get("invite") || "";
    app.replaceChildren();
    leptos.mount_auth(app, state.screen, state.error, invite);
  }

  app.addEventListener("submit", async (event) => {
    const form = event.target;
    const project = form.id === "create-project-form";
    if (state.user?.role !== "admin" || (!project && form.id !== "create-invite-form")) return;
    event.preventDefault();
    const fields = new FormData(form);
    const button = form.querySelector("button[type='submit']");
    if (button.disabled) return;
    button.disabled = true;
    state.error = null;
    state.notice = null;
    try {
      if (project) {
        const body = { name: fields.get("name"), workspace_path: fields.get("workspace_path") };
        await api("/api/admin/projects", { method: "POST", body: JSON.stringify(body) });
        state.notice = "Project registered.";
      } else {
        const body = { role: fields.get("role"), project_ids: fields.getAll("project_ids") };
        const invite = await api("/api/admin/invites", { method: "POST", body: JSON.stringify(body) });
        state.notice = `Created invite code: ${invite.code}`;
      }
      await loadAuthenticatedData();
    } catch (error) {
      state.error = error.message;
      render();
    } finally {
      button.disabled = false;
    }
  });

  app.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-revoke-uid]");
    if (!button || button.disabled || state.user?.role !== "admin") return;
    button.disabled = true;
    state.error = null;
    state.notice = null;
    try {
      await api(`/api/admin/users/${encodeURIComponent(button.dataset.revokeUid)}/revoke-sessions`, { method: "POST" });
      state.notice = "User sessions revoked.";
      await loadAuthenticatedData();
    } catch (error) {
      state.error = error.message;
      render();
    } finally {
      button.disabled = false;
    }
  });

  // ---------------------------------------------------------------------------
  // Dashboard shell: sidebar, page header, drawer.
  //
  // renderDashboard() rebuilds the shell on every render(), so whatever must
  // survive a render (collapse, drawer, gateway status) lives in `shell` and is
  // re-applied from there. Navigation state stays in `state.tab` (top level) and
  // `state.projectTab` (project section); the sidebar items only drive it.
  // ---------------------------------------------------------------------------
  const SIDEBAR_COLLAPSED_KEY = "cwu_sidebar_collapsed";
  const GATEWAY_STATUS_INTERVAL_MS = 30000;

  const shell = {
    collapsed: readSidebarCollapsed(),
    drawerOpen: false,
    gatewayStatus: "checking", // "checking" | "online" | "offline"
    version: null,
    statusTimer: null,
  };

  // Storage can be unavailable (private mode, blocked site data); the
  // preference is then simply not remembered.
  function readSidebarCollapsed() {
    try { return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1"; } catch { return false; }
  }

  function writeSidebarCollapsed(collapsed) {
    try { localStorage.setItem(SIDEBAR_COLLAPSED_KEY, collapsed ? "1" : "0"); } catch {}
  }

  function applyShellClasses() {
    const root = app.querySelector(".dashboard-shell");
    if (!root) return;
    root.classList.toggle("is-collapsed", shell.collapsed);
    root.classList.toggle("is-drawer-open", shell.drawerOpen);
    const collapse = document.getElementById("sidebar-collapse-btn");
    if (collapse) {
      const label = shell.collapsed ? "Expand sidebar" : "Collapse sidebar";
      collapse.setAttribute("aria-pressed", String(shell.collapsed));
      collapse.setAttribute("aria-label", label);
      collapse.title = label;
    }
    const hamburger = document.getElementById("sidebar-open-btn");
    if (hamburger) hamburger.setAttribute("aria-expanded", String(shell.drawerOpen));
  }

  function setDrawerOpen(open) {
    shell.drawerOpen = open;
    applyShellClasses();
  }

  // Navigating from the drawer closes it. Capture phase, so the flag is cleared
  // before the item's own onclick re-renders the shell.
  app.addEventListener("click", (event) => {
    if (shell.drawerOpen && event.target.closest(".sidebar .nav-item, .sidebar .project-item")) {
      setDrawerOpen(false);
    }
  }, true);

  // The project switcher is a <details> dropdown: an outside click closes it.
  document.addEventListener("click", (event) => {
    const switcher = app.querySelector(".project-switcher[open]");
    if (switcher && !switcher.contains(event.target)) switcher.open = false;
  });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const switcher = app.querySelector(".project-switcher[open]");
    if (switcher) {
      switcher.open = false;
      switcher.querySelector("summary").focus();
    } else if (shell.drawerOpen) {
      setDrawerOpen(false);
      document.getElementById("sidebar-open-btn")?.focus();
    }
  });

  function renderGatewayStatus() {
    const el = document.getElementById("gateway-status");
    if (!el) return;
    const label = { checking: "Checking", online: "Online", offline: "Unreachable" }[shell.gatewayStatus];
    el.dataset.status = shell.gatewayStatus;
    el.title = `Gateway: ${label}`;
    el.querySelector(".system-status-value").textContent = label;
    const version = document.getElementById("gateway-version");
    if (version) version.textContent = shell.version ? `v${shell.version}` : "";
  }

  async function refreshGatewayStatus() {
    try {
      const res = await fetch("/api/health", { cache: "no-store" });
      shell.gatewayStatus = res.ok ? "online" : "offline";
    } catch {
      shell.gatewayStatus = "offline";
    }
    if (!shell.version && shell.gatewayStatus === "online") {
      try {
        const res = await fetch("/api/version", { cache: "no-store" });
        if (res.ok) shell.version = (await res.json()).version || null;
      } catch {}
    }
    renderGatewayStatus();
  }

  function startGatewayStatus() {
    if (shell.statusTimer) return;
    void refreshGatewayStatus();
    shell.statusTimer = setInterval(() => {
      if (!state.user) {
        clearInterval(shell.statusTimer);
        shell.statusTimer = null;
        return;
      }
      void refreshGatewayStatus();
    }, GATEWAY_STATUS_INTERVAL_MS);
  }

  // Inline 16px stroke icons for the nav rail (no icon font, no CDN).
  const ICON_PATHS = {
    chat: '<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>',
    files: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="m10 13-2 2 2 2"/><path d="m14 17 2-2-2-2"/>',
    kanban: '<rect x="3" y="3" width="18" height="18" rx="1"/><path d="M9 3v18"/><path d="M15 3v18"/>',
    cron: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    agents: '<rect x="4" y="8" width="16" height="12" rx="1"/><path d="M12 8V4"/><path d="M9 4h6"/><path d="M9 13v2"/><path d="M15 13v2"/>',
    users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
    key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 8.8-8.8"/><path d="m16 7 3 3"/><path d="m19 4 2 2"/>',
    folder: '<path d="M3 6a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
    chevron: '<path d="m6 9 6 6 6-6"/>',
    link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
    logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="m16 17 5-5-5-5"/><path d="M21 12H9"/>',
    rail: '<rect x="3" y="3" width="18" height="18" rx="1"/><path d="M9 3v18"/>',
    menu: '<path d="M4 6h16"/><path d="M4 12h16"/><path d="M4 18h16"/>',
    close: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  };

  function icon(name) {
    return `<svg class="icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${ICON_PATHS[name]}</svg>`;
  }

  // `label` is trusted markup (entities allowed); the title attribute is the
  // tooltip in the collapsed rail.
  function navItem(id, label, iconName, active) {
    return `<button class="nav-item${active ? " active" : ""}" id="${id}" type="button" title="${label}"${active ? ' aria-current="page"' : ""}>${icon(iconName)}<span class="nav-label">${label}</span></button>`;
  }

  // Chat session rail: a right-side drawer below the large breakpoint. The open
  // flag lives here because the chat view is re-mounted on every render.
  let chatRailOpen = false;

  function applyChatRailState() {
    const root = app.querySelector(".chat-workspace");
    if (!root) chatRailOpen = false;
    if (root) root.classList.toggle("is-rail-open", chatRailOpen);
    const toggle = document.getElementById("chat-rail-toggle");
    if (toggle) toggle.setAttribute("aria-expanded", String(chatRailOpen));
  }

  function setChatRailOpen(open) {
    chatRailOpen = open;
    applyChatRailState();
  }

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && chatRailOpen) {
      setChatRailOpen(false);
      document.getElementById("chat-rail-toggle")?.focus();
    }
  });

  // ---------------------------------------------------------------------------
  // Page header API (for the screen renderers).
  //
  // The 56px header above the scrolling content holds the page title, an
  // optional count badge next to it, and an action slot on the right that ends
  // with the CLI switcher island (#project-cli-workspace).
  //
  // renderDashboard() resets the header to the screen's default (title from
  // SCREEN_TITLES, no count, no actions) and then runs the per-screen branch.
  // Call setPageHeader() in that branch, or at any time after render to patch
  // the live header in place without a re-render:
  //
  //   setPageHeader({ title: "CLI Accounts", count: state.adminAccounts.length });
  //   setPageHeader({ actions: '<button class="btn btn-sm" id="my-action" type="button">New</button>' });
  //   setPageHeader({ count: null }); // hides the badge
  //
  //   title    plain text (escaped)
  //   count    number or short text shown as a badge; null/undefined/"" hides it
  //   actions  trusted HTML placed before the CLI switcher; bind its ids in
  //            bindDashboardHandlers() like any other element
  //
  // pageHeaderActionsSlot() returns the element that holds `actions`, e.g. to
  // mount a Leptos island there after render.
  // ---------------------------------------------------------------------------
  const pageHeader = { title: "", count: null, actions: "" };

  const SCREEN_TITLES = {
    agents_skills_mcp: "Agents, Skills & MCP",
    admin: "Users & Projects",
    accounts: "CLI Accounts",
    chat: "Chat & Terminal",
    files: "Files & Git",
    kanban: "Kanban",
    cron: "Cron",
    none: "No Project Selected",
  };

  function hasPageHeaderCount() {
    return pageHeader.count !== null && pageHeader.count !== undefined && pageHeader.count !== "";
  }

  function setPageHeader(patch) {
    Object.assign(pageHeader, patch);
    const title = document.getElementById("page-title");
    if (!title) return;
    if ("title" in patch) title.textContent = pageHeader.title;
    if ("count" in patch) {
      const badge = document.getElementById("page-title-count");
      badge.textContent = hasPageHeaderCount() ? String(pageHeader.count) : "";
      badge.hidden = !hasPageHeaderCount();
    }
    if ("actions" in patch) pageHeaderActionsSlot().innerHTML = pageHeader.actions || "";
  }

  function pageHeaderActionsSlot() {
    return document.getElementById("page-header-actions-extra");
  }

  function pageHeaderHtml() {
    const count = hasPageHeaderCount();
    return `
      <header class="page-header" id="page-header">
        <div class="page-header-title">
          <h1 id="page-title" class="truncate">${esc(pageHeader.title)}</h1>
          <span class="badge badge-outline" id="page-title-count"${count ? "" : " hidden"}>${count ? esc(pageHeader.count) : ""}</span>
        </div>
        <div class="page-header-actions">
          <div class="page-header-actions-extra" id="page-header-actions-extra">${pageHeader.actions || ""}</div>
          <div id="project-cli-workspace"></div>
        </div>
      </header>
    `;
  }

  function sidebarHtml(activeProject, isAdmin) {
    const projectNav = activeProject ? `
          <div class="nav-group">
            ${navItem("proj-tab-chat", "Chat &amp; Terminal", "chat", state.tab === "overview" && state.projectTab === "chat")}
            ${navItem("proj-tab-files", "Files &amp; Git", "files", state.tab === "overview" && state.projectTab === "files")}
            ${navItem("proj-tab-kanban", "Kanban", "kanban", state.tab === "overview" && state.projectTab === "kanban")}
            ${navItem("proj-tab-cron", "Cron", "cron", state.tab === "overview" && state.projectTab === "cron")}
          </div>` : "";
    const projectName = activeProject ? esc(activeProject.name) : "Select project";
    const signOut = `Sign Out (${esc(state.user.username)})`;
    return `
      <aside class="sidebar" id="sidebar" aria-label="Navigation">
        <div class="sidebar-header">
          <div class="brand">
            <span class="brand-name">Claude Web UI</span>
          </div>
          <button class="btn-ghost btn-icon btn-sm sidebar-collapse-btn" id="sidebar-collapse-btn" type="button" aria-controls="sidebar">${icon("rail")}</button>
          <button class="btn-ghost btn-icon btn-sm sidebar-close-btn" id="sidebar-close-btn" type="button" aria-label="Close navigation" title="Close navigation">${icon("close")}</button>
        </div>
        <details class="project-switcher">
          <summary class="project-switcher-trigger" title="Project: ${projectName}">
            ${icon("folder")}
            <span class="project-switcher-text">
              <span class="project-switcher-label">Project</span>
              <span class="project-switcher-name truncate">${projectName}</span>
            </span>
            <span class="project-switcher-chevron">${icon("chevron")}</span>
          </summary>
          <div class="project-switcher-menu">
            <div id="project-list"></div>
          </div>
        </details>
        <nav class="sidebar-nav">
          ${projectNav}
          <div class="nav-group">
            <h3 class="nav-group-heading">Workspace</h3>
            ${navItem("nav-asm", "Agents, Skills &amp; MCP", "agents", state.tab === "agents_skills_mcp")}
            ${isAdmin ? navItem("nav-admin", "Admin: Users &amp; Projects", "users", state.tab === "admin") : ""}
            ${isAdmin ? navItem("nav-accounts", "Admin: CLI Accounts", "key", state.tab === "accounts") : ""}
          </div>
        </nav>
        <div class="sidebar-system">
          <h3 class="nav-group-heading">System</h3>
          <div class="system-status" id="gateway-status" data-status="${shell.gatewayStatus}">
            <span class="status-dot" aria-hidden="true"></span>
            <span class="system-status-label">Gateway</span>
            <span class="system-status-value">Checking</span>
          </div>
          <button class="nav-item" id="pair-messenger-btn" type="button" title="Pair Telegram or Discord">${icon("link")}<span class="nav-label">Pair Telegram or Discord</span></button>
          <p class="subtitle sidebar-pair-result" id="pair-messenger-result" aria-live="polite">${esc(state.messengerPairing || "")}</p>
          <button class="nav-item" id="logout-btn" type="button" title="${signOut}">${icon("logout")}<span class="nav-label">${signOut}</span></button>
          <div class="sidebar-version">
            <span id="gateway-version">${shell.version ? `v${esc(shell.version)}` : ""}</span>
            <span class="role-badge" title="Your role">${esc(state.user.role)}</span>
          </div>
        </div>
      </aside>
    `;
  }

  function bindShellHandlers() {
    document.getElementById("sidebar-collapse-btn").onclick = () => {
      shell.collapsed = !shell.collapsed;
      writeSidebarCollapsed(shell.collapsed);
      applyShellClasses();
    };
    document.getElementById("sidebar-open-btn").onclick = () => setDrawerOpen(true);
    document.getElementById("sidebar-close-btn").onclick = () => setDrawerOpen(false);
    document.getElementById("sidebar-scrim").onclick = () => setDrawerOpen(false);
    applyShellClasses();
    renderGatewayStatus();
    startGatewayStatus();
  }

  function renderDashboard() {
    closeActivePty();
    const activeProject = state.projects.find((p) => p.id === state.activeProjectId) || null;
    const isAdmin = state.user.role === "admin";
    const errHtml = state.error ? `<div class="alert-error">${esc(state.error)}</div>` : "";
    const okHtml = state.notice ? `<div class="alert-success">${esc(state.notice)}</div>` : "";

    // The screen being shown, derived from the two-level navigation state.
    const screen = (state.tab === "admin" || state.tab === "accounts") && isAdmin ? state.tab
      : state.tab === "agents_skills_mcp" ? state.tab
      : !activeProject ? "none"
      : state.projectTab;
    // Header defaults for the screen; the branches below may call setPageHeader().
    Object.assign(pageHeader, { title: SCREEN_TITLES[screen] || "", count: null, actions: "" });

    let mainHtml = "";
    if (screen === "admin") {
      mainHtml = '<div id="admin-workspace"></div>';
    } else if (screen === "accounts") {
      mainHtml = '<div id="accounts-workspace"></div>';
      setPageHeader({
        count: state.adminAccounts.length,
        actions: '<button class="btn btn-sm" id="open-add-account-btn" type="button">+ Add Account</button>',
      });
    } else if (screen === "agents_skills_mcp") {
      mainHtml = '<div id="asm-workspace"></div>';
    } else if (screen === "none") {
      mainHtml = `
        <section class="panel-card">
          <h2>No Project Selected</h2>
          <p class="subtitle">Ask an Admin to assign you to a project workspace.</p>
        </section>
      `;
    } else {
      if (screen === "chat") {
        pageHeader.actions = '<button class="btn-outline btn-sm" id="chat-rail-toggle" type="button" aria-controls="chat-rail" aria-expanded="false">Sessions</button>';
      }
      mainHtml = screen === "files" ? '<div id="files-git-workspace"></div>' : screen === "kanban" ? '<div id="kanban-workspace"></div>' : screen === "cron" ? '<div id="cron-workspace"></div>' : '<div id="chat-workspace-host"></div>';
    }

    app.innerHTML = `
      <div class="dashboard-shell" data-screen="${esc(screen)}">
        ${sidebarHtml(activeProject, isAdmin)}
        <button class="sidebar-scrim" id="sidebar-scrim" type="button" tabindex="-1" aria-label="Close navigation"></button>
        <div class="dashboard-column">
          <header class="mobile-header">
            <button class="btn-ghost btn-icon btn-sm" id="sidebar-open-btn" type="button" aria-controls="sidebar" aria-label="Open navigation" title="Open navigation">${icon("menu")}</button>
            <span class="brand-name">Claude Web UI</span>
          </header>
          ${pageHeaderHtml()}
          <main class="dashboard-main">
            ${errHtml}${okHtml}${mainHtml}
          </main>
        </div>
      </div>
      <div id="account-modal-host"></div>
    `;

    leptos.mount_project_list(document.getElementById("project-list"), JSON.stringify(state.projects), state.activeProjectId);
    leptos.mount_cli_switcher(document.getElementById("project-cli-workspace"), JSON.stringify(state.projectCli));
    const cron = document.getElementById("cron-workspace");
    if (cron) leptos.mount_cron(cron, state.activeProjectId, state.accessToken || "");
    const kanban = document.getElementById("kanban-workspace");
    if (kanban) leptos.mount_kanban(kanban, state.activeProjectId, state.accessToken || "", state.user.role === "viewer");
    const files = document.getElementById("files-git-workspace");
    if (files) {
      leptos.mount_files_git(files, JSON.stringify({
        project_name: activeProject.name,
        workspace_path: activeProject.workspace_path,
        read_only: state.user.role === "viewer",
        respect_gitignore: state.respectGitignore,
        file_tree: state.fileTree,
        collapsed_dirs: state.collapsedDirs,
        active_file: state.activeFile,
        editor_text: state.editorText,
        editor_dirty: state.editorDirty,
        git_status: state.gitStatus,
        git_diff: state.gitDiff,
        diff_mode: state.diffMode,
        diff_path: state.diffPath,
      }));
    }
    const chat = document.getElementById("chat-workspace-host");
    if (chat && activeProject) {
      const isViewer = state.user.role === "viewer";
      const detail = state.activeSessionDetail;
      const activeSess = detail ? detail.session : state.sessions.find((s) => s.id === state.activeSessionId) || null;
      const selectedProvider = state.projectCli?.active_provider;
      const nativeSource = displaysNativeTranscript(detail);
      leptos.mount_chat_workspace(chat, JSON.stringify({
        project_name: activeProject.name,
        workspace_path: activeProject.workspace_path,
        is_viewer: isViewer,
        sessions: state.sessions,
        active_session_id: state.activeSessionId,
        session_search: state.sessionSearch || "",
        handoff_provider: !isViewer && activeSess?.message_count && activeSess.provider !== selectedProvider ? selectedProvider : null,
        has_active_session: Boolean(activeSess),
        approval_mode: activeSess ? activeSess.approval_mode : "full_auto",
        has_active_turn: Boolean(state.schedulerStatus && state.schedulerStatus.active_session_id === state.activeSessionId),
        show_terminal: Boolean(state.showProjectTerminal),
        can_switch_source: Boolean(detail?.session.transcript_path && detail.turns?.length),
        native_source: nativeSource,
        native_messages: nativeSource ? detail.native_messages || [] : [],
        turns: !nativeSource && detail?.turns ? detail.turns : [],
      }));
    }
    applyChatRailState();
    const accounts = document.getElementById("accounts-workspace");
    if (accounts) {
      leptos.mount_accounts_admin(accounts, JSON.stringify({
        accounts: state.adminAccounts,
        projects: state.projects,
        acl_project_id: state.aclProjectId,
        acl_members: state.aclMembers,
        acl_list: state.aclList,
      }));
    }
    const modal = state.addAccountModal;
    if (modal) {
      const account = modal.step === "create" ? null : (state.adminAccounts.find((a) => a.id === modal.accountId) || modal.account);
      leptos.mount_account_modal(document.getElementById("account-modal-host"), JSON.stringify({ step: modal.step, tab: modal.tab || "pty", account }));
    }
    const asm = document.getElementById("asm-workspace");
    if (asm) {
      leptos.mount_agents_skills_mcp(asm, JSON.stringify({
        provider: state.asmProvider,
        scope: state.asmScope,
        sub_tab: state.asmSubTab,
        has_project: Boolean(state.activeProjectId),
        can_edit: asmCanMutate(),
        raw_mode: state.asmRawMode,
        agents: state.asmAgents,
        selected_agent: state.asmSelectedAgent,
        skills: state.asmSkills,
        selected_skill: state.asmSelectedSkill,
        mcp_config: state.asmMcpConfig,
        presets: state.asmPresets,
        graphify_result: state.asmGraphifyResult,
      }));
    }
    const admin = document.getElementById("admin-workspace");
    if (admin) leptos.mount_admin(admin, JSON.stringify(state.adminUsers), JSON.stringify(state.projects), JSON.stringify(state.adminInvites));
    bindShellHandlers();
    bindDashboardHandlers();
  }

  // Terminal appearance, mirroring the hermes-agent dashboard terminal.
  // xterm.js takes literal colors, so these hex values are the one deliberate exception
  // to "no hard-coded colors". Keep every terminal color in this block.
  const TERMINAL_COLORS = {
    background: "#000000",
    foreground: "#f0e6d2",
    selectionAlpha: "44", // ~27% alpha, appended to the foreground hex
  };
  const TERMINAL_FONT_FACE = "JetBrains Mono"; // self-hosted, see @font-face in styles.css
  const TERMINAL_FONT_FAMILY = `'${TERMINAL_FONT_FACE}', 'Cascadia Mono', 'Fira Code', 'Source Code Pro', Menlo, Consolas, 'DejaVu Sans Mono', monospace`;
  const TERMINAL_SCROLLBACK = 5000;
  const TERMINAL_FONT_WAIT_MS = 2000;
  const FALLBACK_PTY_COLS = 90;
  const FALLBACK_PTY_ROWS = 24;

  function terminalTheme() {
    const { background, foreground, selectionAlpha } = TERMINAL_COLORS;
    return {
      background,
      foreground,
      cursor: foreground,
      cursorAccent: background,
      selectionBackground: `${foreground}${selectionAlpha}`,
    };
  }

  // Width used to pick the font size step: the host's width, or the viewport while the host is not laid out yet.
  function terminalTierWidth(host) {
    const w = Math.round(host.clientWidth);
    if (w > 2) return w;
    const vv = window.visualViewport ? window.visualViewport.width : Infinity;
    return Math.round(Math.min(window.innerWidth, vv, document.documentElement.clientWidth || Infinity));
  }

  function terminalFontSizeForWidth(w) {
    if (w < 300) return 7;
    if (w < 360) return 8;
    if (w < 420) return 9;
    if (w < 520) return 10;
    if (w < 720) return 11;
    if (w < 1024) return 12;
    return 14;
  }

  function terminalLineHeightForWidth(w) {
    return w < 1024 ? 1.02 : 1.15;
  }

  // Resolves once the terminal web font is usable (or after a timeout), so fit() measures the real cell size.
  function whenTerminalFontReady() {
    if (!document.fonts || typeof document.fonts.load !== "function") return Promise.resolve();
    const load = Promise.all([
      document.fonts.load(`400 1em '${TERMINAL_FONT_FACE}'`),
      document.fonts.load(`700 1em '${TERMINAL_FONT_FACE}'`),
    ]).catch(() => {});
    const timeout = new Promise((resolve) => setTimeout(resolve, TERMINAL_FONT_WAIT_MS));
    return Promise.race([load, timeout]);
  }

  function attachPtyTerminal(hostId, fallbackLogId, fallbackFormId, fallbackInputId, wsPath) {
    closeActivePty();
    const host = document.getElementById(hostId);
    if (!host) return;
    const fallbackLog = document.getElementById(fallbackLogId);
    const fallbackForm = document.getElementById(fallbackFormId);

    if (typeof window.Terminal !== "function") {
      if (fallbackLog && fallbackForm) {
        fallbackLog.textContent = "Connecting to PTY...\n";
        fallbackForm.style.display = "flex";
      }
      const ws = openPtySocket(wsPath, FALLBACK_PTY_COLS, FALLBACK_PTY_ROWS, null, fallbackLog);
      if (fallbackForm) {
        fallbackForm.onsubmit = (e) => {
          e.preventDefault();
          const inp = document.getElementById(fallbackInputId);
          if (inp && ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: "input", data: inp.value + "\r" }));
            inp.value = "";
          }
        };
      }
      return;
    }

    // xterm opens into an unpadded box filling the host, so the fit addon measures the real space.
    host.innerHTML = "";
    const box = document.createElement("div");
    box.className = "xterm-fit-box";
    host.appendChild(box);

    const tierWidth = terminalTierWidth(host);
    const term = new window.Terminal({
      allowProposedApi: true, // required by the unicode11 addon
      cursorBlink: true,
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: terminalFontSizeForWidth(tierWidth),
      lineHeight: terminalLineHeightForWidth(tierWidth),
      letterSpacing: 0,
      fontWeight: "400",
      fontWeightBold: "700",
      macOptionIsMeta: true,
      rightClickSelectsWord: true,
      scrollback: TERMINAL_SCROLLBACK,
      theme: terminalTheme(),
    });
    const fit = window.FitAddon && typeof window.FitAddon.FitAddon === "function"
      ? new window.FitAddon.FitAddon()
      : null;
    if (fit) term.loadAddon(fit);
    if (window.Unicode11Addon && typeof window.Unicode11Addon.Unicode11Addon === "function") {
      term.loadAddon(new window.Unicode11Addon.Unicode11Addon());
      term.unicode.activeVersion = "11";
    }
    if (window.WebLinksAddon && typeof window.WebLinksAddon.WebLinksAddon === "function") {
      term.loadAddon(new window.WebLinksAddon.WebLinksAddon());
    }
    term.open(box);
    state.ptyTerminal = term;
    term.focus();
    setPtyLive(true);

    const focusTerm = () => term.focus();
    host.addEventListener("click", focusTerm);
    ptyCleanups.push(() => host.removeEventListener("click", focusTerm));

    // Re-pick font size/line height for the host width, then fit cols/rows to the host.
    // term.onResize (below) forwards any grid change to the PTY as a `resize` frame.
    const syncMetrics = () => {
      if (state.ptyTerminal !== term) return;
      // A hidden or detached host would make fit() collapse the grid to 1x1.
      if (!host.isConnected || host.clientWidth <= 0 || host.clientHeight <= 0) return;
      const w = terminalTierWidth(host);
      const fontSize = terminalFontSizeForWidth(w);
      const lineHeight = terminalLineHeightForWidth(w);
      if (term.options.fontSize !== fontSize) term.options.fontSize = fontSize;
      if (term.options.lineHeight !== lineHeight) term.options.lineHeight = lineHeight;
      if (fit) {
        try { fit.fit(); } catch {}
      }
    };

    if (typeof ResizeObserver === "function") {
      let frame = 0;
      const observer = new ResizeObserver(() => {
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(syncMetrics);
      });
      observer.observe(host);
      ptyCleanups.push(() => {
        cancelAnimationFrame(frame);
        observer.disconnect();
      });
    }

    let ws = null;
    term.onData((data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "input", data }));
      }
    });
    term.onResize((sz) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "resize", cols: sz.cols, rows: sz.rows }));
      }
    });

    // Connect only once the web font is in, so the CLI's first frame is laid out for the measured size.
    whenTerminalFontReady().then(() => {
      if (state.ptyTerminal !== term) return; // disposed by render() or a restart while waiting
      // open() measured the cell with a fallback face; changing the family forces xterm to re-measure.
      term.options.fontFamily = "monospace";
      term.options.fontFamily = TERMINAL_FONT_FAMILY;
      if (typeof term.clearTextureAtlas === "function") term.clearTextureAtlas();
      syncMetrics();
      ws = openPtySocket(wsPath, term.cols, term.rows, term, null);
    });
  }

  // Opens the PTY WebSocket at the given size and wires output to the terminal (or the fallback log).
  function openPtySocket(wsPath, cols, rows, term, fallbackLog) {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${window.location.host}${wsPath}&cols=${cols}&rows=${rows}`);
    state.ptySocket = ws;
    setPtyLive(true);
    const markEnded = () => { if (state.ptySocket === ws) setPtyLive(false); };
    ws.onclose = markEnded;
    ws.onerror = markEnded;
    ws.onopen = () => {
      // The grid may have been re-fitted between connecting and opening.
      if (term && (term.cols !== cols || term.rows !== rows)) {
        ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      }
    };

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.type === "output") {
          if (term) term.write(msg.data);
          else if (fallbackLog) fallbackLog.innerHTML += linkifyUrls(msg.data);
        } else if (msg.type === "exit") {
          const note = `\r\n[Process exited with code ${msg.code}]\r\n`;
          markEnded();
          if (term) term.write(note);
          else if (fallbackLog) fallbackLog.innerHTML += esc(note);
        } else if (msg.type === "error") {
          const note = `\r\n[PTY error: ${msg.message}]\r\n`;
          if (term) term.write(note);
          else if (fallbackLog) fallbackLog.innerHTML += esc(note);
        }
      } catch {}
    };
    return ws;
  }

  function connectOAuthTerminal(accountId) {
    const wsPath = `/api/admin/accounts/${encodeURIComponent(accountId)}/login-pty?token=${encodeURIComponent(state.accessToken)}`;
    attachPtyTerminal("oauth-xterm-host", "oauth-xterm-fallback", "oauth-fallback-input-form", "oauth-fallback-input", wsPath);
  }

  function connectProjectTerminal(mode) {
    if (!state.activeProjectId) return;
    const sidParam = mode === "cli" && state.activeSessionId ? `&session_id=${encodeURIComponent(state.activeSessionId)}` : "";
    const wsPath = `/api/projects/${encodeURIComponent(state.activeProjectId)}/terminal/ws?token=${encodeURIComponent(state.accessToken)}&mode=${encodeURIComponent(mode)}${sidParam}`;
    attachPtyTerminal("project-xterm-host", "project-xterm-fallback", "project-xterm-fallback-form", "project-xterm-fallback-input", wsPath);
  }

  function asmBasePath() {
    if (state.asmScope === "global") return "/api/global";
    if (!state.activeProjectId) return null;
    return `/api/projects/${state.activeProjectId}`;
  }

  function asmCanMutate() {
    if (!state.user) return false;
    if (state.asmScope === "global") return state.user.role === "admin";
    return state.user.role !== "viewer" && Boolean(state.activeProjectId);
  }

  async function loadAsmData() {
    const base = asmBasePath();
    if (!base) {
      state.asmAgents = [];
      state.asmSkills = [];
      state.asmMcpConfig = null;
      return;
    }
    const prov = state.asmProvider;
    try {
      const [agents, skills, mcpCfg] = await Promise.all([
        api(`${base}/agents/${prov}`),
        api(`${base}/skills/${prov}`),
        api(`${base}/mcp/${prov}`),
      ]);
      state.asmAgents = agents || [];
      state.asmSkills = skills || [];
      state.asmMcpConfig = mcpCfg || null;
    } catch (err) {
      state.error = err.message;
    }
  }

  function bindDashboardHandlers() {
    // Project sections live in the sidebar next to the workspace screens, so
    // picking one also returns the top-level tab to the project view.
    const kanbanTab = document.getElementById("proj-tab-kanban");
    if (kanbanTab) kanbanTab.onclick = () => { state.tab = "overview"; state.projectTab = "kanban"; render(); };
    const projTabChat = document.getElementById("proj-tab-chat");
    if (projTabChat) projTabChat.onclick = () => { state.tab = "overview"; state.projectTab = "chat"; render(); };

    const projTabCron = document.getElementById("proj-tab-cron");
    if (projTabCron) projTabCron.onclick = () => { state.tab = "overview"; state.projectTab = "cron"; render(); };

    const projTabFiles = document.getElementById("proj-tab-files");
    if (projTabFiles) projTabFiles.onclick = () => { state.tab = "overview"; state.projectTab = "files"; render(); };

    const newSessBtn = document.getElementById("new-chat-session-btn");
    if (newSessBtn) {
      newSessBtn.onclick = async () => {
        try {
          const created = await api(`/api/projects/${state.activeProjectId}/sessions`, {
            method: "POST",
            body: JSON.stringify({}),
          });
          state.activeSessionId = created.id;
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const searchInp = document.getElementById("session-search-input");
    if (searchInp) {
      searchInp.onchange = async () => {
        state.sessionSearch = searchInp.value;
        await loadProjectSessions();
        render();
      };
    }

    const navAsm = document.getElementById("nav-asm");
    if (navAsm) {
      navAsm.onclick = async () => {
        state.tab = "agents_skills_mcp";
        state.error = null; state.notice = null;
        await loadAsmData();
        render();
      };
    }

    const toggleGi = document.getElementById("toggle-gitignore");
    if (toggleGi) {
      toggleGi.onchange = async () => {
        state.respectGitignore = toggleGi.checked;
        await loadProjectWorkspace();
        render();
      };
    }

    const handoff = document.getElementById("chat-handoff");
    if (handoff) {
      handoff.onclick = async () => {
        const projectId = state.activeProjectId;
        const sourceId = state.activeSessionId;
        handoff.disabled = true;
        state.error = null;
        try {
          const continuation = await api(`/api/projects/${encodeURIComponent(projectId)}/sessions/${encodeURIComponent(sourceId)}/handoff`, {
            method: "POST",
            body: JSON.stringify({ provider: handoff.dataset.provider }),
          });
          if (state.activeProjectId !== projectId || state.activeSessionId !== sourceId) return;
          state.activeSessionId = continuation.id;
          state.sessionSearch = "";
          await loadProjectSessions();
        } catch (error) {
          state.error = error.message;
        }
        render();
      };
    }

    const chatSource = document.getElementById("chat-feed-source");
    if (chatSource) {
      chatSource.onclick = () => {
        state.nativeTranscriptSessionId = displaysNativeTranscript(state.activeSessionDetail) ? null : state.activeSessionId;
        connectNativeTranscript();
        render();
      };
    }

    const railToggle = document.getElementById("chat-rail-toggle");
    if (railToggle) railToggle.onclick = () => setChatRailOpen(!chatRailOpen);
    const railScrim = document.getElementById("chat-rail-scrim");
    if (railScrim) railScrim.onclick = () => setChatRailOpen(false);
    const railClose = document.getElementById("chat-rail-close");
    if (railClose) railClose.onclick = () => setChatRailOpen(false);

    document.querySelectorAll("[data-sid]").forEach((btn) => {
      btn.onclick = async () => {
        chatRailOpen = false;
        state.activeSessionId = btn.dataset.sid;
        await loadProjectSessions();
        render();
      };
    });

    const refreshWs = document.getElementById("refresh-workspace-btn");
    if (refreshWs) {
      refreshWs.onclick = async () => {
        await loadProjectWorkspace();
        render();
      };
    }

    document.querySelectorAll("[data-dir-toggle]").forEach((btn) => {
      btn.onclick = () => {
        const p = btn.dataset.dirToggle;
        state.collapsedDirs[p] = !state.collapsedDirs[p];
        render();
      };
    });

    const toggleApprovalBtn = document.getElementById("toggle-approval-mode-btn");
    if (toggleApprovalBtn && state.activeSessionDetail) {
      toggleApprovalBtn.onclick = async () => {
        const cur = state.activeSessionDetail.session.approval_mode;
        const next = cur === "interactive" ? "full_auto" : "interactive";
        try {
          await api(`/api/projects/${state.activeProjectId}/sessions/${state.activeSessionId}`, {
            method: "PATCH",
            body: JSON.stringify({ approval_mode: next }),
          });
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const toggleTermBtn = document.getElementById("toggle-project-terminal-btn");
    if (toggleTermBtn) {
      toggleTermBtn.onclick = () => {
        state.showProjectTerminal = !state.showProjectTerminal;
        render();
        if (state.showProjectTerminal) {
          connectProjectTerminal(state.terminalMode);
        }
      };
    }

    document.querySelectorAll("[data-file-open]").forEach((btn) => {
      btn.onclick = () => openWorkspaceFile(btn.dataset.fileOpen);
    });

    const editorEl = document.getElementById("code-editor");
    if (editorEl) {
      editorEl.oninput = () => {
        state.editorText = editorEl.value;
        state.editorDirty = true;
      };
    }

    const saveBtn = document.getElementById("save-file-btn");
    if (saveBtn) {
      saveBtn.onclick = () => saveWorkspaceFile();
    }

    const newFileForm = document.getElementById("new-file-form");
    if (newFileForm) {
      newFileForm.onsubmit = async (e) => {
        e.preventDefault();
        const path = newFileForm.path.value.trim();
        if (!path) return;
        const pid = encodeURIComponent(state.activeProjectId);
        try {
          await api(`/api/projects/${pid}/files/content`, {
            method: "POST",
            body: JSON.stringify({ path, content: "" }),
          });
          await loadProjectWorkspace();
          await openWorkspaceFile(path);
        } catch (err) {
          state.error = err.message;
          render();
        }
      };
    }

    const closeTermBtn = document.getElementById("close-project-term");
    if (closeTermBtn) {
      closeTermBtn.onclick = () => { state.showProjectTerminal = false; render(); };
    }

    const shellModeBtn = document.getElementById("term-mode-shell");
    if (shellModeBtn) {
      shellModeBtn.onclick = () => { state.terminalMode = "shell"; connectProjectTerminal("shell"); };
    }

    const cliModeBtn = document.getElementById("term-mode-cli");
    if (cliModeBtn) {
      cliModeBtn.onclick = () => { state.terminalMode = "cli"; connectProjectTerminal("cli"); };
    }

    const stopBtn = document.getElementById("stop-active-turn-btn");
    if (stopBtn && state.activeSessionId) {
      stopBtn.onclick = async () => {
        try {
          await api(`/api/projects/${state.activeProjectId}/sessions/${state.activeSessionId}/stop`, {
            method: "POST",
          });
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-allow-turn]").forEach((btn) => {
      btn.onclick = async () => {
        try {
          await api(`/api/projects/${state.activeProjectId}/turns/${btn.dataset.allowTurn}/permission`, {
            method: "POST",
            body: JSON.stringify({ approved: true, request_id: btn.dataset.reqId }),
          });
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-deny-turn]").forEach((btn) => {
      btn.onclick = async () => {
        try {
          await api(`/api/projects/${state.activeProjectId}/turns/${btn.dataset.denyTurn}/permission`, {
            method: "POST",
            body: JSON.stringify({ approved: false, request_id: btn.dataset.reqId }),
          });
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    const promptForm = document.getElementById("chat-prompt-form");
    if (promptForm) {
      promptForm.onsubmit = async (e) => {
        e.preventDefault();
        const input = document.getElementById("chat-prompt-input");
        const prompt = input ? input.value.trim() : "";
        if (!prompt) return;
        try {
          if (!state.activeSessionId) {
            const created = await api(`/api/projects/${state.activeProjectId}/sessions`, {
              method: "POST",
              body: JSON.stringify({}),
            });
            state.activeSessionId = created.id;
          }
          await api(`/api/projects/${state.activeProjectId}/sessions/${state.activeSessionId}/turns`, {
            method: "POST",
            body: JSON.stringify({ prompt }),
          });
          if (input) input.value = "";
          await loadProjectSessions();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-diff-path]").forEach((btn) => {
      btn.onclick = () => loadGitDiff(btn.dataset.diffPath, btn.dataset.diffStaged === "true");
    });

    const diffAllBtn = document.getElementById("diff-all-btn");
    if (diffAllBtn) {
      diffAllBtn.onclick = () => loadGitDiff(null, false);
    }

    const toggleDiffModeBtn = document.getElementById("toggle-diff-mode-btn");
    if (toggleDiffModeBtn) {
      toggleDiffModeBtn.onclick = () => {
        state.diffMode = state.diffMode === "unified" ? "split" : "unified";
        loadGitDiff(state.diffPath, state.diffStaged);
      };
    }

    const navAdmin = document.getElementById("nav-admin");
    if (navAdmin) navAdmin.onclick = () => { state.tab = "admin"; render(); };

    const navAccounts = document.getElementById("nav-accounts");
    if (navAccounts) navAccounts.onclick = () => { state.tab = "accounts"; render(); };

    const pairMessengerButton = document.getElementById("pair-messenger-btn");
    if (pairMessengerButton) pairMessengerButton.onclick = async () => {
      const userId = state.user.id;
      pairMessengerButton.disabled = true;
      try {
        const pairing = await api("/api/messenger/pairing-code", { method: "POST" });
        if (state.user?.id !== userId) return;
        state.messengerPairing = `Send /pair ${pairing.code} in a bot DM within ${Math.ceil(pairing.expires_in_secs / 60)} minutes.`;
        setTimeout(() => {
          if (!state.messengerPairing?.includes(`/pair ${pairing.code} `)) return;
          state.messengerPairing = null;
          const result = document.getElementById("pair-messenger-result");
          if (result) result.textContent = "";
        }, pairing.expires_in_secs * 1000);
      } catch (err) {
        if (state.user?.id !== userId) return;
        state.messengerPairing = err.message;
      } finally {
        const result = document.getElementById("pair-messenger-result");
        if (result) result.textContent = state.messengerPairing || "";
        pairMessengerButton.disabled = false;
      }
    };

    document.getElementById("logout-btn").onclick = async () => {
      try { await api("/api/auth/logout", { method: "POST" }); } catch {}
      clearSession();
      state.screen = "login";
      render();
    };

    const openModalBtn = document.getElementById("open-add-account-btn");
    if (openModalBtn) {
      openModalBtn.onclick = () => {
        state.addAccountModal = { step: "create", tab: "pty", accountId: null, account: null };
        render();
      };
    }

    const closeModalBtn = document.getElementById("close-account-modal");
    if (closeModalBtn) {
      closeModalBtn.onclick = () => {
        state.addAccountModal = null;
        loadAuthenticatedData();
      };
    }

    const createAccForm = document.getElementById("create-account-form");
    if (createAccForm) {
      createAccForm.onsubmit = async (e) => {
        e.preventDefault();
        state.error = null;
        const fd = new FormData(e.target);
        try {
          const acc = await api("/api/admin/accounts", {
            method: "POST",
            body: JSON.stringify(Object.fromEntries(fd.entries())),
          });
          state.adminAccounts.push(acc);
          state.addAccountModal = { step: "auth", tab: "pty", accountId: acc.id, account: acc };
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const tabPty = document.getElementById("modal-tab-pty");
    if (tabPty) tabPty.onclick = () => { state.addAccountModal.tab = "pty"; render(); };

    const tabCreds = document.getElementById("modal-tab-creds");
    if (tabCreds) tabCreds.onclick = () => { state.addAccountModal.tab = "credentials"; render(); };

    const launchPtyBtn = document.getElementById("launch-oauth-pty-btn");
    if (launchPtyBtn && state.addAccountModal?.accountId) {
      launchPtyBtn.onclick = () => connectOAuthTerminal(state.addAccountModal.accountId);
    }

    const credPicker = document.getElementById("cred-file-picker");
    if (credPicker) {
      credPicker.onchange = async () => {
        const file = credPicker.files && credPicker.files[0];
        if (!file) return;
        document.getElementById("cred-filename").value = file.name;
        document.getElementById("cred-content").value = await file.text();
      };
    }

    const uploadCredForm = document.getElementById("upload-cred-form");
    if (uploadCredForm && state.addAccountModal?.accountId) {
      uploadCredForm.onsubmit = async (e) => {
        e.preventDefault();
        state.error = null; state.notice = null;
        const filename = document.getElementById("cred-filename").value;
        const content = document.getElementById("cred-content").value;
        try {
          await api(`/api/admin/accounts/${state.addAccountModal.accountId}/credentials`, {
            method: "POST",
            body: JSON.stringify({ filename, content }),
          });
          state.notice = `Saved ${filename}`;
          state.adminAccounts = await api("/api/admin/accounts");
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-del-cred]").forEach((btn) => {
      btn.onclick = async () => {
        state.error = null;
        try {
          await api(`/api/admin/accounts/${state.addAccountModal.accountId}/credentials/${encodeURIComponent(btn.dataset.delCred)}`, {
            method: "DELETE",
          });
          state.adminAccounts = await api("/api/admin/accounts");
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-auth-acc]").forEach((btn) => {
      btn.onclick = () => {
        const acc = state.adminAccounts.find((a) => a.id === btn.dataset.authAcc);
        if (acc) {
          state.addAccountModal = { step: "auth", tab: "pty", accountId: acc.id, account: acc };
          render();
        }
      };
    });

    document.querySelectorAll("[data-rename-acc]").forEach((btn) => {
      btn.onclick = async () => {
        const acc = state.adminAccounts.find((a) => a.id === btn.dataset.renameAcc);
        const nextName = window.prompt("New account name:", acc ? acc.name : "");
        if (!nextName) return;
        try {
          await api(`/api/admin/accounts/${btn.dataset.renameAcc}`, {
            method: "PATCH",
            body: JSON.stringify({ name: nextName }),
          });
          await loadAuthenticatedData();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-del-acc]").forEach((btn) => {
      btn.onclick = async () => {
        try {
          await api(`/api/admin/accounts/${btn.dataset.delAcc}`, { method: "DELETE" });
          await loadAuthenticatedData();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    const aclProjSel = document.getElementById("acl-project-select");
    if (aclProjSel) {
      aclProjSel.onchange = async () => {
        state.aclProjectId = aclProjSel.value;
        await loadAclProjectData(state.aclProjectId);
        render();
      };
    }

    document.querySelectorAll("[data-save-acl]").forEach((btn) => {
      btn.onclick = async () => {
        const uid = btn.dataset.saveAcl;
        const unrestrictedBox = document.querySelector(`[data-acl-unrestricted="${uid}"]`);
        const isUnrestricted = unrestrictedBox && unrestrictedBox.checked;
        const allowed_accounts = isUnrestricted
          ? null
          : Array.from(document.querySelectorAll(`[data-acl-acc="${uid}"]:checked`)).map((el) => el.value);
        try {
          await api(`/api/admin/projects/${state.aclProjectId}/members/${uid}/accounts`, {
            method: "PUT",
            body: JSON.stringify({ allowed_accounts }),
          });
          state.notice = "Updated Developer account ACL.";
          await loadAclProjectData(state.aclProjectId);
          await loadProjectCli();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-asm-scope]").forEach((btn) => {
      btn.onclick = async () => {
        state.asmScope = btn.dataset.asmScope;
        state.asmSelectedAgent = null;
        state.asmSelectedSkill = null;
        await loadAsmData();
        render();
      };
    });

    document.querySelectorAll("[data-asm-prov]").forEach((btn) => {
      btn.onclick = async () => {
        state.asmProvider = btn.dataset.asmProv;
        state.asmSelectedAgent = null;
        state.asmSelectedSkill = null;
        await loadAsmData();
        render();
      };
    });

    document.querySelectorAll("[data-asm-sub]").forEach((btn) => {
      btn.onclick = () => {
        state.asmSubTab = btn.dataset.asmSub;
        render();
      };
    });

    const toggleRawBtn = document.getElementById("asm-toggle-raw-btn");
    if (toggleRawBtn) {
      toggleRawBtn.onclick = () => {
        state.asmRawMode = !state.asmRawMode;
        render();
      };
    }

    document.querySelectorAll("[data-asm-edit-agent]").forEach((btn) => {
      btn.onclick = () => {
        state.asmSelectedAgent = state.asmAgents.find((a) => a.name === btn.dataset.asmEditAgent) || null;
        render();
      };
    });

    document.querySelectorAll("[data-asm-del-agent]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        try {
          await api(`${base}/agents/${state.asmProvider}/${encodeURIComponent(btn.dataset.asmDelAgent)}`, { method: "DELETE" });
          state.asmSelectedAgent = null;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    const asmAgentForm = document.getElementById("asm-agent-form");
    if (asmAgentForm) {
      asmAgentForm.onsubmit = async (e) => {
        e.preventDefault();
        const base = asmBasePath();
        if (!base) return;
        state.error = null; state.notice = null;
        const name = document.getElementById("asm-agent-name").value.trim();
        const payload = state.asmRawMode
          ? { name, raw_markdown: document.getElementById("asm-agent-raw").value }
          : {
              name,
              description: document.getElementById("asm-agent-desc").value,
              model: document.getElementById("asm-agent-model").value,
              tools: Array.from(document.querySelectorAll('input[name="asm_tools"]:checked')).map((el) => el.value),
              disallowed_tools: document.getElementById("asm-agent-disallowed").value.split(",").map((s) => s.trim()).filter(Boolean),
              permission_mode: document.getElementById("asm-agent-perm").value,
              system_prompt: document.getElementById("asm-agent-prompt").value,
            };
        try {
          const saved = await api(`${base}/agents/${state.asmProvider}/${encodeURIComponent(name)}`, {
            method: "PUT",
            body: JSON.stringify(payload),
          });
          state.asmSelectedAgent = saved;
          state.notice = `Saved subagent '${saved.name}'.`;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-asm-edit-skill]").forEach((btn) => {
      btn.onclick = () => {
        state.asmSelectedSkill = state.asmSkills.find((s) => s.name === btn.dataset.asmEditSkill) || null;
        render();
      };
    });

    document.querySelectorAll("[data-asm-share-skill]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        const nextShared = btn.dataset.shared !== "true";
        try {
          const updated = await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(btn.dataset.asmShareSkill)}/share`, {
            method: "PUT",
            body: JSON.stringify({ shared: nextShared }),
          });
          state.asmSelectedSkill = updated;
          state.notice = nextShared ? "Skill shared across CLIs." : "Cross-CLI skill links removed.";
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-asm-del-skill]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        try {
          await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(btn.dataset.asmDelSkill)}`, { method: "DELETE" });
          state.asmSelectedSkill = null;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    const asmSkillForm = document.getElementById("asm-skill-form");
    if (asmSkillForm) {
      asmSkillForm.onsubmit = async (e) => {
        e.preventDefault();
        const base = asmBasePath();
        if (!base) return;
        const name = document.getElementById("asm-skill-name").value.trim();
        const skill_md = document.getElementById("asm-skill-md").value;
        const wantShare = document.getElementById("asm-skill-share-toggle").checked;
        try {
          let saved = await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(name)}`, {
            method: "PUT",
            body: JSON.stringify({ name, skill_md, companion_files: [] }),
          });
          if (saved.is_shared !== wantShare) {
            saved = await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(name)}/share`, {
              method: "PUT",
              body: JSON.stringify({ shared: wantShare }),
            });
          }
          state.asmSelectedSkill = saved;
          state.notice = `Saved skill '${saved.name}'.`;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const asmCompForm = document.getElementById("asm-comp-form");
    if (asmCompForm) {
      asmCompForm.onsubmit = async (e) => {
        e.preventDefault();
        const base = asmBasePath();
        const skillName = document.getElementById("asm-skill-name")?.value?.trim();
        if (!base || !skillName) return;
        try {
          const updated = await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(skillName)}/files`, {
            method: "PUT",
            body: JSON.stringify({
              relative_path: document.getElementById("asm-comp-path").value.trim(),
              content: document.getElementById("asm-comp-content").value,
            }),
          });
          state.asmSelectedSkill = updated;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-asm-del-comp]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        const skillName = state.asmSelectedSkill?.name;
        if (!base || !skillName) return;
        try {
          const updated = await api(`${base}/skills/${state.asmProvider}/${encodeURIComponent(skillName)}/files?path=${encodeURIComponent(btn.dataset.asmDelComp)}`, {
            method: "DELETE",
          });
          state.asmSelectedSkill = updated;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-asm-preset]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        try {
          await api(`${base}/mcp/${state.asmProvider}/presets/${encodeURIComponent(btn.dataset.asmPreset)}`, { method: "POST" });
          state.notice = `Applied preset ${btn.dataset.asmPreset}.`;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-asm-sync-mcp]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        try {
          await api(`${base}/mcp/${state.asmProvider}/${encodeURIComponent(btn.dataset.asmSyncMcp)}/sync`, {
            method: "POST",
            body: JSON.stringify({}),
          });
          state.notice = `Synced '${btn.dataset.asmSyncMcp}' across all CLIs.`;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    document.querySelectorAll("[data-asm-del-mcp]").forEach((btn) => {
      btn.onclick = async () => {
        const base = asmBasePath();
        if (!base) return;
        try {
          await api(`${base}/mcp/${state.asmProvider}/${encodeURIComponent(btn.dataset.asmDelMcp)}`, { method: "DELETE" });
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    });

    const asmMcpForm = document.getElementById("asm-mcp-form");
    if (asmMcpForm) {
      asmMcpForm.onsubmit = async (e) => {
        e.preventDefault();
        const base = asmBasePath();
        if (!base) return;
        const name = document.getElementById("asm-mcp-name").value.trim();
        const command = document.getElementById("asm-mcp-cmd").value.trim() || null;
        const args = document.getElementById("asm-mcp-args").value.split(/\s+/).filter(Boolean);
        try {
          await api(`${base}/mcp/${state.asmProvider}/${encodeURIComponent(name)}`, {
            method: "PUT",
            body: JSON.stringify({ name, command, args, env: {}, url: null, disabled: false }),
          });
          state.notice = `Saved MCP server '${name}'.`;
          await loadAsmData();
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const graphifyBtn = document.getElementById("asm-run-graphify-btn");
    if (graphifyBtn) {
      graphifyBtn.onclick = async () => {
        if (!state.activeProjectId) return;
        state.error = null; state.notice = null;
        try {
          const res = await api(`/api/projects/${state.activeProjectId}/mcp/graphify/index`, { method: "POST" });
          state.asmGraphifyResult = `${res.status}: ${res.graph_path}\n${res.output}`;
          state.notice = "Graphify indexing finished.";
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }
  }

  async function start() {
    try {
      const bindings = await import("/leptos_gateway.js");
      await bindings.default();
      leptos = bindings;
    } catch (error) {
      console.error("Unable to load the Gateway UI", error);
      app.textContent = "Unable to load the Gateway UI. Reload to try again.";
      return;
    }
    await boot();
  }

  void start();
})();
