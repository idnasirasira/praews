<script setup>
import { computed } from "vue";
import { useAppStore } from "../composables/useAppStore";
import { isRunning, lampClass, statusText } from "../lib/status";

const store = useAppStore();

const services = computed(() => {
  const labels = { Nginx: "Nginx", Php: "PHP", MySql: "MariaDB" };
  return ["Nginx", "Php", "MySql"].map((name) => {
    const status = store.state.statuses[name] || "Stopped";
    const running = isRunning(status);
    return {
      name,
      label: labels[name],
      status,
      running,
      lamp: lampClass(status),
      detail: statusText(status),
    };
  });
});

const phpVersions = computed(() => store.state.phpVersions || []);
const activePhp = computed(() => store.state.phpVersion);

async function toggleService(svc) {
  if (store.state.busy) return;
  if (svc.running) {
    await store.stopService(svc.name);
  } else {
    await store.startService(svc.name);
  }
}

async function pickPhp(ver) {
  if (store.state.busy || ver === activePhp.value) return;
  await store.switchPhp(ver);
}
</script>

<template>
  <section class="pane pane-services view-pane" aria-labelledby="svc-title">
    <div class="pane-head">
      <div>
        <h2 id="svc-title">Services</h2>
        <p class="view-lede">Start the stack and watch Nginx, PHP, and MariaDB.</p>
      </div>
      <div class="php-switch" role="group" aria-label="Active PHP version">
        <span class="php-switch-label">PHP</span>
        <div class="php-chips">
          <button
            v-for="ver in phpVersions"
            :key="ver"
            type="button"
            class="php-chip"
            :class="{ active: ver === activePhp }"
            :disabled="store.state.busy"
            :aria-pressed="ver === activePhp"
            @click="pickPhp(ver)"
          >
            {{ ver }}
          </button>
        </div>
        <RouterLink class="btn btn-chip php-more" to="/php">Manage</RouterLink>
      </div>
    </div>

    <ul class="service-rail">
      <li v-for="svc in services" :key="svc.name">
        <span class="lamp" :class="svc.lamp" />
        <div class="svc-meta">
          <span class="svc-name">{{ svc.label }}</span>
          <span class="svc-detail">{{ svc.detail }}</span>
        </div>
        <button
          class="btn btn-chip"
          :class="svc.running ? 'btn-halt' : 'btn-go'"
          type="button"
          :disabled="store.state.busy"
          @click="toggleService(svc)"
        >
          {{ svc.running ? "Stop" : "Start" }}
        </button>
      </li>
    </ul>
  </section>
</template>
