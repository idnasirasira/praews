const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const servicesEl = document.getElementById("services");
const sitesEl = document.getElementById("sites");
const lanEl = document.getElementById("lan");
const rootEl = document.getElementById("root");
const progressEl = document.getElementById("progress");
const statusFill = document.getElementById("status-fill");
const phpSelect = document.getElementById("php-version");
const logsEl = document.getElementById("logs");
const dnsHintEl = document.getElementById("dns-hint");
const mobileLink = document.getElementById("mobile-link");
const rootCaLink = document.getElementById("root-ca-link");

function lampClass(status) {
  if (!status) return "bad";
  if (typeof status === "string") return status === "Stopped" ? "bad" : "ok";
  if (status.Running) return "ok";
  if (status.Error) return "bad";
  if (status.Starting !== undefined) return "warn";
  return "bad";
}

function statusText(status) {
  if (!status) return "—";
  if (typeof status === "string") return status.toLowerCase();
  if (status.Running) return `pid ${status.Running.pid} · :${status.Running.port}`;
  if (status.Error) return String(status.Error);
  if (status.Starting !== undefined) return "starting";
  return "—";
}

function setProgress(text, kind = "") {
  progressEl.textContent = text;
  progressEl.classList.remove("is-error", "is-ok");
  if (kind === "error") progressEl.classList.add("is-error");
  if (kind === "ok") progressEl.classList.add("is-ok");
}

function setBusy(busy, pct = null) {
  statusFill.classList.toggle("indeterminate", busy && pct == null);
  if (!busy) {
    statusFill.style.width = "0%";
    statusFill.classList.remove("indeterminate");
    return;
  }
  if (pct != null) {
    statusFill.classList.remove("indeterminate");
    statusFill.style.width = `${Math.max(0, Math.min(100, pct))}%`;
  }
}

async function refreshInfo() {
  const info = await invoke("get_app_info");
  lanEl.textContent = `LAN ${info.lan_ip}`;
  rootEl.textContent = info.app_root;
  dnsHintEl.textContent = info.dns_hint || "Set phone DNS to this machine, or use sslip.io.";
  if (info.mobile_url) {
    mobileLink.href = info.mobile_url;
    mobileLink.textContent = info.mobile_url;
  }
  if (info.root_ca_url) {
    rootCaLink.href = info.root_ca_url;
  }
  if (info.settings?.active_php_version) {
    phpSelect.value = info.settings.active_php_version;
  }
  sitesEl.innerHTML = "";
  for (const site of info.settings?.vhosts || []) {
    const li = document.createElement("li");
    li.innerHTML = `<span class="domain">${site.domain}</span><span class="path" title="${site.public_path}">${site.public_path}</span>`;
    sitesEl.appendChild(li);
  }
}

async function refreshStatuses() {
  const statuses = await invoke("get_service_statuses");
  servicesEl.innerHTML = "";
  const labels = { Nginx: "Nginx", Php: "PHP", MySql: "MariaDB" };
  for (const name of ["Nginx", "Php", "MySql"]) {
    const status = statuses[name] || "Stopped";
    const li = document.createElement("li");
    li.innerHTML = `
      <span class="lamp ${lampClass(status)}"></span>
      <span class="svc-name">${labels[name]}</span>
      <span class="svc-detail">${statusText(status)}</span>`;
    servicesEl.appendChild(li);
  }
}

async function refreshLogs() {
  logsEl.textContent = await invoke("read_recent_logs", { lines: 120 });
}

document.getElementById("btn-start").onclick = async () => {
  setBusy(true);
  setProgress("Starting stack…");
  try {
    await invoke("start_all_services");
    setProgress("Stack is up.", "ok");
  } catch (err) {
    setProgress(String(err), "error");
  }
  setBusy(false);
  await refreshStatuses();
  await refreshLogs();
  logsEl.scrollTop = logsEl.scrollHeight;
};

document.getElementById("btn-stop").onclick = async () => {
  setBusy(true);
  setProgress("Stopping…");
  try {
    await invoke("stop_all_services");
    setProgress("Stopped.", "ok");
  } catch (err) {
    setProgress(String(err), "error");
  }
  setBusy(false);
  await refreshStatuses();
};

document.getElementById("btn-switch-php").onclick = async () => {
  setBusy(true);
  try {
    await invoke("switch_php_version", { version: phpSelect.value });
    setProgress(`PHP → ${phpSelect.value}`, "ok");
  } catch (err) {
    setProgress(String(err), "error");
  }
  setBusy(false);
  await refreshInfo();
  await refreshStatuses();
};

document.getElementById("site-form").onsubmit = async (event) => {
  event.preventDefault();
  const domain = document.getElementById("site-domain").value.trim();
  const projectPath = document.getElementById("site-path").value.trim();
  if (!domain || !projectPath) {
    setProgress("Domain and path required.", "error");
    return;
  }
  setBusy(true);
  try {
    await invoke("create_virtual_host", { domain, projectPath });
    setProgress(`Site ${domain} added.`, "ok");
    event.target.reset();
  } catch (err) {
    setProgress(String(err), "error");
  }
  setBusy(false);
  await refreshInfo();
};

document.getElementById("btn-trust-ca").onclick = async () => {
  try {
    await invoke("trust_local_root_ca");
    setProgress("Root CA trust requested.", "ok");
  } catch (err) {
    setProgress(String(err), "error");
  }
};

document.getElementById("btn-open-term").onclick = async () => {
  const info = await invoke("get_app_info");
  const path =
    document.getElementById("site-path").value.trim() || info.app_root;
  try {
    await invoke("open_project_terminal", { projectPath: path });
    setProgress("Terminal opened.", "ok");
  } catch (err) {
    setProgress(String(err), "error");
  }
};

document.getElementById("btn-refresh-logs").onclick = refreshLogs;

async function install(component) {
  setBusy(true, 0);
  setProgress(`Installing ${component}…`);
  try {
    await invoke("install_manifest_component", {
      component,
      phpVersion: phpSelect.value,
    });
    setBusy(false);
    setProgress(`${component} installed.`, "ok");
    statusFill.style.width = "100%";
    setTimeout(() => {
      statusFill.style.width = "0%";
    }, 600);
  } catch (err) {
    setBusy(false);
    setProgress(String(err), "error");
  }
}

document.getElementById("btn-install-nginx").onclick = () => install("nginx");
document.getElementById("btn-install-php").onclick = () => install("php");
document.getElementById("btn-install-mariadb").onclick = () => install("mariadb");

document.getElementById("btn-verify").onclick = async () => {
  try {
    const report = await invoke("verify_portable");
    setProgress(
      report.ok ? `Portable OK · ${report.app_root}` : `Missing: ${(report.missing_dirs || []).join(", ")}`,
      report.ok ? "ok" : "error"
    );
  } catch (err) {
    setProgress(String(err), "error");
  }
};

listen("download-progress", (event) => {
  const p = event.payload;
  setBusy(true, p.percentage || 0);
  setProgress(`${p.component} · ${(p.percentage || 0).toFixed(0)}%`);
});

listen("services-changed", async () => {
  await refreshStatuses();
});

refreshInfo()
  .then(refreshStatuses)
  .then(refreshLogs)
  .catch((err) => setProgress(String(err), "error"));

setInterval(() => {
  refreshStatuses().catch(() => {});
}, 2500);
