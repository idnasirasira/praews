const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const servicesEl = document.getElementById("services");
const sitesEl = document.getElementById("sites");
const lanEl = document.getElementById("lan");
const rootEl = document.getElementById("root");
const progressEl = document.getElementById("progress");
const phpSelect = document.getElementById("php-version");

function lampClass(status) {
  if (!status) return "bad";
  if (typeof status === "string") return status === "Stopped" ? "bad" : "ok";
  if (status.Running) return "ok";
  if (status.Error) return "bad";
  return "bad";
}

function statusText(status) {
  if (!status) return "unknown";
  if (typeof status === "string") return status;
  if (status.Running) return `running pid ${status.Running.pid} :${status.Running.port}`;
  if (status.Error) return `error: ${status.Error}`;
  if (status.Starting !== undefined) return "starting";
  return JSON.stringify(status);
}

async function refreshInfo() {
  const info = await invoke("get_app_info");
  lanEl.textContent = `LAN ${info.lan_ip}`;
  rootEl.textContent = `Sandbox: ${info.app_root}`;
  if (info.settings?.active_php_version) {
    phpSelect.value = info.settings.active_php_version;
  }
  sitesEl.innerHTML = "";
  for (const site of info.settings?.vhosts || []) {
    const li = document.createElement("li");
    li.innerHTML = `<span>${site.domain}</span><span>${site.public_path}</span>`;
    sitesEl.appendChild(li);
  }
}

async function refreshStatuses() {
  const statuses = await invoke("get_service_statuses");
  servicesEl.innerHTML = "";
  for (const name of ["Nginx", "Php", "MySql"]) {
    const status = statuses[name] || "Stopped";
    const li = document.createElement("li");
    li.innerHTML = `<span><span class="lamp ${lampClass(status)}"></span>${name}</span><span>${statusText(status)}</span>`;
    servicesEl.appendChild(li);
  }
}

document.getElementById("btn-start").onclick = async () => {
  progressEl.textContent = "Starting…";
  try {
    await invoke("start_all_services");
    progressEl.textContent = "Services started (or attempted).";
  } catch (err) {
    progressEl.textContent = String(err);
  }
  await refreshStatuses();
};

document.getElementById("btn-stop").onclick = async () => {
  await invoke("stop_all_services");
  progressEl.textContent = "Stopped.";
  await refreshStatuses();
};

document.getElementById("btn-switch-php").onclick = async () => {
  try {
    await invoke("switch_php_version", { version: phpSelect.value });
    progressEl.textContent = `PHP switched to ${phpSelect.value}`;
  } catch (err) {
    progressEl.textContent = String(err);
  }
  await refreshInfo();
  await refreshStatuses();
};

document.getElementById("btn-add-site").onclick = async () => {
  const domain = document.getElementById("site-domain").value.trim();
  const projectPath = document.getElementById("site-path").value.trim();
  if (!domain || !projectPath) {
    progressEl.textContent = "Domain and path required.";
    return;
  }
  try {
    await invoke("create_virtual_host", { domain, projectPath });
    progressEl.textContent = `Site ${domain} added`;
  } catch (err) {
    progressEl.textContent = String(err);
  }
  await refreshInfo();
};

document.getElementById("btn-trust-ca").onclick = async () => {
  try {
    await invoke("trust_local_root_ca");
    progressEl.textContent = "Root CA trust requested.";
  } catch (err) {
    progressEl.textContent = String(err);
  }
};

document.getElementById("btn-open-term").onclick = async () => {
  const info = await invoke("get_app_info");
  const path =
    document.getElementById("site-path").value.trim() || info.app_root;
  try {
    await invoke("open_project_terminal", { projectPath: path });
  } catch (err) {
    progressEl.textContent = String(err);
  }
};

listen("download-progress", (event) => {
  const p = event.payload;
  progressEl.textContent = `${p.component}: ${p.percentage.toFixed(1)}%`;
});

listen("services-changed", async () => {
  await refreshStatuses();
});

refreshInfo().then(refreshStatuses).catch((err) => {
  progressEl.textContent = String(err);
});
setInterval(() => {
  refreshStatuses().catch(() => {});
}, 2500);
