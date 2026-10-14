import { computed, reactive, readonly } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const state = reactive({
  info: null,
  statuses: {},
  logs: "(no logs yet)",
  logSource: "all",
  logSources: [
    { id: "all", label: "All" },
    { id: "praews", label: "praews" },
    { id: "nginx", label: "Nginx" },
    { id: "php", label: "PHP" },
    { id: "mysql", label: "MariaDB" },
  ],
  progress: { text: "Ready.", kind: "" },
  busy: false,
  busyPct: null,
  phpVersions: ["8.3"],
  phpVersion: "8.3",
  booted: false,
});

let listenersBound = false;
let pollTimer = null;

async function refreshInfo() {
  const info = await invoke("get_app_info");
  state.info = info;
  const installed = info.installed_php_versions?.length
    ? info.installed_php_versions
    : info.available_php_versions?.length
      ? info.available_php_versions
      : ["8.3"];
  state.phpVersions = installed;
  const active = info.settings?.active_php_version;
  state.phpVersion =
    active && installed.includes(active) ? active : installed[0] || "8.3";
  return info;
}

async function refreshStatuses() {
  state.statuses = await invoke("get_service_statuses");
  return state.statuses;
}

async function refreshLogs(source = state.logSource) {
  const result = await invoke("read_recent_logs", {
    lines: 200,
    source,
  });
  if (typeof result === "string") {
    state.logs = result;
  } else {
    state.logSource = result.source || source || "all";
    state.logSources = result.sources?.length ? result.sources : state.logSources;
    state.logs = result.content || "(no log output yet)";
  }
  return state.logs;
}

async function clearLogs() {
  await invoke("clear_logs");
  state.logs = "(logs cleared)";
  setProgress("Logs cleared.", "ok");
  await refreshLogs(state.logSource);
}

function setProgress(text, kind = "") {
  state.progress = { text, kind };
}

function setBusy(busy, pct = null) {
  state.busy = busy;
  state.busyPct = busy ? pct : null;
}

async function bootAppData() {
  await refreshInfo();
  await refreshStatuses();
  await refreshLogs();
  state.booted = true;
  setProgress("Stack installed. Hit Start when you’re ready.", "ok");
}

async function bindListeners() {
  if (listenersBound) return;
  listenersBound = true;

  await listen("download-progress", (event) => {
    const p = event.payload;
    setBusy(true, p.percentage ?? 0);
    setProgress(`${p.component} · ${(p.percentage || 0).toFixed(0)}%`);
  });

  await listen("services-changed", async () => {
    await refreshStatuses();
  });

  pollTimer = setInterval(() => {
    if (state.booted) refreshStatuses().catch(() => {});
  }, 2500);
}

export function useAppStore() {
  const sites = computed(() => state.info?.settings?.vhosts || []);
  const lanIp = computed(() => state.info?.lan_ip || "—");
  const appRoot = computed(() => state.info?.app_root || "—");
  const dnsHint = computed(() => state.info?.dns_hint || "—");
  const mobileUrl = computed(() => state.info?.mobile_url || "#");
  const rootCaUrl = computed(() => state.info?.root_ca_url || "#");

  return {
    state: readonly(state),
    sites,
    lanIp,
    appRoot,
    dnsHint,
    mobileUrl,
    rootCaUrl,
    refreshInfo,
    refreshStatuses,
    refreshLogs,
    clearLogs,
    setProgress,
    setBusy,
    bootAppData,
    bindListeners,
    async startAll() {
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
    },
    async stopAll() {
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
    },
    async switchPhp(version) {
      setBusy(true);
      setProgress(`Switching to PHP ${version}…`);
      try {
        await invoke("switch_php_version", { version });
        state.phpVersion = version;
        setProgress(`PHP → ${version}`, "ok");
      } catch (err) {
        setProgress(String(err), "error");
      }
      setBusy(false);
      await refreshInfo();
      await refreshStatuses();
    },
    async installPhp(version) {
      setBusy(true, 0);
      setProgress(`Downloading PHP ${version}…`);
      try {
        await invoke("install_manifest_component", {
          component: "php",
          phpVersion: version,
        });
        setBusy(false);
        setProgress(`PHP ${version} installed.`, "ok");
      } catch (err) {
        setBusy(false);
        setProgress(String(err), "error");
      }
      await refreshInfo();
    },
    async addSite(domain, projectPath) {
      setBusy(true);
      try {
        await invoke("create_virtual_host", { domain, projectPath });
        setProgress(`Site ${domain} added.`, "ok");
      } catch (err) {
        setProgress(String(err), "error");
        setBusy(false);
        throw err;
      }
      setBusy(false);
      await refreshInfo();
    },
    async trustCa() {
      try {
        await invoke("trust_local_root_ca");
        setProgress("Root CA trust requested.", "ok");
      } catch (err) {
        setProgress(String(err), "error");
      }
    },
    async openTerminal(projectPath) {
      const info = state.info || (await refreshInfo());
      const path = projectPath || info.app_root;
      try {
        await invoke("open_project_terminal", { projectPath: path });
        setProgress("Terminal opened.", "ok");
      } catch (err) {
        setProgress(String(err), "error");
      }
    },
    async install(component) {
      setBusy(true, 0);
      setProgress(`Installing ${component}…`);
      try {
        await invoke("install_manifest_component", {
          component,
          phpVersion: state.phpVersion,
        });
        setBusy(false);
        setProgress(`${component} installed.`, "ok");
      } catch (err) {
        setBusy(false);
        setProgress(String(err), "error");
      }
    },
    async verify() {
      try {
        const report = await invoke("verify_portable");
        setProgress(
          report.ok
            ? `Portable OK · ${report.app_root}`
            : `Missing: ${(report.missing_dirs || []).join(", ")}`,
          report.ok ? "ok" : "error"
        );
      } catch (err) {
        setProgress(String(err), "error");
      }
    },
  };
}
