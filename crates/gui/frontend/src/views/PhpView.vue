<script setup>
import { onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const catalog = ref([]);
const busyAction = ref("");

watch(
  () => store.state.info?.php_catalog,
  (rows) => {
    if (Array.isArray(rows)) catalog.value = rows;
  },
  { immediate: true }
);

function statusLabel(entry) {
  if (entry.active) return "Active";
  if (entry.installed) return "Installed";
  return "Not installed";
}

async function refreshCatalog() {
  try {
    catalog.value = await invoke("list_php_catalog");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  await store.refreshInfo();
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
  store.setProgress(`Removing PHP ${id}…`);
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
          Download any catalog release (including older ones), activate it, or free disk space.
        </p>
      </div>
    </div>

    <ul class="catalog-list" aria-label="PHP catalog">
      <li
        v-for="entry in catalog"
        :key="entry.id"
        class="catalog-row"
        :class="{ 'is-active': entry.active, 'is-installed': entry.installed }"
      >
        <div class="catalog-main">
          <span class="catalog-id">PHP {{ entry.id }}</span>
          <span class="catalog-ver">{{ entry.version }}</span>
          <span class="catalog-badge">{{ statusLabel(entry) }}</span>
        </div>
        <p v-if="entry.note" class="catalog-note">{{ entry.note }}</p>
        <div class="catalog-actions">
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
            v-if="entry.installed && !entry.active"
            class="btn btn-ghost"
            type="button"
            :disabled="!!busyAction"
            @click="usePhp(entry.id)"
          >
            Use
          </button>
          <button
            v-if="entry.installed && !entry.active"
            class="btn btn-chip"
            type="button"
            :disabled="!!busyAction"
            @click="removePhp(entry.id)"
          >
            Remove
          </button>
          <span v-if="entry.active" class="catalog-hint">In use</span>
          <span v-if="!entry.downloadable && !entry.installed" class="catalog-hint">
            Unavailable
          </span>
        </div>
      </li>
    </ul>
  </section>
</template>
