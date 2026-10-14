<script setup>
import { onMounted, ref } from "vue";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const clearing = ref(false);
const loading = ref(false);

onMounted(async () => {
  loading.value = true;
  try {
    await store.refreshLogs(store.state.logSource);
  } finally {
    loading.value = false;
  }
});

async function selectTab(id) {
  if (id === store.state.logSource && !loading.value) {
    await store.refreshLogs(id);
    return;
  }
  loading.value = true;
  try {
    await store.refreshLogs(id);
  } finally {
    loading.value = false;
  }
}

async function onRefresh() {
  loading.value = true;
  try {
    await store.refreshLogs(store.state.logSource);
  } finally {
    loading.value = false;
  }
}

async function onClear() {
  if (clearing.value) return;
  clearing.value = true;
  try {
    await store.clearLogs();
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  clearing.value = false;
}
</script>

<template>
  <section class="pane pane-logs view-pane" aria-labelledby="logs-title">
    <div class="pane-head">
      <div>
        <h2 id="logs-title">Logs</h2>
        <p class="view-lede">Tails from each service — or All in one view.</p>
      </div>
      <div class="log-actions">
        <button class="btn btn-ghost" type="button" :disabled="loading" @click="onRefresh">
          Refresh
        </button>
        <button
          class="btn btn-ghost btn-clear-logs"
          type="button"
          :disabled="clearing"
          @click="onClear"
        >
          Clear
        </button>
      </div>
    </div>

    <div class="log-tabs" role="tablist" aria-label="Log sources">
      <button
        v-for="tab in store.state.logSources"
        :key="tab.id"
        type="button"
        role="tab"
        class="log-tab"
        :class="{ active: store.state.logSource === tab.id }"
        :aria-selected="store.state.logSource === tab.id"
        @click="selectTab(tab.id)"
      >
        {{ tab.label }}
      </button>
    </div>

    <pre class="logs logs-full" :class="{ 'is-loading': loading }">{{ store.state.logs }}</pre>
  </section>
</template>
