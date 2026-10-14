<script setup>
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { markSetupComplete } from "../router";
import { useAppStore } from "../composables/useAppStore";

const router = useRouter();
const store = useAppStore();

const phpVersions = ref(["8.3"]);
const phpVersion = ref("8.3");
const components = ref([]);
const message = ref("Checking what’s installed…");
const messageKind = ref("");
const running = ref(false);
const progressPct = ref(null);
const busy = ref(false);
const ctaLabel = ref("Install everything");

let unlisten = null;

const fillStyle = computed(() => {
  if (!busy.value) return { width: "0%" };
  if (progressPct.value == null) return {};
  return { width: `${Math.max(0, Math.min(100, progressPct.value))}%` };
});

function stepClass(c) {
  if (c.uiState) return `is-${c.uiState}`;
  return c.installed ? "is-done" : "is-pending";
}

function stepState(c) {
  if (c.uiState === "active") return "installing";
  if (c.uiState === "done" || c.installed) return "ready";
  return "needed";
}

async function loadStatus(version = phpVersion.value) {
  const status = await invoke("get_setup_status", { phpVersion: version });
  phpVersions.value = status.available_php_versions?.length
    ? status.available_php_versions
    : ["8.3"];
  phpVersion.value = status.php_version || version;
  components.value = (status.components || []).map((c) => ({ ...c, uiState: null }));
  const needed = components.value.filter((c) => !c.installed).length;
  messageKind.value = "";
  if (needed === 0) {
    message.value = `PHP ${phpVersion.value} already installed.`;
    messageKind.value = "ok";
    ctaLabel.value = "Continue";
  } else {
    message.value =
      needed === 1
        ? `PHP ${phpVersion.value} still needed.`
        : `${needed} components still needed for PHP ${phpVersion.value}.`;
    ctaLabel.value = "Install everything";
  }
  return status;
}

async function onPhpChange() {
  try {
    await loadStatus(phpVersion.value);
  } catch (err) {
    message.value = String(err);
    messageKind.value = "error";
  }
}

async function enterApp() {
  markSetupComplete();
  await store.bootAppData();
  router.replace({ name: "dashboard" });
}

async function runSetup() {
  const statusPreview = await invoke("get_setup_status", {
    phpVersion: phpVersion.value,
  });
  if (statusPreview.ready) {
    await enterApp();
    return;
  }

  running.value = true;
  busy.value = true;
  progressPct.value = 0;
  message.value = "Preparing download…";
  messageKind.value = "";

  try {
    const status = await invoke("run_first_setup", {
      phpVersion: phpVersion.value,
    });
    components.value = (status.components || []).map((c) => ({
      ...c,
      uiState: c.installed ? "done" : null,
    }));
    if (status.ready) {
      progressPct.value = 100;
      message.value = "Setup complete.";
      messageKind.value = "ok";
      setTimeout(() => enterApp(), 450);
    } else {
      throw new Error("Setup finished but some components are still missing.");
    }
  } catch (err) {
    busy.value = false;
    progressPct.value = null;
    message.value = String(err);
    messageKind.value = "error";
    running.value = false;
    ctaLabel.value = "Retry install";
  }
}

onMounted(async () => {
  document.body.classList.add("is-setup");
  unlisten = await listen("setup-progress", (event) => {
    const p = event.payload;
    if (p.component && (p.phase === "install" || p.phase === "download")) {
      components.value = components.value.map((c) =>
        c.id === p.component ? { ...c, uiState: "active" } : c
      );
    }
    if (p.phase === "done_step" && p.component) {
      components.value = components.value.map((c) =>
        c.id === p.component ? { ...c, installed: true, uiState: "done" } : c
      );
    }
    const overall =
      p.total > 0
        ? ((p.step - 1) / p.total) * 100 + (p.percentage || 0) / p.total
        : p.percentage || 0;
    busy.value = true;
    progressPct.value = overall;
    message.value = p.message || "";
    messageKind.value = p.phase === "complete" ? "ok" : "";
  });

  try {
    const status = await loadStatus();
    if (status.ready) {
      await enterApp();
    }
  } catch (err) {
    message.value = String(err);
    messageKind.value = "error";
  }
});

onUnmounted(() => {
  document.body.classList.remove("is-setup");
  if (unlisten) unlisten();
});
</script>

<template>
  <div class="setup-gate">
    <div class="setup-stage">
      <p class="setup-kicker">First run</p>
      <h1 class="brand setup-brand">praews</h1>
      <p class="setup-lead">
        Install the portable stack once — Nginx, PHP, and MariaDB stay inside this app.
        After that, just Start.
      </p>

      <div class="setup-php">
        <label for="setup-php-version">PHP version</label>
        <select
          id="setup-php-version"
          v-model="phpVersion"
          :disabled="running"
          @change="onPhpChange"
        >
          <option v-for="ver in phpVersions" :key="ver" :value="ver">{{ ver }}</option>
        </select>
      </div>

      <ul class="setup-steps" aria-label="Components">
        <li v-for="c in components" :key="c.id" :class="stepClass(c)">
          <span class="step-mark" aria-hidden="true" />
          <span class="step-label">{{ c.label }}</span>
          <span class="step-state">{{ stepState(c) }}</span>
        </li>
      </ul>

      <div class="setup-progress">
        <div class="status-track">
          <div
            class="status-fill"
            :class="{ indeterminate: busy && progressPct == null }"
            :style="fillStyle"
          />
        </div>
        <p
          id="setup-message"
          role="status"
          aria-live="polite"
          :class="{ 'is-error': messageKind === 'error', 'is-ok': messageKind === 'ok' }"
        >
          {{ message }}
        </p>
      </div>

      <div class="setup-cta">
        <button
          class="btn btn-go"
          type="button"
          :disabled="running"
          @click="runSetup"
        >
          {{ ctaLabel }}
        </button>
      </div>
    </div>
  </div>
</template>
