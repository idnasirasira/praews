<script setup>
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const catalog = ref([]);
const platform = ref(null);
const busyAction = ref("");
const showGuide = ref(true);

const newVersion = ref("");
const linkingId = ref(null);
const linkPath = ref("");

watch(
  () => store.state.info,
  (info) => {
    if (Array.isArray(info?.php_catalog)) catalog.value = info.php_catalog;
    if (info?.php_platform) platform.value = info.php_platform;
  },
  { immediate: true }
);

const downloadable = computed(() =>
  catalog.value.filter((e) => e.downloadable || e.source === "available")
);
const customOnes = computed(() =>
  catalog.value.filter((e) => e.source === "user" || (!e.downloadable && e.source !== "available"))
);

const downloadList = computed(() => {
  if (!platform.value?.downloadable?.length) return "—";
  return platform.value.downloadable
    .map((id, i) => {
      const patch = platform.value.downloadable_versions?.[i];
      return patch ? `${id} (${patch})` : id;
    })
    .join(" · ");
});

const linkExamples = computed(() =>
  (platform.value?.linkable_examples || []).join(", ")
);

const pathPlaceholder = computed(
  () => platform.value?.brew_path_example || "/opt/homebrew/opt/php@7.4"
);

function statusLabel(entry) {
  if (entry.active) return "Active";
  if (entry.custom_path) return "Linked";
  if (entry.installed) return "Installed";
  if (entry.source === "user") return "Needs folder";
  return "Not installed";
}

