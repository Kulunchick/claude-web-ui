(() => {
  const state = {
    initialized: true,
    screen: "login",
    accessToken: localStorage.getItem("cwu_access_token") || null,
    refreshToken: localStorage.getItem("cwu_refresh_token") || null,
    user: null,
    projects: [],
    activeProjectId: null,
    tab: "overview",
    adminUsers: [],
    adminInvites: [],
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
  };

  const app = document.getElementById("app");

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

  function saveTokens(auth) {
    state.accessToken = auth.access_token;
    state.refreshToken = auth.refresh_token;
    state.user = auth.user;
    localStorage.setItem("cwu_access_token", auth.access_token);
    localStorage.setItem("cwu_refresh_token", auth.refresh_token);
  }

  function clearSession() {
    state.accessToken = null;
    state.refreshToken = null;
    state.user = null;
    state.projects = [];
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

  async function loadAuthenticatedData() {
    const me = await api("/api/auth/me");
    state.user = me.user;
    state.projects = me.projects || [];
    if (!state.activeProjectId && state.projects.length > 0) {
      state.activeProjectId = state.projects[0].id;
    }
    if (state.user.role === "admin") {
      state.adminUsers = await api("/api/admin/users");
      state.adminInvites = await api("/api/admin/invites");
    }
    if (state.activeProjectId) {
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
              ${collapsed ? "▸ 📁" : "▾ 📂"} ${esc(node.name)}
            </button>
            ${collapsed ? "" : renderTreeNodes(node.children || [], depth + 1)}
          </li>`;
      }
      const isSelected = state.activeFile && state.activeFile.path === node.path;
      return `
        <li>
          <button class="tree-node-btn ${isSelected ? "active" : ""}" style="padding-left:${8 + pad}px" data-file-open="${esc(node.path)}">
            📄 ${esc(node.name)}
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

  function renderDashboard() {
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
          <div><code>${esc(inv.code)}</code> — ${esc(inv.role)}</div>
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

      mainHtml = `
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
          ${isAdmin ? `
            <div class="sidebar-section">
              <button class="nav-item ${state.tab === "admin" ? "active" : ""}" id="nav-admin">Admin: Users &amp; Projects</button>
            </div>` : ""}
          <div class="sidebar-section">
            <button class="btn-secondary" id="logout-btn">Sign Out (${esc(state.user.username)})</button>
          </div>
        </aside>
        <main class="dashboard-main">
          ${errHtml}${okHtml}${mainHtml}
        </main>
      </div>
    `;

    document.querySelectorAll("[data-pid]").forEach((btn) => {
      btn.onclick = async () => {
        state.activeProjectId = btn.dataset.pid;
        state.tab = "overview";
        state.activeFile = null;
        state.editorText = "";
        state.editorDirty = false;
        state.diffPath = null;
        await loadProjectWorkspace();
        render();
      };
    });

    const toggleGi = document.getElementById("toggle-gitignore");
    if (toggleGi) {
      toggleGi.onchange = async () => {
        state.respectGitignore = toggleGi.checked;
        await loadProjectWorkspace();
        render();
      };
    }

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

    document.getElementById("logout-btn").onclick = async () => {
      try { await api("/api/auth/logout", { method: "POST" }); } catch {}
      clearSession();
      state.screen = "login";
      render();
    };

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
  }

  boot();
})();
