(() => {
  const PROVIDER_LABELS = {
    claude_code: "Claude Code",
    codex: "Codex",
    antigravity: "Antigravity",
  };

  const PROVIDER_MODEL_TIERS = {
    claude_code: ["opus", "sonnet", "haiku", "inherit"],
    codex: ["gpt-5-codex", "o3", "o4-mini", "codex-mini", "inherit"],
    antigravity: ["pro", "flash", "flash_lite", "inherit"],
  };

  const CLAUDE_TOOLS = [
    "Read", "Edit", "Write", "MultiEdit", "Bash", "Grep", "Glob", "LS",
    "WebFetch", "WebSearch", "Task", "TodoWrite",
  ];

  const PERMISSION_MODES = [
    "default", "acceptEdits", "bypassPermissions", "plan", "dontAsk",
  ];

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

  if (desktop) {
    window.addEventListener("cwu:desktop-token", () => {
      state.accessToken = window.__CWU_DESKTOP_ACCESS_TOKEN;
      if (state.user) {
        connectChatStream(state.activeProjectId);
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

  function closeActivePty() {
    if (state.ptySocket) {
      try { state.ptySocket.close(); } catch {}
      state.ptySocket = null;
    }
    if (state.ptyTerminal) {
      try { state.ptyTerminal.dispose(); } catch {}
      state.ptyTerminal = null;
    }
  }

  function closeChatSocket() {
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
    }
  }

  function connectChatStream(projectId) {
    closeChatSocket();
    if (!projectId || !state.accessToken) return;
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${proto}//${window.location.host}/api/projects/${encodeURIComponent(projectId)}/chat/ws?token=${encodeURIComponent(state.accessToken)}`;
    const ws = new WebSocket(wsUrl);
    state.chatSocket = ws;
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

  function renderTreeNodes(nodes, depth = 0) {
    if (!nodes || nodes.length === 0) return "";
    return `<ul class="file-tree-list">${nodes.map((node) => {
      const pad = depth * 12;
      if (node.is_dir) {
        const collapsed = !!state.collapsedDirs[node.path];
        return `
          <li>
            <button class="tree-node-btn" style="padding-left:${8 + pad}px" data-dir-toggle="${esc(node.path)}">
              ${collapsed ? "Ã¢â€“Â¸ Ã°Å¸â€œÂ" : "Ã¢â€“Â¾ Ã°Å¸â€œâ€š"} ${esc(node.name)}
            </button>
            ${collapsed ? "" : renderTreeNodes(node.children || [], depth + 1)}
          </li>`;
      }
      const isSelected = state.activeFile && state.activeFile.path === node.path;
      return `
        <li>
          <button class="tree-node-btn ${isSelected ? "active" : ""}" style="padding-left:${8 + pad}px" data-file-open="${esc(node.path)}">
            Ã°Å¸â€œâ€ž ${esc(node.name)}
          </button>
        </li>`;
    }).join("")}</ul>`;
  }

  function renderDiffBody() {
    const diff = state.gitDiff;
    if (!diff || !diff.is_git_repo) {
      return `<p class="subtitle">Workspace is not a Git repository.</p>`;
    }
    if (!diff.files || diff.files.length === 0) {
      return `<p class="subtitle">Working tree clean for current selection.</p>`;
    }
    if (state.diffMode === "split") {
      const sections = diff.files.map((file) => {
        const rowsHtml = (file.hunks || []).map((hunk) => {
          const headerRow = `<div class="diff-hunk-header">${esc(hunk.header)}</div>`;
          const splitRows = (hunk.split_rows || []).map((r) => `
            <div class="diff-split-row">
              <div class="diff-cell diff-${esc(r.left_kind)}">
                <span class="diff-ln">${r.left_line ?? ""}</span>
                <code>${esc(r.left_content ?? "")}</code>
              </div>
              <div class="diff-cell diff-${esc(r.right_kind)}">
                <span class="diff-ln">${r.right_line ?? ""}</span>
                <code>${esc(r.right_content ?? "")}</code>
              </div>
            </div>`).join("");
          return headerRow + splitRows;
        }).join("");
        return `<div class="diff-file-block"><div class="diff-file-title">${esc(file.new_path)}</div>${rowsHtml}</div>`;
      }).join("");
      return `<div class="diff-split-container">${sections}</div>`;
    }
    const linesHtml = (diff.unified || "").split("\n").map((line) => {
      let cls = "diff-ctx";
      if (line.startsWith("+") && !line.startsWith("+++")) cls = "diff-add";
      else if (line.startsWith("-") && !line.startsWith("---")) cls = "diff-rem";
      else if (line.startsWith("@@") || line.startsWith("diff --git")) cls = "diff-meta";
      return `<div class="${cls}">${esc(line)}</div>`;
    }).join("");
    return `<pre class="diff-unified">${linesHtml}</pre>`;
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
    if (!state.user) {
      renderAuth();
    } else {
      renderDashboard();
    }
  }

  function renderAuth() {
    const errHtml = state.error ? `<div class="alert-error">${esc(state.error)}</div>` : "";
    if (state.screen === "bootstrap") {
      app.innerHTML = `
        <main class="auth-shell">
          <form class="auth-card" id="bootstrap-form">
            <h2>Initialize Gateway Admin</h2>
            <p class="subtitle">Create the first administrator account to secure this gateway.</p>
            ${errHtml}
            <div class="form-group"><label>Username</label><input name="username" required /></div>
            <div class="form-group"><label>Email</label><input name="email" type="email" required /></div>
            <div class="form-group"><label>Password</label><input name="password" type="password" required /></div>
            <button class="btn-primary" type="submit">Create Admin Account</button>
          </form>
        </main>`;
      document.getElementById("bootstrap-form").onsubmit = async (e) => {
        e.preventDefault();
        state.error = null;
        const fd = new FormData(e.target);
        try {
          const auth = await api("/api/auth/bootstrap", {
            method: "POST",
            body: JSON.stringify(Object.fromEntries(fd.entries())),
          });
          saveTokens(auth);
          await loadAuthenticatedData();
        } catch (err) {
          state.error = err.message;
          render();
        }
      };
      return;
    }

    if (state.screen === "invite") {
      const codeParam = new URLSearchParams(window.location.search).get("invite") || "";
      app.innerHTML = `
        <main class="auth-shell">
          <form class="auth-card" id="invite-form">
            <h2>Redeem Invite</h2>
            <p class="subtitle">Enter your single-use invite code and create your account.</p>
            ${errHtml}
            <div class="form-group"><label>Invite Code</label><input name="code" value="${esc(codeParam)}" required /></div>
            <div class="form-group"><label>Username</label><input name="username" required /></div>
            <div class="form-group"><label>Email</label><input name="email" type="email" required /></div>
            <div class="form-group"><label>Password</label><input name="password" type="password" required /></div>
            <button class="btn-primary" type="submit">Redeem &amp; Join</button>
            <button class="btn-link" type="button" id="to-login">Back to Sign In</button>
          </form>
        </main>`;
      document.getElementById("to-login").onclick = () => { state.error = null; state.screen = "login"; render(); };
      document.getElementById("invite-form").onsubmit = async (e) => {
        e.preventDefault();
        state.error = null;
        const fd = new FormData(e.target);
        try {
          const auth = await api("/api/auth/invites/redeem", {
            method: "POST",
            body: JSON.stringify(Object.fromEntries(fd.entries())),
          });
          saveTokens(auth);
          await loadAuthenticatedData();
        } catch (err) {
          state.error = err.message;
          render();
        }
      };
      return;
    }

    app.innerHTML = `
      <main class="auth-shell">
        <form class="auth-card" id="login-form">
          <h2>Sign In to Claude Web UI</h2>
          <p class="subtitle">Authenticate with your username or email and password.</p>
          ${errHtml}
          <div class="form-group"><label>Username or Email</label><input name="identifier" required /></div>
          <div class="form-group"><label>Password</label><input name="password" type="password" required /></div>
          <button class="btn-primary" type="submit">Sign In</button>
          <button class="btn-link" type="button" id="to-invite">Have an invite code? Redeem Invite</button>
        </form>
      </main>`;
    document.getElementById("to-invite").onclick = () => { state.error = null; state.screen = "invite"; render(); };
    document.getElementById("login-form").onsubmit = async (e) => {
      e.preventDefault();
      state.error = null;
      const fd = new FormData(e.target);
      try {
        const auth = await api("/api/auth/login", {
          method: "POST",
          body: JSON.stringify(Object.fromEntries(fd.entries())),
        });
        saveTokens(auth);
        await loadAuthenticatedData();
      } catch (err) {
        state.error = err.message;
        render();
      }
    };
  }

  function renderHeaderSwitcher(activeProject) {
    if (!activeProject || !state.projectCli) {
      return `
        <header class="dashboard-header">
          <div class="cli-switcher">
            <span class="subtitle">No active project selected</span>
          </div>
        </header>`;
    }
    const cli = state.projectCli;
    if (!cli.can_switch) {
      return `
        <header class="dashboard-header">
          <div class="cli-readonly-pill">
            <span>Active CLI:</span>
            <strong>${esc(PROVIDER_LABELS[cli.active_provider] || cli.active_provider)}</strong>
            <span class="role-badge">Read-Only</span>
          </div>
        </header>`;
    }
    const providerOptions = Object.entries(PROVIDER_LABELS).map(([val, label]) =>
      `<option value="${esc(val)}" ${cli.active_provider === val ? "selected" : ""}>${esc(label)}</option>`
    ).join("");
    const matchingAccounts = (cli.available_accounts || []).filter((a) => a.provider === cli.active_provider);
    const accountOptions = [
      `<option value="">Ã¢â‚¬â€ Select account Ã¢â‚¬â€</option>`,
      ...matchingAccounts.map((a) =>
        `<option value="${esc(a.id)}" ${cli.active_account_id === a.id ? "selected" : ""}>${esc(a.name)}</option>`
      ),
    ].join("");
    return `
      <header class="dashboard-header">
        <div class="cli-switch-controls">
          <label for="header-cli-provider">CLI</label>
          <select id="header-cli-provider">${providerOptions}</select>
          <label for="header-cli-account">Account</label>
          <select id="header-cli-account">${accountOptions}</select>
        </div>
      </header>`;
  }

  function renderAccountsTab() {
    const accountCards = state.adminAccounts.map((acc) => {
      const creds = acc.credential_files && acc.credential_files.length > 0
        ? `Credentials: ${acc.credential_files.map(esc).join(", ")}`
        : "Not authenticated yet";
      return `
        <article class="panel-card account-card">
          <div class="panel-header-row">
            <strong>${esc(acc.name)}</strong>
            <span class="role-badge">${esc(PROVIDER_LABELS[acc.provider] || acc.provider)}</span>
          </div>
          <p class="project-path">${esc(acc.profile_dir)}</p>
          <p class="subtitle">${creds}</p>
          <div class="btn-group">
            <button class="btn-secondary" data-auth-acc="${esc(acc.id)}">OAuth / Credentials</button>
            <button class="btn-secondary" data-rename-acc="${esc(acc.id)}">Rename</button>
            <button class="btn-danger" data-del-acc="${esc(acc.id)}">Delete</button>
          </div>
        </article>`;
    }).join("");

    const projectOptions = state.projects.map((p) =>
      `<option value="${esc(p.id)}" ${p.id === state.aclProjectId ? "selected" : ""}>${esc(p.name)}</option>`
    ).join("");

    const devMembers = state.aclMembers.filter((m) => m.role === "developer");
    const aclRows = devMembers.map((m) => {
      const acl = state.aclList.find((a) => a.user_id === m.user_id) || { restricted: false, allowed_accounts: [] };
      const accountCheckboxes = state.adminAccounts.map((acc) => {
        const checked = acl.restricted && acl.allowed_accounts.includes(acc.id) ? "checked" : "";
        return `
          <label class="checkbox-item">
            <input type="checkbox" data-acl-acc="${esc(m.user_id)}" value="${esc(acc.id)}" ${checked} />
            ${esc(acc.name)} (${esc(PROVIDER_LABELS[acc.provider] || acc.provider)})
          </label>`;
      }).join("");
      return `
        <div class="panel-card">
          <div class="panel-header-row">
            <strong>${esc(m.username)}</strong>
            <label class="checkbox-item">
              <input type="checkbox" data-acl-unrestricted="${esc(m.user_id)}" ${!acl.restricted ? "checked" : ""} />
              Unrestricted (All Accounts)
            </label>
          </div>
          <div class="checkbox-list">${accountCheckboxes || "<span class='subtitle'>No CLI accounts created yet.</span>"}</div>
          <button class="btn-secondary" data-save-acl="${esc(m.user_id)}">Save ACL</button>
        </div>`;
    }).join("");

    return `
      <section class="panel-card">
        <div class="panel-header-row">
          <div>
            <h2>CLI Accounts &amp; OAuth Profiles</h2>
            <p class="subtitle">Isolated credentials per account with shared provider sessions &amp; skills.</p>
          </div>
          <button class="btn-primary" id="open-add-account-btn" type="button">+ Add Account</button>
        </div>
        <div class="admin-grid">${accountCards || "<p class='subtitle'>No CLI accounts yet.</p>"}</div>
      </section>
      <section class="panel-card">
        <h3>Developer Account ACLs</h3>
        <div class="form-group">
          <label>Project</label>
          <select id="acl-project-select">${projectOptions || "<option value=''>No projects</option>"}</select>
        </div>
        ${aclRows || "<p class='subtitle'>No Developer members assigned to this project.</p>"}
      </section>`;
  }

  function renderAddAccountModal() {
    const modal = state.addAccountModal;
    if (!modal) return "";

    if (modal.step === "create") {
      return `
        <div class="modal-backdrop">
          <div class="modal-card">
            <div class="panel-header-row">
              <h3>Add Account</h3>
              <button class="btn-secondary" id="close-account-modal" type="button">Close</button>
            </div>
            <form id="create-account-form">
              <div class="form-group">
                <label>CLI Provider</label>
                <select name="provider">
                  <option value="claude_code">Claude Code</option>
                  <option value="codex">Codex</option>
                  <option value="antigravity">Antigravity</option>
                </select>
              </div>
              <div class="form-group">
                <label>Account Name</label>
                <input name="name" placeholder="e.g. Work, Personal" required />
              </div>
              <button class="btn-primary" type="submit">Create Profile &amp; Authenticate</button>
            </form>
          </div>
        </div>`;
    }

    const acc = state.adminAccounts.find((a) => a.id === modal.accountId) || modal.account;
    const credFilesHtml = (acc.credential_files || []).map((f) => `
      <div class="row">
        <code>${esc(f)}</code>
        <button class="btn-danger" data-del-cred="${esc(f)}" type="button">Delete</button>
      </div>
    `).join("");

    const bodyHtml = modal.tab === "credentials"
      ? `
        <form id="upload-cred-form">
          <div class="form-group">
            <label>Choose Credential File (.credentials.json, .claude.json, auth.json)</label>
            <input type="file" id="cred-file-picker" />
          </div>
          <div class="form-group">
            <label>Target Filename</label>
            <input id="cred-filename" name="filename" value=".credentials.json" required />
          </div>
          <div class="form-group">
            <label>Paste Credential Content</label>
            <textarea id="cred-content" name="content" rows="6" required></textarea>
          </div>
          <button class="btn-primary" type="submit">Save Credential File</button>
        </form>
        <div style="margin-top: 14px;">
          <h4>Stored Credential Files</h4>
          ${credFilesHtml || "<p class='subtitle'>No credential files uploaded yet.</p>"}
        </div>`
      : `
        <div class="btn-group" style="margin-bottom: 10px;">
          <button class="btn-secondary" id="launch-oauth-pty-btn" type="button">Launch OAuth Terminal</button>
        </div>
        <div id="oauth-xterm-host" class="xterm-container">
          <pre id="oauth-xterm-fallback" class="xterm-fallback-log">Click "Launch OAuth Terminal" to start ${esc(PROVIDER_LABELS[acc.provider] || acc.provider)} login...</pre>
          <form id="oauth-fallback-input-form" style="display:none; gap:8px;">
            <input id="oauth-fallback-input" placeholder="Paste OAuth code and press Enter..." />
            <button class="btn-secondary" type="submit">Send</button>
          </form>
        </div>`;

    return `
      <div class="modal-backdrop">
        <div class="modal-card">
          <div class="panel-header-row">
            <div>
              <h3>Authenticate: ${esc(acc.name)}</h3>
              <span class="role-badge">${esc(PROVIDER_LABELS[acc.provider] || acc.provider)}</span>
            </div>
            <button class="btn-secondary" id="close-account-modal" type="button">Close</button>
          </div>
          <div class="tab-bar">
            <button class="btn-secondary ${modal.tab === "pty" ? "active" : ""}" id="modal-tab-pty" type="button">OAuth Terminal (xterm.js)</button>
            <button class="btn-secondary ${modal.tab === "credentials" ? "active" : ""}" id="modal-tab-creds" type="button">Upload / Paste Credentials</button>
          </div>
          ${bodyHtml}
        </div>
      </div>`;
  }


  const kanbanColumns = ["triage", "ready", "running", "blocked", "review", "done", "archived"];
  const kanban = { projectId: null, cards: [], editing: null, error: null };

  function kanbanPath(projectId = state.activeProjectId) {
    return `/api/projects/${encodeURIComponent(projectId)}/kanban`;
  }

  async function loadKanban() {
    const projectId = state.activeProjectId;
    if (!projectId) return;
    const cards = await api(kanbanPath(projectId));
    if (projectId !== state.activeProjectId || state.projectTab !== "kanban") return;
    const changed = projectId !== kanban.projectId || JSON.stringify(cards) !== JSON.stringify(kanban.cards);
    kanban.projectId = projectId;
    kanban.cards = cards;
    if (changed && !kanban.editing) render();
  }

  function renderKanban(readOnly) {
    const cards = kanban.projectId === state.activeProjectId ? kanban.cards : [];
    const card = cards.find((item) => item.id === kanban.editing);
    const label = (value) => value[0].toUpperCase() + value.slice(1);
    const options = (selected) => kanbanColumns.map((column) => `
      <option value="${column}" ${column === selected ? "selected" : ""}>${label(column)}</option>
    `).join("");
    return `
      <section class="panel-card kanban-panel">
        <div class="pane-head">
          <h2>Kanban</h2>
          ${readOnly ? "" : '<button class="btn-primary" type="button" id="kanban-new">New card</button>'}
        </div>
        ${kanban.error ? `<p role="alert">${esc(kanban.error)}</p>` : ""}
        ${!readOnly && kanban.editing ? `
          <form id="kanban-form" class="kanban-form">
            <label>Title <input name="title" value="${esc(card?.title)}" required></label>
            <label>Description <textarea name="description">${esc(card?.description)}</textarea></label>
            ${card ? "" : `<label>Column <select name="column">${options("triage")}</select></label>`}
            <label>Depends on <select name="depends_on" multiple ${card?.column === "running" ? "disabled" : ""}>
              ${cards.filter((item) => item.id !== card?.id).map((item) => `
                <option value="${esc(item.id)}" ${card?.depends_on.includes(item.id) ? "selected" : ""}>${esc(item.title)}</option>`).join("")}
            </select></label>
            <label><input name="create_git_branch" type="checkbox" ${card?.create_git_branch ? "checked" : ""}> Create dedicated git branch</label>
            <label>Branch name <input name="git_branch_name" value="${esc(card?.git_branch_name)}" placeholder="Generated if blank"></label>
            <div><button type="submit" class="btn-primary">Save card</button>
              <button type="button" class="btn-secondary" id="kanban-cancel">Cancel</button></div>
          </form>` : ""}
        <div class="kanban-board">
          ${kanbanColumns.map((column) => `
            <section class="kanban-lane" data-kanban-lane="${column}">
              <h3>${label(column)}</h3>
              ${cards.filter((card) => card.column === column).map((card, index) => `
                <article class="kanban-card" data-kanban-card="${esc(card.id)}" data-kanban-index="${index}" draggable="${!readOnly}">
                  ${card.session_id ? `<button class="kanban-title" type="button" data-kanban-session="${esc(card.session_id)}">${esc(card.title)}</button>` : `<strong>${esc(card.title)}</strong>`}
                  ${card.description ? `<p>${esc(card.description)}</p>` : ""}
                  ${card.unresolved_blockers.length ? `<p>Waiting on ${card.unresolved_blockers.map((id) => esc(cards.find((item) => item.id === id)?.title || id)).join(", ")}</p>` : ""}
                  ${card.git_branch_name ? `<p>Branch <code>${esc(card.git_branch_name)}</code></p>` : ""}
                  ${card.session_status ? `<p>CLI session ${esc(card.session_status.replaceAll("_", " "))}</p>` : ""}
                  ${card.blocked_reason ? `<p>${esc(card.blocked_reason)}</p>` : ""}
                  ${card.result_summary ? `<p>${esc(card.result_summary)}</p>` : ""}
                  <div class="kanban-actions">
                    ${card.session_id ? '<button type="button" class="btn-secondary" data-kanban-diff>Git diff</button>' : ""}
                    ${readOnly ? "" : `
                      <button type="button" class="btn-secondary" data-kanban-edit="${esc(card.id)}">Edit</button>
                      ${column === "triage" ? `<button type="button" class="btn-secondary" data-kanban-decompose="${esc(card.id)}">Decompose with Agent</button>` : ""}
                      <select aria-label="Move card" data-kanban-move="${esc(card.id)}">${options(column)}</select>`}
                  </div>
                </article>`).join("")}
            </section>`).join("")}
        </div>
      </section>`;
  }

  function bindKanbanHandlers() {
    const tab = document.getElementById("proj-tab-kanban");
    if (tab) tab.onclick = async () => {
      kanban.editing = null;
      kanban.error = null;
      state.projectTab = "kanban";
      render();
      try { await loadKanban(); } catch (error) { kanban.error = error.message; render(); }
    };
    const board = document.querySelector(".kanban-board");
    if (!board) return;
    const path = kanbanPath();
    let dragging = null;
    const mutate = async (suffix, method, value) => {
      try {
        kanban.error = null;
        await api(path + suffix, { method, body: JSON.stringify(value) });
        kanban.editing = null;
        await loadKanban();
      } catch (error) {
        kanban.error = error.message;
      }
      render();
    };
    const move = (id, column, position) => mutate(`/${encodeURIComponent(id)}/move`, "POST", { column, position });
    document.getElementById("kanban-new")?.addEventListener("click", () => { kanban.editing = "new"; render(); });
    document.getElementById("kanban-cancel")?.addEventListener("click", () => { kanban.editing = null; render(); });
    document.getElementById("kanban-form")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const form = new FormData(event.currentTarget);
      const value = {
        title: form.get("title"),
        description: form.get("description"),
        create_git_branch: form.has("create_git_branch"),
        git_branch_name: form.get("git_branch_name").trim() || null,
      };
      if (!event.currentTarget.elements.depends_on.disabled) value.depends_on = form.getAll("depends_on");
      if (kanban.editing === "new") {
        value.column = form.get("column");
        void mutate("", "POST", value);
      } else {
        void mutate(`/${encodeURIComponent(kanban.editing)}`, "PATCH", value);
      }
    });
    board.querySelectorAll("[data-kanban-edit]").forEach((button) => {
      button.onclick = () => { kanban.editing = button.dataset.kanbanEdit; render(); };
    });
    board.querySelectorAll("[data-kanban-decompose]").forEach((button) => {
      button.onclick = () => mutate(`/${encodeURIComponent(button.dataset.kanbanDecompose)}/decompose`, "POST", {});
    });
    board.querySelectorAll("[data-kanban-session]").forEach((button) => {
      button.onclick = async () => {
        state.activeSessionId = button.dataset.kanbanSession;
        state.projectTab = "chat";
        await loadProjectSessions();
        render();
      };
    });
    board.querySelectorAll("[data-kanban-diff]").forEach((button) => {
      button.onclick = async () => {
        try {
          const id = button.closest("[data-kanban-card]").dataset.kanbanCard;
          const result = await api(path + "/" + encodeURIComponent(id) + "/diff");
          const dialog = document.createElement("dialog");
          const heading = document.createElement("h3");
          heading.textContent = "Card git diff";
          const diff = document.createElement("pre");
          diff.textContent = result.unified === null ? "No diff recorded." : result.unified || "No tracked file changes.";
          const close = document.createElement("button");
          close.type = "button";
          close.textContent = "Close";
          close.onclick = () => dialog.close();
          dialog.addEventListener("close", () => dialog.remove(), { once: true });
          dialog.append(heading, diff, close);
          document.body.appendChild(dialog);
          dialog.showModal();
        } catch (error) {
          kanban.error = error.message;
          render();
        }
      };
    });
    board.querySelectorAll("[data-kanban-move]").forEach((select) => {
      select.onchange = () => move(select.dataset.kanbanMove, select.value);
    });
    board.querySelectorAll(".kanban-card[draggable=true]").forEach((card) => {
      card.ondragstart = (event) => {
        dragging = card;
        event.dataTransfer.setData("text/plain", card.dataset.kanbanCard);
        event.dataTransfer.effectAllowed = "move";
      };
      card.ondragend = () => { dragging = null; };
    });
    board.querySelectorAll("[data-kanban-lane]").forEach((lane) => {
      lane.ondragover = (event) => { if (dragging) event.preventDefault(); };
      lane.ondrop = (event) => {
        if (!dragging) return;
        event.preventDefault();
        const target = event.target.closest("[data-kanban-card]");
        if (target === dragging) return;
        let position = target ? Number(target.dataset.kanbanIndex) : undefined;
        if (target && dragging.parentNode === lane && Number(dragging.dataset.kanbanIndex) < position) position -= 1;
        void move(dragging.dataset.kanbanCard, lane.dataset.kanbanLane, position);
        dragging = null;
      };
    });
  }

  function renderDashboard() {
    closeActivePty();
    const activeProject = state.projects.find((p) => p.id === state.activeProjectId) || null;
    const isAdmin = state.user.role === "admin";
    const errHtml = state.error ? `<div class="alert-error">${esc(state.error)}</div>` : "";
    const okHtml = state.notice ? `<div class="alert-success">${esc(state.notice)}</div>` : "";

    const projectItems = state.projects.map((p) => `
      <li>
        <button class="project-item ${p.id === state.activeProjectId ? "active" : ""}" data-pid="${esc(p.id)}">
          <span>${esc(p.name)}</span>
          <span class="project-path">${esc(p.workspace_path)}</span>
        </button>
      </li>
    `).join("");

    let mainHtml = "";
    if (state.tab === "admin" && isAdmin) {
      const projectOptions = state.projects.map((p) => `<option value="${esc(p.id)}">${esc(p.name)}</option>`).join("");
      const usersHtml = state.adminUsers.map((u) => `
        <div class="row">
          <div>
            <strong>${esc(u.username)}</strong> (${esc(u.email)})
            <span class="role-badge">${esc(u.role)}</span>
          </div>
          <button class="btn-danger" data-revoke-uid="${esc(u.id)}">Revoke Sessions</button>
        </div>
      `).join("");
      const invitesHtml = state.adminInvites.map((inv) => `
        <div class="row">
          <div><code>${esc(inv.code)}</code> Ã¢â‚¬â€ ${esc(inv.role)}</div>
          <span>${inv.used_by ? "Used" : "Available"}</span>
        </div>
      `).join("");

      mainHtml = `
        <div class="admin-grid">
          <form class="panel-card" id="create-project-form">
            <h3>Register Project Workspace</h3>
            <div class="form-group"><label>Project Name</label><input name="name" required /></div>
            <div class="form-group"><label>Workspace Path</label><input name="workspace_path" required /></div>
            <button class="btn-primary" type="submit">Register Project</button>
          </form>
          <form class="panel-card" id="create-invite-form">
            <h3>Generate Single-Use Invite</h3>
            <div class="form-group">
              <label>Role</label>
              <select name="role">
                <option value="developer">Developer</option>
                <option value="viewer">Viewer</option>
                <option value="admin">Admin</option>
              </select>
            </div>
            <div class="form-group">
              <label>Allowed Projects</label>
              <select name="project_ids" multiple>${projectOptions}</select>
            </div>
            <button class="btn-primary" type="submit">Create Invite</button>
          </form>
        </div>
        <div class="panel-card"><h3>Users</h3>${usersHtml || "<p class='subtitle'>No users.</p>"}</div>
        <div class="panel-card"><h3>Invites</h3>${invitesHtml || "<p class='subtitle'>No invites yet.</p>"}</div>
      `;
    } else if (state.tab === "accounts" && isAdmin) {
      mainHtml = renderAccountsTab();
    } else if (state.tab === "agents_skills_mcp") {
      mainHtml = renderAgentsSkillsMcpTab();
    } else if (!activeProject) {
      mainHtml = `
        <section class="panel-card">
          <h2>No Project Selected</h2>
          <p class="subtitle">Ask an Admin to assign you to a project workspace.</p>
        </section>
      `;
    } else {
      const readOnly = state.user.role === "viewer";
      const gs = state.gitStatus;
      const stagedHtml = (gs && gs.staged || []).map((c) => `
        <button class="git-change-item" data-diff-path="${esc(c.path)}" data-diff-staged="true">
          <span class="git-badge staged">${esc(c.status)}</span> ${esc(c.path)}
        </button>`).join("");
      const unstagedHtml = (gs && gs.unstaged || []).map((c) => `
        <button class="git-change-item" data-diff-path="${esc(c.path)}" data-diff-staged="false">
          <span class="git-badge unstaged">${esc(c.status)}</span> ${esc(c.path)}
        </button>`).join("");
      const untrackedHtml = (gs && gs.untracked || []).map((p) => `
        <button class="git-change-item" data-diff-path="${esc(p)}" data-diff-staged="false">
          <span class="git-badge untracked">?</span> ${esc(p)}
        </button>`).join("");
      const commitsHtml = (gs && gs.recent_commits || []).map((cm) => `
        <div class="commit-item">
          <code>${esc(cm.short_hash)}</code>
          <span class="commit-msg">${esc(cm.message)}</span>
          <span class="commit-meta">${esc(cm.author_name)}</span>
        </div>`).join("");

      const filesGitHtml = `
        <section class="panel-card workspace-header-card">
          <div>
            <h2>${esc(activeProject.name)}</h2>
            <p class="subtitle" style="margin:0">${esc(activeProject.workspace_path)}</p>
          </div>
          <div class="files-git-controls">
            <label class="toggle-label">
              <input type="checkbox" id="toggle-gitignore" ${state.respectGitignore ? "checked" : ""} />
              <span>Respect .gitignore</span>
            </label>
            <button class="btn-secondary" type="button" id="refresh-workspace-btn">Refresh</button>
          </div>
        </section>

        <div class="files-git-grid">
          <section class="panel-card file-tree-pane">
            <div class="pane-head">
              <h3>Files</h3>
            </div>
            ${!readOnly ? `
              <form id="new-file-form" class="new-file-form">
                <input name="path" placeholder="new/file.txt" required />
                <button class="btn-secondary" type="submit">+</button>
              </form>` : ""}
            <div class="tree-scroll">${renderTreeNodes(state.fileTree) || "<p class='subtitle'>Empty workspace</p>"}</div>
          </section>

          <section class="panel-card editor-pane">
            <div class="editor-toolbar">
              <strong>${state.activeFile ? esc(state.activeFile.path) : "Select a file"}</strong>
              <div class="files-git-controls">
                ${readOnly ? `<span class="role-badge">Read-Only (Viewer)</span>` : `
                  <button class="btn-primary" type="button" id="save-file-btn" ${!state.activeFile || state.activeFile.is_binary ? "disabled" : ""}>
                    ${state.editorDirty ? "Save *" : "Save"}
                  </button>`}
              </div>
            </div>
            ${state.activeFile && state.activeFile.is_binary
              ? `<p class="subtitle">Binary file (${state.activeFile.size} bytes) cannot be edited in text view.</p>`
              : `<textarea id="code-editor" class="code-editor" ${readOnly || !state.activeFile ? "readonly" : ""} placeholder="Select a file from the explorer...">${esc(state.editorText)}</textarea>`}
          </section>

          <section class="panel-card git-pane">
            <div class="pane-head">
              <h3>Git ${gs && gs.branch ? `<span class="role-badge">${esc(gs.branch)}</span>` : ""}</h3>
              <div class="files-git-controls">
                <button class="btn-secondary" type="button" id="diff-all-btn">All</button>
                <button class="btn-secondary" type="button" id="toggle-diff-mode-btn">${state.diffMode === "unified" ? "Unified" : "Split"}</button>
              </div>
            </div>
            <div class="git-changes-section">
              <div class="git-group-label">Staged (${(gs && gs.staged || []).length})</div>
              ${stagedHtml || "<p class='subtitle'>None</p>"}
              <div class="git-group-label">Unstaged (${(gs && gs.unstaged || []).length})</div>
              ${unstagedHtml || "<p class='subtitle'>None</p>"}
              <div class="git-group-label">Untracked (${(gs && gs.untracked || []).length})</div>
              ${untrackedHtml || "<p class='subtitle'>None</p>"}
            </div>
            <div class="git-diff-section">
              <div class="git-group-label">Diff ${state.diffPath ? `(${esc(state.diffPath)})` : "(Workspace)"}</div>
              ${renderDiffBody()}
            </div>
            <div class="git-commits-section">
              <div class="git-group-label">Recent Commits</div>
              ${commitsHtml || "<p class='subtitle'>No commits</p>"}
            </div>
          </section>
        </div>
      `;

      mainHtml = `
        <div class="tab-bar" style="margin:0;">
          <button class="btn-secondary ${state.projectTab === "chat" ? "active" : ""}" id="proj-tab-chat" type="button">Chat &amp; Terminal</button>
          <button class="btn-secondary ${state.projectTab === "files" ? "active" : ""}" id="proj-tab-files" type="button">Files &amp; Git</button>
          <button class="btn-secondary ${state.projectTab === "kanban" ? "active" : ""}" id="proj-tab-kanban" type="button">Kanban</button>
        </div>
        ${state.projectTab === "files" ? filesGitHtml : state.projectTab === "kanban" ? renderKanban(readOnly) : renderProjectWorkspace(activeProject)}
      `;
    }

    app.innerHTML = `
      <div class="dashboard-shell">
        <aside class="sidebar">
          <div class="brand">
            <h1>Claude Web UI</h1>
            <span class="role-badge">${esc(state.user.role)}</span>
          </div>
          <div class="sidebar-section">
            <h3>Projects</h3>
            <ul class="project-list">${projectItems || "<li class='subtitle'>No assigned projects</li>"}</ul>
          </div>
          <div class="sidebar-section">
            <h3>Workspace</h3>
            <button class="nav-item ${state.tab === "agents_skills_mcp" ? "active" : ""}" id="nav-asm">Agents, Skills &amp; MCP</button>
          </div>
          ${isAdmin ? `
            <div class="sidebar-section">
              <button class="nav-item ${state.tab === "admin" ? "active" : ""}" id="nav-admin">Admin: Users &amp; Projects</button>
              <button class="nav-item ${state.tab === "accounts" ? "active" : ""}" id="nav-accounts">Admin: CLI Accounts</button>
            </div>` : ""}
          <div class="sidebar-section">
            <button class="btn-secondary" id="pair-messenger-btn">Pair Telegram or Discord</button>
            <p class="subtitle" id="pair-messenger-result" aria-live="polite">${esc(state.messengerPairing || "")}</p>
            <button class="btn-secondary" id="logout-btn">Sign Out (${esc(state.user.username)})</button>
          </div>
        </aside>
        <main class="dashboard-main">
          ${renderHeaderSwitcher(activeProject)}
          ${errHtml}${okHtml}${mainHtml}
        </main>
      </div>
      ${renderAddAccountModal()}
    `;

    bindDashboardHandlers();
  }

  function renderTurnEvent(ev) {
    switch (ev.type) {
      case "text_delta":
        return `<div class="event-text">${esc(ev.text)}</div>`;
      case "thinking_delta":
        return `<details class="event-thinking"><summary>Thinking</summary><pre>${esc(ev.text)}</pre></details>`;
      case "tool_call_start":
        return `<div class="event-tool-card"><strong>Tool: ${esc(ev.name)}</strong><pre>${esc(JSON.stringify(ev.input, null, 2))}</pre></div>`;
      case "tool_call_result":
        return `<div class="event-tool-result ${ev.is_error ? "error" : ""}"><pre>${esc(ev.output)}</pre></div>`;
      case "subagent_start":
        return `<div class="role-badge">Subagent started: ${esc(ev.name)}</div>`;
      case "subagent_end":
        return `<div class="role-badge">Subagent finished: ${esc(ev.id)}</div>`;
      case "permission_request":
        return `<div class="subtitle">Requested approval for ${esc(ev.tool_name)}</div>`;
      case "turn_complete":
        return ev.result_text ? `<div class="event-complete">${esc(ev.result_text)}</div>` : "";
      case "turn_error":
        return `<div class="alert-error">${esc(ev.message)}</div>`;
      default:
        return "";
    }
  }

  function renderProjectWorkspace(activeProject) {
    if (!activeProject) {
      return `
        <section class="panel-card">
          <h2>No Project Selected</h2>
          <p class="subtitle">Ask an Admin to assign you to a project workspace.</p>
        </section>`;
    }
    const isViewer = state.user.role === "viewer";
    const detail = state.activeSessionDetail;
    const activeSess = detail ? detail.session : state.sessions.find((s) => s.id === state.activeSessionId) || null;
    const sessionListHtml = state.sessions.map((s) => `
      <li>
        <button class="session-item ${s.id === state.activeSessionId ? "active" : ""}" data-sid="${esc(s.id)}">
          <div class="panel-header-row" style="margin-bottom:4px;">
            <strong>${esc(s.title)}</strong>
            <span class="role-badge">${esc(PROVIDER_LABELS[s.provider] || s.provider)}</span>
          </div>
          <div class="subtitle" style="margin:0;font-size:0.78rem;">
            <span>${esc(s.source)}</span>
            ${s.native_session_id ? ` Ã¢â‚¬Â¢ <code>${esc(s.native_session_id)}</code>` : ""}
          </div>
        </button>
      </li>
    `).join("");

    const nativeHtml = detail && detail.native_messages && detail.native_messages.length > 0 && (!detail.turns || detail.turns.length === 0)
      ? detail.native_messages.map((m) => `
        <article class="turn-card">
          <strong>${esc(m.role)}${m.tool_name ? ` (${esc(m.tool_name)})` : ""}</strong>
          <pre class="event-text">${esc(m.text)}</pre>
        </article>`).join("")
      : "";

    const turnsHtml = detail && detail.turns ? detail.turns.map((t) => {
      const pending = t.pending_permission;
      const approvalCard = pending && !isViewer ? `
        <div class="approval-card" data-view="approval-prompt">
          <strong>Permission Request: ${esc(pending.tool_name)}</strong>
          <pre>${esc(JSON.stringify(pending.input, null, 2))}</pre>
          <div class="btn-group">
            <button class="btn-primary" data-allow-turn="${esc(t.id)}" data-req-id="${esc(pending.request_id)}" type="button">Allow</button>
            <button class="btn-danger" data-deny-turn="${esc(t.id)}" data-req-id="${esc(pending.request_id)}" type="button">Deny</button>
          </div>
        </div>` : "";
      return `
        <article class="turn-card">
          <div class="panel-header-row" style="margin-bottom:6px;">
            <strong>Prompt</strong>
            <span class="role-badge">${esc(t.status)}</span>
          </div>
          <p style="margin:0 0 8px;">${esc(t.prompt)}</p>
          <div class="turn-events">${(t.events || []).map(renderTurnEvent).join("")}</div>
          ${t.error_message ? `<div class="alert-error">${esc(t.error_message)}</div>` : ""}
          ${approvalCard}
        </article>`;
    }).join("") : "";

    const approvalMode = activeSess ? activeSess.approval_mode : "full_auto";
    const approvalLabel = approvalMode === "interactive" ? "Mode: Interactive Approval" : "Mode: Full Auto";
    const hasActiveTurn = state.schedulerStatus && state.schedulerStatus.active_session_id === state.activeSessionId;

    const terminalHtml = state.showProjectTerminal && !isViewer ? `
      <div class="panel-card" style="margin-top:14px;">
        <div class="panel-header-row">
          <strong>Project Terminal (xterm.js)</strong>
          <div class="btn-group">
            <button class="btn-secondary" id="term-mode-shell" type="button">Workspace Shell</button>
            <button class="btn-secondary" id="term-mode-cli" type="button">Resume CLI</button>
            <button class="btn-secondary" id="close-project-term" type="button">Close</button>
          </div>
        </div>
        <div id="project-xterm-host" class="xterm-container">
          <pre id="project-xterm-fallback" class="xterm-fallback-log">Connecting to project terminal...</pre>
          <form id="project-xterm-fallback-form" style="display:none; gap:8px;">
            <input id="project-xterm-fallback-input" placeholder="Type command and press Enter..." />
            <button class="btn-secondary" type="submit">Send</button>
          </form>
        </div>
      </div>` : "";

    return `
      <section class="chat-workspace" data-view="chat-workspace">
        <aside class="chat-sessions-panel panel-card" data-view="session-list">
          <div class="panel-header-row">
            <h3>Sessions</h3>
            ${!isViewer ? `<button class="btn-primary" id="new-chat-session-btn" type="button">+ New Chat</button>` : ""}
          </div>
          <div class="form-group">
            <input id="session-search-input" type="search" placeholder="Search sessions..." value="${esc(state.sessionSearch)}" />
          </div>
          <ul class="session-list">${sessionListHtml || "<li class='subtitle'>No sessions yet.</li>"}</ul>
        </aside>

        <div class="chat-main-panel panel-card" data-view="chat-stream">
          <div class="panel-header-row">
            <div>
              <h2 style="margin:0;">${esc(activeProject.name)}</h2>
              <p class="project-path" style="margin:4px 0 0;">${esc(activeProject.workspace_path)}</p>
            </div>
            ${!isViewer ? `
              <div class="btn-group">
                ${activeSess ? `<button class="btn-secondary" id="toggle-approval-mode-btn" type="button">${approvalLabel}</button>` : ""}
                <button class="btn-secondary" id="toggle-project-terminal-btn" type="button">${state.showProjectTerminal ? "Hide Terminal" : "Project Terminal"}</button>
                ${activeSess && hasActiveTurn ? `<button class="btn-danger" id="stop-active-turn-btn" type="button">Stop</button>` : ""}
              </div>` : `<span class="role-badge">Read-Only Stream</span>`}
          </div>

          <div class="chat-feed">
            ${nativeHtml}${turnsHtml || (!nativeHtml ? "<p class='subtitle'>Select or create a session and send a prompt.</p>" : "")}
          </div>

          ${!isViewer ? `
            <form id="chat-prompt-form" class="chat-composer">
              <textarea id="chat-prompt-input" rows="3" placeholder="Send prompt to active CLI..." required></textarea>
              <div class="btn-group" style="margin-top:8px;">
                <button class="btn-primary" type="submit" style="width:auto;">Send Prompt</button>
              </div>
            </form>` : ""}

          ${terminalHtml}
        </div>
      </section>`;
  }

  function attachPtyTerminal(hostId, fallbackLogId, fallbackFormId, fallbackInputId, wsPath) {
    closeActivePty();
    const host = document.getElementById(hostId);
    if (!host) return;
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    let cols = 90;
    let rows = 24;
    let term = null;
    const fallbackLog = document.getElementById(fallbackLogId);
    const fallbackForm = document.getElementById(fallbackFormId);

    if (typeof window.Terminal === "function") {
      host.innerHTML = "";
      term = new window.Terminal({ cols, rows, cursorBlink: true });
      if (window.WebLinksAddon && typeof window.WebLinksAddon.WebLinksAddon === "function") {
        term.loadAddon(new window.WebLinksAddon.WebLinksAddon());
      }
      term.open(host);
      state.ptyTerminal = term;
    } else if (fallbackLog && fallbackForm) {
      fallbackLog.textContent = "Connecting to PTY...\n";
      fallbackForm.style.display = "flex";
    }

    const wsUrl = `${proto}//${window.location.host}${wsPath}&cols=${cols}&rows=${rows}`;
    const ws = new WebSocket(wsUrl);
    state.ptySocket = ws;

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.type === "output") {
          if (term) term.write(msg.data);
          else if (fallbackLog) fallbackLog.innerHTML += linkifyUrls(msg.data);
        } else if (msg.type === "exit") {
          const note = `\r\n[Process exited with code ${msg.code}]\r\n`;
          if (term) term.write(note);
          else if (fallbackLog) fallbackLog.innerHTML += esc(note);
        } else if (msg.type === "error") {
          const note = `\r\n[PTY error: ${msg.message}]\r\n`;
          if (term) term.write(note);
          else if (fallbackLog) fallbackLog.innerHTML += esc(note);
        }
      } catch {}
    };

    if (term) {
      term.onData((data) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "input", data }));
        }
      });
      term.onResize((sz) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "resize", cols: sz.cols, rows: sz.rows }));
        }
      });
    } else if (fallbackForm) {
      fallbackForm.onsubmit = (e) => {
        e.preventDefault();
        const inp = document.getElementById(fallbackInputId);
        if (inp && ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "input", data: inp.value + "\r" }));
          inp.value = "";
        }
      };
    }
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

  function renderAgentsSkillsMcpTab() {
    const canEdit = asmCanMutate();
    const readOnlyAttr = canEdit ? "" : "disabled";
    const provTabs = Object.entries(PROVIDER_LABELS).map(([k, label]) => `
      <button class="btn-secondary ${state.asmProvider === k ? "active" : ""}" data-asm-prov="${esc(k)}" type="button">${esc(label)}</button>
    `).join("");

    let bodyHtml = "";
    if (state.asmScope === "project" && !state.activeProjectId) {
      bodyHtml = `<div class="panel-card"><p class="subtitle">Select a project from the sidebar or switch to Global Scope.</p></div>`;
    } else if (state.asmSubTab === "agents") {
      const sel = state.asmSelectedAgent || {
        name: "",
        description: "",
        model: PROVIDER_MODEL_TIERS[state.asmProvider][0],
        tools: ["Read", "Grep"],
        disallowed_tools: [],
        permission_mode: "default",
        system_prompt: "",
        raw_markdown: "---\nname: \ndescription: \nmodel: " + PROVIDER_MODEL_TIERS[state.asmProvider][0] + "\n---\n",
      };
      const agentRows = state.asmAgents.map((a) => `
        <div class="row">
          <div>
            <strong>${esc(a.name)}</strong>
            <span class="role-badge">${esc(a.model || "inherit")}</span>
            <p class="subtitle">${esc(a.description || "")}</p>
          </div>
          <div class="btn-group">
            <button class="btn-secondary" data-asm-edit-agent="${esc(a.name)}" type="button">Edit</button>
            ${canEdit ? `<button class="btn-danger" data-asm-del-agent="${esc(a.name)}" type="button">Delete</button>` : ""}
          </div>
        </div>
      `).join("");

      const modelOptions = PROVIDER_MODEL_TIERS[state.asmProvider].map((m) =>
        `<option value="${esc(m)}" ${sel.model === m ? "selected" : ""}>${esc(m)}</option>`
      ).join("");
      const permOptions = PERMISSION_MODES.map((m) =>
        `<option value="${esc(m)}" ${sel.permission_mode === m ? "selected" : ""}>${esc(m)}</option>`
      ).join("");
      const toolChecks = CLAUDE_TOOLS.map((t) => {
        const chk = (sel.tools || []).includes(t) ? "checked" : "";
        return `<label class="checkbox-item"><input type="checkbox" name="asm_tools" value="${esc(t)}" ${chk} ${readOnlyAttr} /> ${esc(t)}</label>`;
      }).join("");

      const editorFields = state.asmRawMode
        ? `
          <div class="form-group">
            <label>Raw Markdown + YAML Frontmatter</label>
            <textarea id="asm-agent-raw" name="raw_markdown" rows="12" ${readOnlyAttr}>${esc(sel.raw_markdown || "")}</textarea>
          </div>`
        : `
          <div class="form-group"><label>Description</label><input id="asm-agent-desc" value="${esc(sel.description || "")}" ${readOnlyAttr} /></div>
          <div class="form-group"><label>Model Tier</label><select id="asm-agent-model" ${readOnlyAttr}>${modelOptions}</select></div>
          <div class="form-group"><label>Allowed Tools</label><div class="checkbox-list">${toolChecks}</div></div>
          <div class="form-group"><label>Disallowed Tools (comma-separated)</label><input id="asm-agent-disallowed" value="${esc((sel.disallowed_tools || []).join(", "))}" ${readOnlyAttr} /></div>
          <div class="form-group"><label>Permission Mode</label><select id="asm-agent-perm" ${readOnlyAttr}>${permOptions}</select></div>
          <div class="form-group"><label>System Prompt</label><textarea id="asm-agent-prompt" rows="6" ${readOnlyAttr}>${esc(sel.system_prompt || "")}</textarea></div>`;

      bodyHtml = `
        <div class="admin-grid">
          <div class="panel-card">
            <h3>Configured Subagents (${esc(PROVIDER_LABELS[state.asmProvider])})</h3>
            ${agentRows || "<p class='subtitle'>No subagents found in this scope.</p>"}
          </div>
          <form class="panel-card" id="asm-agent-form">
            <div class="panel-header-row">
              <h3>Subagent Editor</h3>
              <button class="btn-secondary" id="asm-toggle-raw-btn" type="button">${state.asmRawMode ? "Visual Form" : "Raw YAML / Markdown"}</button>
            </div>
            <div class="form-group"><label>Agent Name</label><input id="asm-agent-name" value="${esc(sel.name || "")}" required ${readOnlyAttr} /></div>
            ${editorFields}
            ${canEdit ? `<button class="btn-primary" type="submit">Save Subagent</button>` : `<p class="subtitle">Read-only access in this scope.</p>`}
          </form>
        </div>`;
    } else if (state.asmSubTab === "skills") {
      const sel = state.asmSelectedSkill || {
        name: "",
        skill_md: "---\nname: \ndescription: \n---\n# Instructions\n",
        companion_files: [],
        is_shared: false,
      };
      const skillRows = state.asmSkills.map((s) => `
        <div class="row">
          <div>
            <strong>${esc(s.name)}</strong>
            ${s.is_shared ? `<span class="role-badge">Shared</span>` : ""}
            <p class="subtitle">${esc(s.description || "")}</p>
          </div>
          <div class="btn-group">
            <button class="btn-secondary" data-asm-edit-skill="${esc(s.name)}" type="button">Edit</button>
            ${canEdit ? `<button class="btn-secondary" data-asm-share-skill="${esc(s.name)}" data-shared="${s.is_shared ? "true" : "false"}" type="button">${s.is_shared ? "Unshare" : "Share"}</button>` : ""}
            ${canEdit ? `<button class="btn-danger" data-asm-del-skill="${esc(s.name)}" type="button">Delete</button>` : ""}
          </div>
        </div>
      `).join("");

      const compRows = (sel.companion_files || []).map((c) => `
        <div class="row">
          <code>${esc(c.relative_path)}</code>
          ${canEdit && sel.name ? `<button class="btn-danger" data-asm-del-comp="${esc(c.relative_path)}" type="button">Remove</button>` : ""}
        </div>
      `).join("");

      bodyHtml = `
        <div class="admin-grid">
          <div class="panel-card">
            <h3>Skills (${esc(PROVIDER_LABELS[state.asmProvider])})</h3>
            ${skillRows || "<p class='subtitle'>No skills found in this scope.</p>"}
          </div>
          <div class="panel-card">
            <form id="asm-skill-form">
              <h3>Skill &amp; Companion Editor</h3>
              <div class="form-group"><label>Skill Name</label><input id="asm-skill-name" value="${esc(sel.name || "")}" required ${readOnlyAttr} /></div>
              <label class="checkbox-item" style="margin-bottom: 10px;">
                <input type="checkbox" id="asm-skill-share-toggle" ${sel.is_shared ? "checked" : ""} ${readOnlyAttr} />
                Share Skill across CLIs
              </label>
              <div class="form-group"><label>SKILL.md</label><textarea id="asm-skill-md" rows="8" ${readOnlyAttr}>${esc(sel.skill_md || "")}</textarea></div>
              ${canEdit ? `<button class="btn-primary" type="submit">Save Skill</button>` : ""}
            </form>
            <div style="margin-top: 16px;">
              <h4>Companion Files (scripts/, references/, examples/)</h4>
              ${compRows || "<p class='subtitle'>No companion files yet.</p>"}
              ${canEdit ? `
                <form id="asm-comp-form" style="margin-top: 10px;">
                  <div class="form-group"><label>Relative Path</label><input id="asm-comp-path" value="scripts/helper.sh" required /></div>
                  <div class="form-group"><label>Content</label><textarea id="asm-comp-content" rows="4"></textarea></div>
                  <button class="btn-secondary" type="submit">Save Companion File</button>
                </form>` : ""}
            </div>
          </div>
        </div>`;
    } else {
      const cfg = state.asmMcpConfig || { config_path: "", servers: [] };
      const srvRows = (cfg.servers || []).map((s) => `
        <div class="row">
          <div>
            <strong>${esc(s.name)}</strong>
            <div class="project-path"><code>${esc(s.command || s.url || "")} ${esc((s.args || []).join(" "))}</code></div>
          </div>
          <div class="btn-group">
            ${canEdit ? `<button class="btn-secondary" data-asm-sync-mcp="${esc(s.name)}" type="button">Sync across CLIs</button>` : ""}
            ${canEdit ? `<button class="btn-danger" data-asm-del-mcp="${esc(s.name)}" type="button">Delete</button>` : ""}
          </div>
        </div>
      `).join("");

      const presetButtons = (state.asmPresets.length > 0 ? state.asmPresets : [
        { id: "graphify-mcp", display_name: "Graphify Knowledge Graph (graphify-mcp)" },
        { id: "chrome-devtools", display_name: "Chrome DevTools MCP" },
        { id: "playwright", display_name: "Playwright Browser MCP" },
      ]).map((p) => `
        <button class="btn-secondary" data-asm-preset="${esc(p.id)}" type="button" ${readOnlyAttr}>+ ${esc(p.display_name)}</button>
      `).join("");

      bodyHtml = `
        <div class="admin-grid">
          <div class="panel-card">
            <div class="panel-header-row">
              <h3>Native MCP Servers</h3>
              ${state.asmScope === "project" && canEdit ? `<button class="btn-secondary" id="asm-run-graphify-btn" type="button">Run Graphify Index</button>` : ""}
            </div>
            <p class="project-path">${esc(cfg.config_path || "")}</p>
            ${srvRows || "<p class='subtitle'>No MCP servers configured yet.</p>"}
            <h4 style="margin-top: 14px;">One-Click Presets</h4>
            <div class="btn-group">${presetButtons}</div>
            ${state.asmGraphifyResult ? `<pre class="xterm-fallback-log" style="margin-top: 12px;">${esc(state.asmGraphifyResult)}</pre>` : ""}
          </div>
          <form class="panel-card" id="asm-mcp-form">
            <h3>Add / Update MCP Server</h3>
            <div class="form-group"><label>Server Name</label><input id="asm-mcp-name" placeholder="my-mcp" required ${readOnlyAttr} /></div>
            <div class="form-group"><label>Command</label><input id="asm-mcp-cmd" placeholder="npx" ${readOnlyAttr} /></div>
            <div class="form-group"><label>Args (space-separated)</label><input id="asm-mcp-args" placeholder="-y @playwright/mcp@latest" ${readOnlyAttr} /></div>
            ${canEdit ? `<button class="btn-primary" type="submit">Save MCP Server</button>` : ""}
          </form>
        </div>`;
    }

    return `
      <section class="panel-card">
        <div class="panel-header-row">
          <div>
            <h2>Agents, Skills &amp; MCP Manager</h2>
            <p class="subtitle">Native CLI subagents, skills with cross-CLI symlink sharing, and MCP presets.</p>
          </div>
          <div class="tab-bar">
            <button class="btn-secondary ${state.asmScope === "project" ? "active" : ""}" data-asm-scope="project" type="button">Project Scope</button>
            <button class="btn-secondary ${state.asmScope === "global" ? "active" : ""}" data-asm-scope="global" type="button">Global Scope</button>
          </div>
        </div>
        <div class="tab-bar">${provTabs}</div>
        <div class="tab-bar">
          <button class="btn-secondary ${state.asmSubTab === "agents" ? "active" : ""}" data-asm-sub="agents" type="button">Subagents</button>
          <button class="btn-secondary ${state.asmSubTab === "skills" ? "active" : ""}" data-asm-sub="skills" type="button">Skills</button>
          <button class="btn-secondary ${state.asmSubTab === "mcp" ? "active" : ""}" data-asm-sub="mcp" type="button">MCP Servers</button>
        </div>
      </section>
      ${bodyHtml}`;
  }

  function bindDashboardHandlers() {
    bindKanbanHandlers();
    document.querySelectorAll("[data-pid]").forEach((btn) => {
      btn.onclick = async () => {
        state.activeProjectId = btn.dataset.pid;
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
      };
    });

    const projTabChat = document.getElementById("proj-tab-chat");
    if (projTabChat) projTabChat.onclick = () => { state.projectTab = "chat"; render(); };

    const projTabFiles = document.getElementById("proj-tab-files");
    if (projTabFiles) projTabFiles.onclick = () => { state.projectTab = "files"; render(); };

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

    document.querySelectorAll("[data-sid]").forEach((btn) => {
      btn.onclick = async () => {
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

    const providerSel = document.getElementById("header-cli-provider");
    const accountSel = document.getElementById("header-cli-account");
    if (providerSel) {
      providerSel.onchange = async () => {
        state.error = null; state.notice = null;
        try {
          state.projectCli = await api(`/api/projects/${state.activeProjectId}/cli`, {
            method: "PUT",
            body: JSON.stringify({ provider: providerSel.value, account_id: null }),
          });
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }
    if (accountSel) {
      accountSel.onchange = async () => {
        state.error = null; state.notice = null;
        try {
          state.projectCli = await api(`/api/projects/${state.activeProjectId}/cli`, {
            method: "PUT",
            body: JSON.stringify({
              provider: providerSel.value,
              account_id: accountSel.value || null,
            }),
          });
          render();
        } catch (err) { state.error = err.message; render(); }
      };
    }

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

    const cpForm = document.getElementById("create-project-form");
    if (cpForm) {
      cpForm.onsubmit = async (e) => {
        e.preventDefault();
        state.error = null; state.notice = null;
        const fd = new FormData(e.target);
        try {
          await api("/api/projects", { method: "POST", body: JSON.stringify(Object.fromEntries(fd.entries())) });
          state.notice = "Project registered.";
          await loadAuthenticatedData();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    const ciForm = document.getElementById("create-invite-form");
    if (ciForm) {
      ciForm.onsubmit = async (e) => {
        e.preventDefault();
        state.error = null; state.notice = null;
        const role = e.target.role.value;
        const project_ids = Array.from(e.target.project_ids.selectedOptions).map((o) => o.value);
        try {
          const inv = await api("/api/admin/invites", { method: "POST", body: JSON.stringify({ role, project_ids }) });
          state.notice = `Created invite code: ${inv.code}`;
          await loadAuthenticatedData();
        } catch (err) { state.error = err.message; render(); }
      };
    }

    document.querySelectorAll("[data-revoke-uid]").forEach((btn) => {
      btn.onclick = async () => {
        state.error = null; state.notice = null;
        try {
          await api(`/api/admin/users/${btn.dataset.revokeUid}/revoke-sessions`, { method: "POST" });
          state.notice = "User sessions revoked.";
          await loadAuthenticatedData();
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

  setInterval(() => { if (state.projectTab === "kanban" && state.user && !kanban.editing && !document.querySelector(".kanban-card:active")) void loadKanban().catch(() => {}); }, 1500);
  boot();
})();