async function refreshCatalog() {
  try {
    const result = await invoke("list_php_catalog");
    if (Array.isArray(result)) {
      catalog.value = result;
    } else {
      catalog.value = result.catalog || [];
      if (result.platform) platform.value = result.platform;
    }
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  await store.refreshInfo();
}

async function addVersion() {
  const label = newVersion.value.trim();
  if (!label) {
    store.setProgress("Enter a version like 7.4.", "error");
    return;
  }
  busyAction.value = "add";
  store.setBusy(true);
  try {
    const id = await invoke("add_php_version", { versionLabel: label });
    newVersion.value = "";
    store.setProgress(`PHP ${id} added — link a folder next.`, "ok");
    await refreshCatalog();
    openLink({ id, detected_homebrew: null, custom_path: null });
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyAction.value = "";
}

function openLink(entry) {
  linkingId.value = entry.id;
  linkPath.value = entry.custom_path || entry.detected_homebrew || "";
}

function cancelLink() {
  linkingId.value = null;
  linkPath.value = "";
}

async function submitLink() {
  const id = linkingId.value;
  const path = linkPath.value.trim();
  if (!id || !path) {
    store.setProgress("Paste the PHP folder path.", "error");
    return;
  }
  busyAction.value = `link:${id}`;
  store.setBusy(true);
  store.setProgress(`Linking PHP ${id}…`);
  try {
    const verLine = await invoke("register_custom_php_path", {
      versionLabel: id,
      customDirPath: path,
    });
    store.setProgress(`Linked PHP ${id} · ${verLine}`, "ok");
    cancelLink();
    await refreshCatalog();
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyAction.value = "";
}

async function useDetected(entry) {
  if (!entry.detected_homebrew) return;
  linkingId.value = entry.id;
  linkPath.value = entry.detected_homebrew;
  await submitLink();
}

async function downloadPhp(id) {
  busyAction.value = `php:${id}`;
  store.setBusy(true, 0);
  store.setProgress(`Downloading PHP ${id}…`);
  try {
    await invoke("install_manifest_component", {
      component: "php",
      phpVersion: id,
    });
    store.setProgress(`PHP ${id} installed.`, "ok");
    await refreshCatalog();
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyAction.value = "";
}

async function usePhp(id) {
  busyAction.value = `use:${id}`;
  await store.switchPhp(id);
  await refreshCatalog();
  busyAction.value = "";
}

async function removePhp(id) {
  busyAction.value = `rm:${id}`;
  store.setBusy(true);
  try {
    await invoke("remove_php_version", { version: id });
    store.setProgress(`PHP ${id} removed.`, "ok");
    await refreshCatalog();
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyAction.value = "";
}

async function unlinkPhp(id) {
  busyAction.value = `unlink:${id}`;
  store.setBusy(true);
  try {
    await invoke("clear_custom_php_path", { versionLabel: id });
    store.setProgress(`Unlinked PHP ${id}.`, "ok");
    await refreshCatalog();
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  store.setBusy(false);
  busyAction.value = "";
}

onMounted(() => {
  refreshCatalog().catch(() => {});
});
</script>

<template>
  <section class="pane view-pane" aria-labelledby="php-title">
    <div class="pane-head">
      <div>
        <h2 id="php-title">PHP</h2>
        <p class="view-lede">
          Download portable builds, or add any version yourself and link a local folder.
        </p>
      </div>
      <button class="btn btn-ghost" type="button" @click="showGuide = !showGuide">
        {{ showGuide ? "Hide guide" : "Show guide" }}
      </button>
    </div>

    <div v-if="showGuide && platform" class="php-guide" aria-label="PHP availability for this OS">
      <h3 class="guide-title">On {{ platform.platform_label }}</h3>
      <p class="guide-note">{{ platform.link_hint }}</p>
      <ul class="guide-avail">
        <li>
          <span class="guide-k">Download</span>
          <span>{{ downloadList }}</span>
        </li>
        <li>
          <span class="guide-k">Add + link</span>
          <span>{{ linkExamples || "any version you have locally" }}</span>
        </li>
      </ul>
      <ol v-if="platform.brew_formula_example" class="guide-steps">
        <li>
          Install with Homebrew (example for 7.4):
          <pre class="guide-code">{{ platform.brew_formula_example }}</pre>
        </li>
        <li>Type <code>7.4</code> below → <strong>Add version</strong>.</li>
        <li>
          <strong>Link folder…</strong> / <strong>Use Homebrew</strong> to:
          <pre class="guide-code">{{ pathPlaceholder }}</pre>
        </li>
        <li>Hit <strong>Use</strong>, then Start the stack.</li>
      </ol>
      <ol v-else class="guide-steps">
        <li>Type a label like <code>7.2</code> → <strong>Add version</strong>.</li>
        <li><strong>Link folder…</strong> to a local PHP directory that has CLI + FPM/CGI.</li>
        <li>Hit <strong>Use</strong>, then Start.</li>
      </ol>
    </div>

    <form class="add-php-form" autocomplete="off" @submit.prevent="addVersion">
      <label class="add-php-label" for="php-new-version">Add your own version</label>
      <div class="input-group add-php-row">
        <input
          id="php-new-version"
          v-model="newVersion"
          placeholder="e.g. 7.4"
          required
        />
        <button class="btn btn-go" type="submit" :disabled="!!busyAction">Add version</button>
      </div>
    </form>
  </section>

  <section
    v-if="customOnes.length"
    class="pane view-pane root-pane"
    aria-labelledby="php-custom-title"
  >
    <div class="pane-head">
      <div>
        <h2 id="php-custom-title">Your versions</h2>
        <p class="view-lede">Linked or waiting for a folder.</p>
      </div>
    </div>

    <ul class="catalog-list" aria-label="Your PHP versions">
      <li
        v-for="entry in customOnes"
        :key="'u-' + entry.id"
        class="catalog-row"
        :class="{ 'is-active': entry.active, 'is-installed': entry.installed }"
      >
        <div class="catalog-main">
          <span class="catalog-id">PHP {{ entry.id }}</span>
          <span class="catalog-badge">{{ statusLabel(entry) }}</span>
        </div>
        <p v-if="entry.custom_path" class="catalog-note catalog-path">
          Linked → <code>{{ entry.custom_path }}</code>
        </p>

        <div v-if="linkingId === entry.id" class="import-row">
          <div class="input-group">
            <input
              v-model="linkPath"
              type="text"
              :placeholder="entry.detected_homebrew || pathPlaceholder.replace('7.4', entry.id)"
              @keydown.enter.prevent="submitLink"
            />
            <button class="btn btn-go" type="button" :disabled="!!busyAction" @click="submitLink">
              Link
            </button>
          </div>
          <button class="btn btn-ghost" type="button" @click="cancelLink">Cancel</button>
        </div>
        <div v-else class="catalog-actions">
          <button
            v-if="!entry.installed && entry.detected_homebrew"
            class="btn btn-go"
            type="button"
            :disabled="!!busyAction"
            @click="useDetected(entry)"
          >
            Use Homebrew
          </button>
          <button
            v-if="!entry.custom_path"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="openLink(entry)"
          >
            Link folder…
          </button>
          <button
            v-if="entry.custom_path && !entry.active"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="openLink(entry)"
          >
            Relink…
          </button>
          <button
            v-if="entry.installed && !entry.active"
            class="btn btn-ghost"
            type="button"
            :disabled="!!busyAction"
            @click="usePhp(entry.id)"
          >
            Use
          </button>
          <button
            v-if="entry.custom_path && !entry.active"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="unlinkPhp(entry.id)"
          >
            Unlink
          </button>
          <button
            v-if="!entry.active"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="removePhp(entry.id)"
          >
            Remove
          </button>
          <span v-if="entry.active" class="catalog-hint">In use</span>
        </div>
      </li>
    </ul>
  </section>

  <section class="pane view-pane root-pane" aria-labelledby="php-dl-title">
    <div class="pane-head">
      <div>
        <h2 id="php-dl-title">Downloadable</h2>
        <p class="view-lede">
          Slim set for {{ platform?.platform_label || "this OS" }} — more versions via Add + link.
        </p>
      </div>
    </div>

    <ul class="catalog-list" aria-label="Downloadable PHP">
      <li
        v-for="entry in downloadable"
        :key="'d-' + entry.id"
        class="catalog-row"
        :class="{ 'is-active': entry.active, 'is-installed': entry.installed }"
      >
        <div class="catalog-main">
          <span class="catalog-id">PHP {{ entry.id }}</span>
          <span class="catalog-ver">{{ entry.version }}</span>
          <span class="catalog-badge">{{ statusLabel(entry) }}</span>
        </div>

        <div v-if="linkingId === entry.id" class="import-row">
          <div class="input-group">
            <input
              v-model="linkPath"
              type="text"
              :placeholder="'/opt/homebrew/opt/php@' + entry.id"
              @keydown.enter.prevent="submitLink"
            />
            <button class="btn btn-go" type="button" :disabled="!!busyAction" @click="submitLink">
              Link
            </button>
          </div>
          <button class="btn btn-ghost" type="button" @click="cancelLink">Cancel</button>
        </div>
        <div v-else class="catalog-actions">
          <button
            v-if="!entry.installed && entry.downloadable"
            class="btn btn-go"
            type="button"
            :disabled="!!busyAction"
            @click="downloadPhp(entry.id)"
          >
            {{ busyAction === `php:${entry.id}` ? "Downloading…" : "Download" }}
          </button>
          <button
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="openLink(entry)"
          >
            Link folder…
          </button>
          <button
            v-if="entry.installed && !entry.active"
            class="btn btn-ghost"
            type="button"
            :disabled="!!busyAction"
            @click="usePhp(entry.id)"
          >
            Use
          </button>
          <button
            v-if="entry.installed && !entry.active && !entry.custom_path"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="removePhp(entry.id)"
          >
            Remove
          </button>
          <span v-if="entry.active" class="catalog-hint">In use</span>
        </div>
      </li>
    </ul>
  </section>
</template>
