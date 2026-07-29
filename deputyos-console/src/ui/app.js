// deputyOS Console frontend. Deliberately framework-free: the shipped app has
// no CDN or JavaScript supply-chain dependency.

function invoke(cmd, args) {
  const tauri = window.__TAURI__;
  const fn = (tauri && (tauri.core ? tauri.core.invoke : tauri.invoke)) || null;
  return fn ? fn(cmd, args) : Promise.reject(new Error("Tauri bridge unavailable"));
}

const $ = (selector) => document.querySelector(selector);
const node = (tag, className, text) => {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
};

let instances = [];
let authenticated = false;
let hostInfo = null;
let loginPollTimer = null;
const statuses = new Map();
const healthReports = new Map();

function errorText(error) {
  if (typeof error === "string") return error;
  return error && error.message ? error.message : String(error);
}

function toast(message, isError = false) {
  const target = $("#toast");
  target.textContent = message;
  target.classList.toggle("err", isError);
  target.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => target.classList.add("hidden"), 4200);
}

function liveState(status, fallback = "stopped") {
  if (status && typeof status === "object") {
    if ("Running" in status) return "running";
    if ("Paused" in status) return "paused";
  }
  if (status === "Stopped") return "stopped";
  return String(fallback || "stopped").toLowerCase();
}

function formatMemory(mib) {
  if (!Number.isFinite(Number(mib))) return "—";
  return Number(mib) >= 1024
    ? `${(Number(mib) / 1024).toFixed(Number(mib) % 1024 ? 1 : 0)} GB`
    : `${mib} MiB`;
}

function formatBytes(bytes) {
  if (!Number.isFinite(Number(bytes))) return "—";
  return `${(Number(bytes) / 1073741824).toFixed(1)} GB`;
}

async function perform(button, promise, successMessage) {
  if (button) button.disabled = true;
  try {
    const result = await promise;
    if (successMessage) toast(successMessage);
    return result;
  } catch (error) {
    toast(errorText(error), true);
    throw error;
  } finally {
    if (button) button.disabled = false;
  }
}

// ---- account / remote authentication ----

async function refreshAccount() {
  const status = await invoke("login_status").catch(() => null);
  authenticated = !!status && status.status === "authorized";
  $("#account-label").textContent = authenticated
    ? (status.account_id ? `Account ${status.account_id.slice(0, 8)}` : "Signed in")
    : "Local mode";
  $("#login-open-btn").classList.toggle("hidden", authenticated);
  $("#logout-btn").classList.toggle("hidden", !authenticated);
  $("#fleet-signin").classList.toggle("hidden", authenticated);
  if (authenticated) refreshFleet();
  else $("#fleet-grid").replaceChildren();
}

async function startLogin() {
  const button = $("#login-start-btn");
  const started = await perform(
    button,
    invoke("login_start", { clientName: "deputyos-console" }),
    null
  ).catch(() => null);
  if (!started) return;
  button.classList.add("hidden");
  $("#login-code").classList.remove("hidden");
  $("#login-uri").textContent = started.verification_uri;
  $("#login-uri").href = started.verification_uri;
  $("#login-usercode").textContent = started.user_code;
  clearInterval(loginPollTimer);
  loginPollTimer = setInterval(async () => {
    const status = await invoke("login_poll").catch(() => null);
    if (status && status.status === "authorized") {
      clearInterval(loginPollTimer);
      $("#login-pending").textContent = "Confirmed. Loading your fleet…";
      setTimeout(() => $("#login-dialog").close(), 500);
      refreshAccount();
    }
  }, (started.interval || 5) * 1000);
}

async function logout() {
  await perform($("#logout-btn"), invoke("logout"), "Signed out").catch(() => null);
  $("#login-start-btn").classList.remove("hidden");
  $("#login-code").classList.add("hidden");
  $("#login-pending").textContent = "Waiting for confirmation…";
  refreshAccount();
}

// ---- local runtime ----

async function refreshHost() {
  hostInfo = await invoke("host_prereq").catch((error) => ({
    ok: false,
    target: "unknown",
    message: errorText(error),
    capabilities: {},
  }));
  $("#host-label").textContent = hostInfo.ok ? "Runtime ready" : "Runtime needs attention";
  $("#host-target").textContent = hostInfo.target || "unknown target";
  $("#host-dot").className = `health-dot ${hostInfo.ok ? "ok" : "err"}`;
  const banner = $("#local-host-note");
  banner.textContent = hostInfo.message || "";
  banner.classList.toggle("hidden", hostInfo.ok || !hostInfo.message);
  renderInstances();
}

