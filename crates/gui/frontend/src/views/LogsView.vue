<script setup>
import { ref } from "vue";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const clearing = ref(false);

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
        <p class="view-lede">Recent tails from praews and the stack processes.</p>
      </div>
      <div class="log-actions">
        <button class="btn btn-ghost" type="button" @click="store.refreshLogs()">
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
    <pre class="logs logs-full">{{ store.state.logs }}</pre>
  </section>
</template>
