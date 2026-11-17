const { invoke } = window.__TAURI__.core;
const select = document.getElementById("profile");
const status = document.getElementById("status");
const credentials = document.getElementById("credentials");
const signin = document.getElementById("signin");
const jwtSettings = document.getElementById("jwt-settings");
const add = document.getElementById("add");
const open = document.getElementById("open");

async function refreshProfiles() {
  const { profiles, active_id } = await invoke("profiles");
  select.replaceChildren();
  for (const profile of profiles) {
    select.add(new Option(profile.name + " (" + profile.origin + ")", profile.id));
  }
  if (active_id) select.value = active_id;
  select.disabled = !active_id;
  signin.hidden = !active_id;
  jwtSettings.hidden = !active_id;
}

async function action(callback, message = "") {
  for (const form of [signin, credentials, add]) form.inert = true;
  select.disabled = true;
  status.textContent = "";
  try {
    await callback();
    await refreshProfiles();
    status.textContent = message;
  } catch (error) {
    status.textContent = String(error);
    await refreshProfiles().catch(() => {});
  } finally {
    for (const form of [signin, credentials, add]) form.inert = false;
  }
}

select.addEventListener("change", () => {
  credentials.reset();
  signin.reset();
  action(() => invoke("select_profile", { id: select.value }));
});

signin.addEventListener("submit", event => {
  event.preventDefault();
  const identifier = document.getElementById("identifier").value;
  const password = document.getElementById("password").value;
  action(async () => {
    await invoke("sign_in", { id: select.value, identifier, password });
    signin.reset();
    await invoke("open_dashboard");
  }, "Signed in.");
});

credentials.addEventListener("submit", event => {
  event.preventDefault();
  const accessToken = document.getElementById("access-token").value;
  const refreshToken = document.getElementById("refresh-token").value;
  action(async () => {
    await invoke("save_credentials", { id: select.value, accessToken, refreshToken });
    credentials.reset();
  }, "Credentials saved.");
});

add.addEventListener("submit", event => {
  event.preventDefault();
  const name = document.getElementById("name").value;
  const origin = document.getElementById("origin").value;
  action(async () => {
    await invoke("add_profile", { name, origin });
    add.reset();
    credentials.reset();
  }, "Gateway added.");
});

open.addEventListener("click", () => action(() => invoke("open_dashboard")));
action(async () => {});
