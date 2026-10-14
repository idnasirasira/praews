<script setup>
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const projectPath = ref("");
const newRoot = ref("");
const mode = ref("point");
const busyRoot = ref(false);

const htdocsEnabled = ref(false);
const htdocsPath = ref("");
const busyHtdocs = ref(false);

const currentRoot = computed(() => store.appRoot);
const usingCustom = computed(() => !!store.state.info?.using_custom_root);
const defaultRoot = computed(
  () => store.state.info?.default_app_root || "—"
);
const activeHtdocs = computed(
  () => store.state.info?.htdocs_root || `${store.appRoot}/htdocs`
);

watch(
  () => store.state.info?.settings,
  (s) => {
    if (!s) return;
    htdocsEnabled.value = !!s.custom_htdocs_enabled;
    htdocsPath.value = s.custom_htdocs_path || "";
  },
  { immediate: true }
);

const modeHelp = computed(() => {
  switch (mode.value) {
    case "copy":
      return "Copy binaries, configs, and data into the new folder, then switch.";
    case "move":
      return "Move the whole sandbox to the new folder (old folder is emptied).";
    default:
      return "Use that folder as-is. Empty folders get a fresh sandbox layout.";
  }
});

async function applyRoot() {
  const path = newRoot.value.trim();
  if (!path) {
    store.setProgress("Enter a folder path for the sandbox root.", "error");
    return;
  }
  busyRoot.value = true;
  store.setBusy(true);
  store.setProgress("Changing sandbox root…");
  try {
    const result = await invoke("change_app_root", {
      newRoot: path,
      mode: mode.value,
    });
    store.setProgress(result.message || "Restarting…", "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
    store.setBusy(false);
    busyRoot.value = false;
  }
}

async function resetRoot() {
  busyRoot.value = true;
  store.setBusy(true);
  store.setProgress("Resetting to default sandbox…");
  try {
    const result = await invoke("reset_app_root");
    store.setProgress(result.message || "Restarting…", "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
    store.setBusy(false);
    busyRoot.value = false;
  }
}

async function saveHtdocs() {
  busyHtdocs.value = true;
  store.setBusy(true);
  store.setProgress("Updating htdocs…");
  try {
    await invoke("update_htdocs_root", {
      enabled: htdocsEnabled.value,
      path: htdocsPath.value.trim() || null,
    });
    await store.refreshInfo();
    store.setProgress(
      htdocsEnabled.value
        ? `Custom htdocs enabled → ${htdocsPath.value.trim()}`
        : "Using default sandbox/htdocs.",
      "ok"
    );
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyHtdocs.value = false;
}
</script>

<template>
  <section class="pane view-pane" aria-labelledby="tools-title">
    <div class="pane-head">
      <div>
        <h2 id="tools-title">Tools</h2>
        <p class="view-lede">Reinstall binaries, verify the sandbox, open a scoped terminal.</p>
      </div>
    </div>

    <div class="setup-actions tools-grid">
      <button class="btn btn-chip" type="button" @click="store.install('nginx')">
        Reinstall Nginx
      </button>
      <button class="btn btn-chip" type="button" @click="store.install('php')">
        Reinstall PHP
      </button>
      <button class="btn btn-chip" type="button" @click="store.install('mariadb')">
        Reinstall MariaDB
      </button>
      <button class="btn btn-chip" type="button" @click="store.verify()">Verify</button>
      <button class="btn btn-chip" type="button" @click="store.trustCa()">Trust CA</button>
      <button
        class="btn btn-chip"
        type="button"
        @click="store.openTerminal(projectPath.trim())"
      >
        Terminal
      </button>
    </div>

    <form class="site-form tools-path" autocomplete="off" @submit.prevent>
      <input
        v-model="projectPath"
        placeholder="Optional project path for terminal"
      />
    </form>
  </section>

  <section class="pane view-pane root-pane" aria-labelledby="htdocs-title">
    <div class="pane-head">
      <div>
        <h2 id="htdocs-title">Document root (htdocs)</h2>
        <p class="view-lede">
          Default <code>localhost</code> site only — does not move Nginx/PHP/MariaDB.
          Virtual hosts under Sites stay on their own project paths.
        </p>
      </div>
    </div>

    <p class="root-current">
      <span class="root-label">Active htdocs</span>
      <code class="root-path">{{ activeHtdocs }}</code>
    </p>

    <label class="mode-opt htdocs-toggle">
      <input v-model="htdocsEnabled" type="checkbox" />
      <span>Use a custom htdocs folder</span>
    </label>

    <form class="site-form root-form" autocomplete="off" @submit.prevent="saveHtdocs">
      <input
        v-model="htdocsPath"
        name="htdocs"
        placeholder="/Users/you/Sites/my-project/public"
        :disabled="!htdocsEnabled"
      />
      <button class="btn btn-go" type="submit" :disabled="busyHtdocs">Save</button>
    </form>
  </section>

  <section class="pane view-pane root-pane" aria-labelledby="root-title">
    <div class="pane-head">
      <div>
        <h2 id="root-title">Sandbox root</h2>
        <p class="view-lede">
          Advanced: relocate the <em>entire</em> portable stack (bin/conf/data/logs/htdocs).
          App restarts after Apply.
        </p>
      </div>
    </div>

    <p class="root-current">
      <span class="root-label">Current</span>
      <code class="root-path">{{ currentRoot }}</code>
    </p>
    <p v-if="usingCustom" class="root-meta">
      Custom root active · default would be <code>{{ defaultRoot }}</code>
    </p>
    <p v-else class="root-meta">Using the default sandbox location.</p>

    <div class="root-mode">
      <label class="mode-opt">
        <input v-model="mode" type="radio" value="point" />
        <span>Use folder</span>
      </label>
      <label class="mode-opt">
        <input v-model="mode" type="radio" value="copy" />
        <span>Copy here</span>
      </label>
      <label class="mode-opt">
        <input v-model="mode" type="radio" value="move" />
        <span>Move here</span>
      </label>
    </div>
    <p class="root-help">{{ modeHelp }}</p>

    <form class="site-form root-form" autocomplete="off" @submit.prevent="applyRoot">
      <input
        v-model="newRoot"
        name="root"
        placeholder="/Users/you/Sites/praews-sandbox"
        required
      />
      <button class="btn btn-go" type="submit" :disabled="busyRoot">Apply</button>
    </form>

    <div v-if="usingCustom" class="root-reset">
      <button class="btn btn-ghost" type="button" :disabled="busyRoot" @click="resetRoot">
        Reset to default
      </button>
    </div>
  </section>
</template>
