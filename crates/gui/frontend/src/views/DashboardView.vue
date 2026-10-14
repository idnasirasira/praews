<script setup>
import { computed, ref, watch } from "vue";
import { useAppStore } from "../composables/useAppStore";
import { lampClass, statusText } from "../lib/status";

const store = useAppStore();
const phpVersion = ref(store.state.phpVersion);

watch(
  () => store.state.phpVersion,
  (v) => {
    phpVersion.value = v;
  }
);

const services = computed(() => {
  const labels = { Nginx: "Nginx", Php: "PHP", MySql: "MariaDB" };
  return ["Nginx", "Php", "MySql"].map((name) => {
    const status = store.state.statuses[name] || "Stopped";
    return {
      name,
      label: labels[name],
      status,
      lamp: lampClass(status),
      detail: statusText(status),
    };
  });
});
</script>

<template>
  <section class="pane pane-services view-pane" aria-labelledby="svc-title">
    <div class="pane-head">
      <div>
        <h2 id="svc-title">Services</h2>
        <p class="view-lede">Start the stack and watch Nginx, PHP, and MariaDB.</p>
      </div>
      <div class="php-switch">
        <label for="php-version">PHP</label>
        <select id="php-version" v-model="phpVersion">
          <option v-for="ver in store.state.phpVersions" :key="ver" :value="ver">
            {{ ver }}
          </option>
        </select>
        <button
          class="btn btn-ghost"
          type="button"
          @click="store.switchPhp(phpVersion)"
        >
          Switch
        </button>
        <RouterLink class="btn btn-chip php-more" to="/php">More…</RouterLink>
      </div>
    </div>

    <ul class="service-rail">
      <li v-for="svc in services" :key="svc.name">
        <span class="lamp" :class="svc.lamp" />
        <span class="svc-name">{{ svc.label }}</span>
        <span class="svc-detail">{{ svc.detail }}</span>
      </li>
    </ul>
  </section>
</template>