async function refreshLocal() {
  instances = await invoke("list_instances").catch((error) => {
    toast(errorText(error), true);
    return [];
  });
  updateMetrics();
  renderInstances();

  await Promise.all(instances.map(async (instance) => {
    const status = await invoke("status_instance", { id: instance.id }).catch(() => null);
    if (status !== null) statuses.set(instance.id, status);
  }));

  const canQueryAgent = !hostInfo || hostInfo.capabilities?.guest_agent !== false;
  if (canQueryAgent) {
    await Promise.all(instances.map(async (instance) => {
      if (liveState(statuses.get(instance.id), instance.last_status) !== "running") {
        healthReports.delete(instance.id);
        return;
      }
      const health = await invoke("instance_agent_health", { id: instance.id }).catch(() => null);
      if (health) healthReports.set(instance.id, health);
    }));
  }
  updateMetrics();
  renderInstances();
}

function updateMetrics() {
  let running = 0;
  let paused = 0;
  let memoryMib = 0;
  for (const instance of instances) {
    const state = liveState(statuses.get(instance.id), instance.last_status);
    if (state === "running") running += 1;
    if (state === "paused") paused += 1;
    memoryMib += Number(instance.resources?.memory_max_mib || 0);
  }
  $("#metric-total").textContent = String(instances.length);
  $("#metric-running").textContent = String(running);
  $("#metric-paused").textContent = String(paused);
  $("#metric-memory").textContent = formatMemory(memoryMib);
}

function renderInstances() {
  const grid = $("#instances-grid");
  grid.replaceChildren();
  if (!instances.length) {
    const empty = node("section", "empty-panel");
    empty.append(
      node("span", "empty-mark", "◇"),
      node("h3", "", "Create your first deputy"),
      node("p", "", "Each deputy gets its own image, resources, resident agent, and tunnel.")
    );
    const create = node("button", "primary", "New deputy");
    create.onclick = () => $("#create-dialog").showModal();
    empty.appendChild(create);
    grid.appendChild(empty);
    return;
  }
  instances.forEach((instance) => grid.appendChild(instanceCard(instance)));
}

function instanceCard(instance) {
  const status = statuses.get(instance.id);
  const state = liveState(status, instance.last_status);
  const resources = instance.resources || {};
  const health = healthReports.get(instance.id);
  const report = health && health.kind === "health" ? health.report : null;
  const card = node("article", `instance-card ${state}`);

  const top = node("div", "card-top");
  const identity = node("div", "instance-identity");
  identity.appendChild(node("span", "instance-glyph", "◇"));
  const identityText = node("div");
  identityText.append(
    node("h4", "", instance.name),
    node("p", "mono", `${instance.profile || "default"} · ${instance.target}`)
  );
  identity.appendChild(identityText);
  top.append(identity, node("span", `status-pill ${state}`, state));
  card.appendChild(top);

  const resourceRow = node("div", "resource-row");
  resourceRow.append(
    resourceTile("PROCESSORS", `${resources.vcpus || 2} vCPU`),
    resourceTile(
      "MEMORY ENVELOPE",
      `${formatMemory(resources.memory_min_mib || 1024)} – ${formatMemory(resources.memory_max_mib || 4096)}`
    )
  );
  card.appendChild(resourceRow);

  const agentLine = node("div", "agent-line");
  const agentState = node(
    "span",
    `agent-state ${report ? "ok" : ""}`,
    report ? `Resident agent v${report.agent_version}` : (state === "running" ? "Agent connecting…" : "Resident agent idle")
  );
  const memory = report?.memory;
  agentLine.append(
    agentState,
    node("span", "", memory?.available_bytes ? `${formatBytes(memory.available_bytes)} available` : "typed control")
  );
  card.appendChild(agentLine);

  const memoryBar = node("div", "memory-bar");
  const memoryFill = node("span");
  if (memory?.total_bytes && memory?.available_bytes) {
    const used = 100 * (1 - Number(memory.available_bytes) / Number(memory.total_bytes));
    memoryFill.style.width = `${Math.max(2, Math.min(100, used))}%`;
  }
  memoryBar.appendChild(memoryFill);
  card.appendChild(memoryBar);

  const actions = node("div", "card-actions");
  if (state === "running") {
    actions.append(
      actionButton("Open", "open", (button) => openWizard(instance.id, button)),
      ...(hostInfo?.capabilities?.pause_resume === false
        ? []
        : [actionButton("Pause", "", (button) => lifecycle(button, "pause_instance", instance.id, "Paused"))]),
      actionButton("Stop", "", (button) => lifecycle(button, "stop_instance", instance.id, "Stopped"))
    );
  } else if (state === "paused") {
    actions.append(
      actionButton("Resume", "open", (button) => lifecycle(button, "resume_instance", instance.id, "Resumed")),
      actionButton("Stop", "", (button) => lifecycle(button, "stop_instance", instance.id, "Stopped"))
    );
  } else {
    actions.append(
      actionButton("Install", "", (button) => lifecycle(button, "install_instance", instance.id, "Installed")),
      actionButton("Start", "open", (button) => lifecycle(button, "start_instance", instance.id, "Started"))
    );
  }
  actions.appendChild(actionButton("×", "danger", () => deleteInstance(instance)));
  card.appendChild(actions);
  return card;
}

