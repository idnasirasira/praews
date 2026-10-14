<script setup>
import { computed, onMounted, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();

const targets = [
  { id: "Php", label: "PHP" },
  { id: "Nginx", label: "Nginx" },
  { id: "MySql", label: "MariaDB" },
];

const active = ref("Php");
const mode = ref("visual"); // visual | raw
const saving = ref(false);
const extensions = ref([]);
const rawDraft = ref("");
const isWindows = ref(false);

const php = reactive({
  memory_limit: "512M",
  upload_max_filesize: "128M",
  post_max_size: "128M",
  max_execution_time: 300,
  max_input_time: 300,
  display_errors: true,
  enabled_extensions: [],
});

const mysql = reactive({
  innodb_buffer_pool_size: "256M",
  max_allowed_packet: "64M",
  max_connections: 150,
});

const nginx = reactive({
  client_max_body_size: "128M",
  keepalive_timeout: 65,
});

const targetLabel = computed(
  () => targets.find((t) => t.id === active.value)?.label || active.value
);

function syncFromSettings() {
  const s = store.state.info?.settings;
  if (!s) return;
  if (s.php_settings) Object.assign(php, structuredClone(s.php_settings));
  if (s.mysql_settings) Object.assign(mysql, structuredClone(s.mysql_settings));
  if (s.nginx_settings) Object.assign(nginx, structuredClone(s.nginx_settings));
}

async function loadExtensions() {
  try {
    const list = await invoke("get_php_extensions_list");
    extensions.value = Array.isArray(list) ? list : [];
    const enabled = new Set(php.enabled_extensions || []);
    for (const ext of extensions.value) {
      if (enabled.has(ext.name)) ext.enabled = true;
    }
  } catch (err) {
    extensions.value = [];
    store.setProgress(String(err), "error");
  }
}

async function loadRaw() {
  try {
    rawDraft.value = await invoke("get_custom_raw_config", { target: active.value });
  } catch (err) {
    rawDraft.value = "";
    store.setProgress(String(err), "error");
  }
}

function toggleExt(name, checked) {
  const set = new Set(php.enabled_extensions || []);
  if (checked) set.add(name);
  else set.delete(name);
  php.enabled_extensions = [...set].sort();
  const row = extensions.value.find((e) => e.name === name);
  if (row) row.enabled = checked;
}

async function saveVisual() {
  saving.value = true;
  store.setBusy(true);
  store.setProgress(`Saving ${targetLabel.value} visual settings…`);
  try {
    const payload = {
      target: active.value,
      phpSettings: null,
      mysqlSettings: null,
      nginxSettings: null,
    };
    if (active.value === "Php") {
      payload.phpSettings = {
        ...php,
        enabled_extensions: [...(php.enabled_extensions || [])],
      };
    } else if (active.value === "MySql") {
      payload.mysqlSettings = { ...mysql };
    } else {
      payload.nginxSettings = { ...nginx };
    }
    await invoke("save_visual_config", payload);
    await store.refreshInfo();
    await store.refreshStatuses();
    syncFromSettings();
    store.setProgress(`${targetLabel.value} visual settings applied.`, "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  saving.value = false;
}

async function saveRaw() {
  saving.value = true;
  store.setBusy(true);
  store.setProgress(`Saving ${targetLabel.value} custom directives…`);
  try {
    await invoke("save_custom_raw_config", {
      target: active.value,
      rawContent: rawDraft.value,
    });
    await store.refreshStatuses();
    store.setProgress(`${targetLabel.value} custom config applied.`, "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  saving.value = false;
}

async function resetConfig() {
  if (
    !confirm(
      `Reset ${targetLabel.value} to factory defaults?\nVisual settings and custom raw will be cleared.`
    )
  ) {
    return;
  }
  saving.value = true;
  store.setBusy(true);
  store.setProgress(`Resetting ${targetLabel.value}…`);
  try {
    await invoke("reset_service_config", { target: active.value });
    await store.refreshInfo();
    await store.refreshStatuses();
    syncFromSettings();
    await loadRaw();
    if (active.value === "Php") await loadExtensions();
    store.setProgress(`${targetLabel.value} reset to defaults.`, "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  saving.value = false;
}

async function openRendered() {
  try {
    await invoke("open_rendered_config_in_os", { target: active.value });
  } catch (err) {
    store.setProgress(String(err), "error");
  }
}

watch(active, async () => {
  mode.value = "visual";
  await loadRaw();
  if (active.value === "Php") await loadExtensions();
});

onMounted(async () => {
  if (!store.state.booted) await store.bootAppData();
  const platId = store.state.info?.php_platform?.platform_id || "";
  isWindows.value =
    platId.startsWith("windows") || /Win/i.test(navigator.userAgent);
  syncFromSettings();
  await loadRaw();
  await loadExtensions();
});
</script>

<template>
  <section class="pane view-pane" aria-labelledby="config-title">
    <div class="pane-head">
      <div>
        <h2 id="config-title">Config</h2>
        <p class="view-lede">
          Tune PHP, Nginx, and MariaDB without losing edits on restart. Visual controls write to
          <code>state.json</code>; raw lines live in <code>conf/custom/</code> and survive template
          re-renders.
        </p>
      </div>
    </div>

    <div class="config-tabs" role="tablist" aria-label="Service">
      <button
        v-for="t in targets"
        :key="t.id"
        type="button"
        class="btn btn-chip"
        role="tab"
        :aria-selected="active === t.id"
        :class="{ active: active === t.id }"
        @click="active = t.id"
      >
        {{ t.label }}
      </button>
    </div>

    <div class="config-modes">
      <button
        type="button"
        class="btn btn-ghost"
        :class="{ active: mode === 'visual' }"
        @click="mode = 'visual'"
      >
        Visual
      </button>
      <button
        type="button"
        class="btn btn-ghost"
        :class="{ active: mode === 'raw' }"
        @click="mode = 'raw'"
      >
        Raw overrides
      </button>
    </div>

    <div v-if="mode === 'visual'" class="config-panel">
      <template v-if="active === 'Php'">
        <div class="config-grid">
          <label>
            Memory limit
            <input v-model="php.memory_limit" type="text" spellcheck="false" />
          </label>
          <label>
            Upload max filesize
            <input v-model="php.upload_max_filesize" type="text" spellcheck="false" />
          </label>
          <label>
            Post max size
            <input v-model="php.post_max_size" type="text" spellcheck="false" />
          </label>
          <label>
            Max execution time
            <input v-model.number="php.max_execution_time" type="number" min="0" />
          </label>
          <label>
            Max input time
            <input v-model.number="php.max_input_time" type="number" min="0" />
          </label>
          <label class="check-row">
            <input v-model="php.display_errors" type="checkbox" />
            Display errors
          </label>
        </div>

        <div class="ext-block">
          <h3>Extensions</h3>
          <p v-if="!isWindows" class="hint">
            On macOS/Linux static PHP builds, extensions are compiled in — checkboxes only apply on
            Windows DLL builds.
          </p>
          <p v-else-if="!extensions.length" class="hint">
            No <code>ext/</code> folder found for the active PHP version yet.
          </p>
          <ul v-else class="ext-grid">
            <li v-for="ext in extensions" :key="ext.name">
              <label>
                <input
                  type="checkbox"
                  :checked="ext.enabled"
                  @change="toggleExt(ext.name, $event.target.checked)"
                />
                {{ ext.name }}
              </label>
            </li>
          </ul>
        </div>
      </template>

      <template v-else-if="active === 'MySql'">
        <div class="config-grid">
          <label>
            InnoDB buffer pool
            <input v-model="mysql.innodb_buffer_pool_size" type="text" spellcheck="false" />
          </label>
          <label>
            Max allowed packet
            <input v-model="mysql.max_allowed_packet" type="text" spellcheck="false" />
          </label>
          <label>
            Max connections
            <input v-model.number="mysql.max_connections" type="number" min="1" />
          </label>
        </div>
      </template>

      <template v-else>
        <div class="config-grid">
          <label>
            Client max body size
            <input v-model="nginx.client_max_body_size" type="text" spellcheck="false" />
          </label>
          <label>
            Keepalive timeout
            <input v-model.number="nginx.keepalive_timeout" type="number" min="1" />
          </label>
        </div>
      </template>

      <div class="config-actions">
        <button class="btn btn-go" type="button" :disabled="saving" @click="saveVisual">
          Apply visual
        </button>
        <button class="btn btn-ghost" type="button" :disabled="saving" @click="openRendered">
          Open rendered file
        </button>
        <button class="btn btn-halt" type="button" :disabled="saving" @click="resetConfig">
          Reset defaults
        </button>
      </div>
    </div>

    <div v-else class="config-panel">
      <p class="hint">
        Appended at the bottom of the rendered config. Nginx syntax is checked with
        <code>nginx -t</code> and rolled back on failure.
      </p>
      <textarea
        v-model="rawDraft"
        class="raw-editor"
        spellcheck="false"
        :aria-label="`${targetLabel} custom directives`"
        rows="16"
      />
      <div class="config-actions">
        <button class="btn btn-go" type="button" :disabled="saving" @click="saveRaw">
          Apply raw
        </button>
        <button class="btn btn-ghost" type="button" :disabled="saving" @click="openRendered">
          Open rendered file
        </button>
        <button class="btn btn-halt" type="button" :disabled="saving" @click="resetConfig">
          Reset defaults
        </button>
      </div>
    </div>
  </section>
</template>
