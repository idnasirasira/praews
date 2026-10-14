<script setup>
import { computed, onMounted, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();

const draft = reactive({
  http_port: 8888,
  https_port: 8443,
  mysql_port: 3306,
  php_fastcgi_port: 9000,
  dns_port: 53535,
});

const checks = ref({});
const saving = ref(false);
const applying = ref("");
let debounceTimer = null;

const fields = [
  { key: "http_port", label: "HTTP (Nginx)" },
  { key: "https_port", label: "HTTPS" },
  { key: "mysql_port", label: "MySQL / MariaDB" },
  { key: "php_fastcgi_port", label: "PHP FastCGI" },
  { key: "dns_port", label: "DNS" },
];

const duplicatePorts = computed(() => {
  const vals = [
    draft.http_port,
    draft.https_port,
    draft.mysql_port,
    draft.php_fastcgi_port,
  ];
  const seen = new Set();
  const dups = new Set();
  for (const v of vals) {
    if (seen.has(v)) dups.add(v);
    seen.add(v);
  }
  return dups;
});

const canSave = computed(() => {
  if (saving.value || applying.value) return false;
  if (duplicatePorts.value.size > 0) return false;
  return fields.every((f) => {
    const c = checks.value[draft[f.key]];
    return c?.available !== false;
  });
});

function syncFromSettings() {
  const s = store.state.info?.settings;
  if (!s) return;
  draft.http_port = s.http_port;
  draft.https_port = s.https_port;
  draft.mysql_port = s.mysql_port;
  draft.php_fastcgi_port = s.php_fastcgi_port;
  draft.dns_port = s.dns_port;
}

async function runChecks() {
  const ports = fields.map((f) => Number(draft[f.key])).filter((n) => Number.isFinite(n) && n > 0);
  try {
    const results = await invoke("check_ports_availability", { ports });
    const map = {};
    for (const r of results) map[r.port] = r;
    checks.value = map;
  } catch (err) {
    store.setProgress(String(err), "error");
  }
}

function scheduleCheck() {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(runChecks, 300);
}

function useSuggested(key) {
  const port = Number(draft[key]);
  const alt = checks.value[port]?.suggested_alternative;
  if (alt) {
    draft[key] = alt;
    scheduleCheck();
  }
}

async function savePorts() {
  saving.value = true;
  store.setBusy(true);
  store.setProgress("Updating ports…");
  try {
    await invoke("update_ports", {
      newPorts: {
        http_port: Number(draft.http_port),
        https_port: Number(draft.https_port),
        mysql_port: Number(draft.mysql_port),
        php_fastcgi_port: Number(draft.php_fastcgi_port),
        dns_port: Number(draft.dns_port),
      },
    });
    await store.refreshInfo();
    await store.refreshStatuses();
    syncFromSettings();
    await runChecks();
    store.setProgress("Ports saved. Configs and vhosts re-rendered.", "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  saving.value = false;
}

async function applyPreset(preset) {
  applying.value = preset;
  store.setBusy(true);
  store.setProgress(`Applying ${preset} ports…`);
  try {
    await invoke("apply_port_preset", { preset });
    await store.refreshInfo();
    await store.refreshStatuses();
    syncFromSettings();
    await runChecks();
    store.setProgress(`Preset “${preset}” applied.`, "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  applying.value = "";
}

function statusFor(key) {
  const port = Number(draft[key]);
  if (duplicatePorts.value.has(port) && ["http_port", "https_port", "mysql_port", "php_fastcgi_port"].includes(key)) {
    return { kind: "dup", text: "Duplicate among services" };
  }
  const c = checks.value[port];
  if (!c) return { kind: "pending", text: "Checking…" };
  if (c.available) return { kind: "ok", text: "Available" };
  return {
    kind: "bad",
    text: "In use",
    alt: c.suggested_alternative,
  };
}

onMounted(async () => {
  if (!store.state.booted) await store.bootAppData();
  syncFromSettings();
  await runChecks();
});

watch(
  () => ({ ...draft }),
  () => scheduleCheck(),
  { deep: true }
);
</script>

<template>
  <section class="pane view-pane" aria-labelledby="ports-title">
    <div class="pane-head">
      <div>
        <h2 id="ports-title">Ports</h2>
        <p class="view-lede">
          Change HTTP / HTTPS / MySQL ports without touching system defaults. Saving rewrites
          nginx, my.cnf, and every virtual host, then restarts if the stack was running.
        </p>
      </div>
    </div>

    <div class="preset-row">
      <button
        class="btn btn-chip"
        type="button"
        :disabled="!!applying"
        @click="applyPreset('standard')"
      >
        Web default 80 / 443 / 3306
      </button>
      <button
        class="btn btn-chip"
        type="button"
        :disabled="!!applying"
        @click="applyPreset('high_ports')"
      >
        High ports 8888 / 8890 / 8889
      </button>
      <button
        class="btn btn-chip"
        type="button"
        :disabled="!!applying"
        @click="applyPreset('auto_free')"
      >
        Auto-detect free ports
      </button>
    </div>

    <ul class="port-grid">
      <li v-for="field in fields" :key="field.key">
        <label :for="field.key">{{ field.label }}</label>
        <div class="port-row">
          <input
            :id="field.key"
            v-model.number="draft[field.key]"
            type="number"
            min="1"
            max="65535"
          />
          <span
            class="port-badge"
            :class="{
              ok: statusFor(field.key).kind === 'ok',
              bad: statusFor(field.key).kind === 'bad' || statusFor(field.key).kind === 'dup',
            }"
          >
            {{ statusFor(field.key).text }}
          </span>
          <button
            v-if="statusFor(field.key).alt"
            class="btn btn-ghost"
            type="button"
            @click="useSuggested(field.key)"
          >
            Use {{ statusFor(field.key).alt }}
          </button>
        </div>
      </li>
    </ul>

    <div class="ports-actions">
      <button class="btn btn-go" type="button" :disabled="!canSave" @click="savePorts">
        Save ports
      </button>
    </div>
  </section>
</template>