function resourceTile(label, value) {
  const tile = node("div", "resource");
  tile.append(node("small", "", label), node("strong", "", value));
  return tile;
}

function actionButton(label, className, handler) {
  const button = node("button", className, label);
  button.onclick = () => handler(button);
  return button;
}

async function lifecycle(button, command, id, message) {
  await perform(button, invoke(command, { id }), message).catch(() => null);
  await refreshLocal();
}

async function deleteInstance(instance) {
  if (!confirm(`Delete "${instance.name}" and its instance files?`)) return;
  await perform(null, invoke("delete_instance", { id: instance.id }), "Deputy deleted").catch(() => null);
  statuses.delete(instance.id);
  healthReports.delete(instance.id);
  await refreshLocal();
}

async function createInstance(event) {
  event.preventDefault();
  const name = $("#new-name").value.trim();
  const profile = $("#new-profile").value.trim() || null;
  const resources = {
    vcpus: Number($("#new-vcpus").value),
    memory_min_mib: Number($("#new-memory-min").value),
    memory_max_mib: Number($("#new-memory-max").value),
    auto_balloon: $("#new-auto-balloon").checked,
  };
  if (!name) return toast("A name is required.", true);
  if (resources.memory_max_mib < resources.memory_min_mib) {
    return toast("Maximum memory must be at least the minimum.", true);
  }
  const button = $("#create-btn");
  const created = await perform(
    button,
    invoke("create_instance", { name, profile, manifestUrl: null, channel: null }),
    null
  ).catch(() => null);
  if (!created) return;
  const configured = await perform(
    button,
    invoke("configure_instance_resources", { id: created.id, resources }),
    "Deputy created"
  ).catch(() => null);
  if (!configured) return;
  $("#create-dialog").close();
  $("#create-form").reset();
  $("#new-vcpus").value = "2";
  $("#new-memory-min").value = "1024";
  $("#new-memory-max").value = "4096";
  $("#new-auto-balloon").checked = true;
  await refreshLocal();
}

async function openWizard(id, button) {
  const url = await perform(button, invoke("open_wizard", { id }), null).catch(() => null);
  if (!url) return;
  await invoke("open_url", { url }).catch(() => window.open(url, "_blank"));
}

// ---- remote fleet ----

