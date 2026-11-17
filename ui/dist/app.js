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
    render();
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
    } else {
      mainHtml = `
        <section class="panel-card">
          <h2>${activeProject ? esc(activeProject.name) : "No Project Selected"}</h2>
          <p class="subtitle">${activeProject ? esc(activeProject.workspace_path) : "Ask an Admin to assign you to a project workspace."}</p>
        </section>
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
      btn.onclick = () => {
        state.activeProjectId = btn.dataset.pid;
        state.tab = "overview";
        render();
      };
    });

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