async function refreshFleet() {
  if (!authenticated) return;
  const devices = await invoke("list_fleet").catch((error) => {
    toast(errorText(error), true);
    return [];
  });
  const grid = $("#fleet-grid");
  grid.replaceChildren();
  if (!devices.length) {
    const empty = node("section", "empty-panel");
    empty.append(
      node("span", "empty-mark", "◎"),
      node("h3", "", "No remote deputies yet"),
      node("p", "", "Register an image to your account and its outbound tunnel will appear here.")
    );
    grid.appendChild(empty);
    return;
  }
  devices.forEach((device) => {
    const revoked = !!device.revoked_at;
    const online = !revoked && !!device.tunnel_online;
    const card = node("article", `instance-card ${online ? "running" : ""}`);
    const top = node("div", "card-top");
    const identity = node("div", "instance-identity");
    identity.append(node("span", "instance-glyph", "◎"));
    const text = node("div");
    text.append(node("h4", "", device.name), node("p", "mono", device.id));
    identity.appendChild(text);
    top.append(identity, node("span", `status-pill ${online ? "running" : ""}`, revoked ? "revoked" : online ? "online" : "offline"));
    card.append(top);
    const details = node("div", "resource-row");
    details.append(
      resourceTile("CREATED", device.created_at || "—"),
      resourceTile("ACCESS", "AccountOwner JWT")
    );
    card.appendChild(details);
    const line = node("div", "agent-line");
    line.append(
      node("span", `agent-state ${online ? "ok" : ""}`, revoked ? "Tunnel disabled" : online ? "Authenticated tunnel online" : "Tunnel reconnecting"),
      node("span", "", "No inbound port")
    );
    card.appendChild(line);
    const spacer = node("div", "memory-bar");
    spacer.appendChild(node("span"));
    card.appendChild(spacer);
    const actions = node("div", "card-actions");
    const webui = actionButton("Open WebUI", "open", (button) => openRemote(device.id, "webui", button));
    const terminal = actionButton("Terminal", "", (button) => openRemote(device.id, "terminal", button));
    const control = actionButton("System", "", (button) => openRemote(device.id, "control", button));
    webui.disabled = !online;
    terminal.disabled = !online;
    control.disabled = !online;
    actions.append(webui, terminal, control);
    card.appendChild(actions);
    const agentActions = node("div", "card-actions");
    agentActions.append(
      actionButton("Health check", "", (button) => queueRemote(device.id, "agent.health", button)),
      actionButton("Self-heal", "", (button) => queueRemote(device.id, "repair.run", button)),
      actionButton("Update", "", (button) => queueRemote(device.id, "update.run", button)),
      actionButton("Back up", "", (button) => queueRemote(device.id, "backup.run", button)),
      actionButton("Restart agent", "", (button) => queueRemote(device.id, "workload.restart", button))
    );
    card.appendChild(agentActions);
    grid.appendChild(card);
  });
}

async function openRemote(deviceId, surface, button) {
  const url = await perform(
    button,
    invoke("open_remote_surface", { deviceId, surface }),
    null
  ).catch(() => null);
  if (url) await invoke("open_url", { url }).catch(() => window.open(url, "_blank"));
}

async function queueRemote(deviceId, command, button) {
  const queued = await perform(
    button,
    invoke("queue_remote_command", { deviceId, command }),
    null
  ).catch(() => null);
  if (queued) toast(`${command} queued · ${queued.id}`);
}

// ---- navigation / bootstrap ----

function switchView(view) {
  document.querySelectorAll(".nav-item").forEach((item) =>
    item.classList.toggle("active", item.dataset.view === view)
  );
  $("#view-local").classList.toggle("hidden", view !== "local");
  $("#view-fleet").classList.toggle("hidden", view !== "fleet");
  $("#page-eyebrow").textContent = view === "local" ? "LOCAL RUNTIME" : "REMOTE ACCESS";
  $("#page-title").textContent = view === "local" ? "Your deputies" : "Fleet";
  if (view === "fleet") refreshFleet();
}

document.addEventListener("DOMContentLoaded", async () => {
  document.querySelectorAll(".nav-item").forEach((item) =>
    item.addEventListener("click", () => switchView(item.dataset.view))
  );
  $("#create-open-btn").onclick = () => $("#create-dialog").showModal();
  $("#create-form").addEventListener("submit", createInstance);
  $("#local-refresh").onclick = refreshLocal;
  $("#fleet-refresh").onclick = refreshFleet;
  $("#login-open-btn").onclick = () => $("#login-dialog").showModal();
  $("#fleet-login-btn").onclick = () => $("#login-dialog").showModal();
  $("#login-start-btn").onclick = startLogin;
  $("#logout-btn").onclick = logout;
  document.querySelectorAll(".dialog-close").forEach((button) => {
    button.onclick = () => button.closest("dialog").close();
  });
  await Promise.all([refreshHost(), refreshAccount()]);
  await refreshLocal();
  setInterval(() => {
    if (!$("#view-local").classList.contains("hidden")) refreshLocal();
  }, 8000);
});
