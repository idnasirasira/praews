<script setup>
import { onMounted, computed } from "vue";
import { useRoute } from "vue-router";
import { useAppStore } from "../composables/useAppStore";
import { stackSummary } from "../lib/status";
import markUrl from "../assets/praews-icon.png";

const route = useRoute();
const store = useAppStore();

const nav = [
  { name: "dashboard", label: "Stack", to: "/" },
  { name: "sites", label: "Sites", to: "/sites" },
  { name: "ports", label: "Ports", to: "/ports" },
  { name: "config", label: "Config", to: "/config" },
  { name: "phone", label: "Phone", to: "/phone" },
  { name: "logs", label: "Logs", to: "/logs" },
  { name: "php", label: "PHP", to: "/php" },
  { name: "tools", label: "Tools", to: "/tools" },
];

const stack = computed(() => stackSummary(store.state.statuses));

const fillStyle = computed(() => {
  if (!store.state.busy) return { width: "0%" };
  if (store.state.busyPct == null) return {};
  return { width: `${Math.max(0, Math.min(100, store.state.busyPct))}%` };
});

onMounted(async () => {
  if (!store.state.booted) {
    await store.bootAppData();
  }
});
</script>

<template>
  <div class="shell">
    <header class="masthead">
      <div class="identity">
        <div class="brand-row">
          <img class="brand-mark" :src="markUrl" width="44" height="44" alt="" />
          <h1 class="brand">praews</h1>
          <span class="stack-pill" :class="stack.kind" :title="`Stack · ${stack.label}`">
            <span class="stack-dot" aria-hidden="true" />
            {{ stack.label }}
          </span>
        </div>
        <p class="tagline">portable local stack · zero system pollution</p>
      </div>
      <div class="power">
        <button class="btn btn-go" type="button" @click="store.startAll()">Start all</button>
        <button class="btn btn-halt" type="button" @click="store.stopAll()">Stop all</button>
      </div>
    </header>

    <div class="status-bar status-top" role="status" aria-live="polite">
      <div class="status-track">
        <div
          class="status-fill"
          :class="{ indeterminate: store.state.busy && store.state.busyPct == null }"
          :style="fillStyle"
        />
      </div>
      <p
        class="progress-line"
        :class="{
          'is-error': store.state.progress.kind === 'error',
          'is-ok': store.state.progress.kind === 'ok',
        }"
      >
        {{ store.state.progress.text }}
      </p>
    </div>

    <p class="meta-line">
      <span>LAN {{ store.lanIp }}</span>
      <span class="dot" aria-hidden="true" />
      <span>{{ store.appRoot }}</span>
    </p>

    <nav class="nav-rail" aria-label="Sections">
      <RouterLink
        v-for="item in nav"
        :key="item.name"
        :to="item.to"
        class="nav-link"
        :class="{ active: route.name === item.name }"
      >
        {{ item.label }}
      </RouterLink>
    </nav>

    <main class="view-frame">
      <RouterView v-slot="{ Component, route }">
        <Transition name="page" mode="out-in">
          <component :is="Component" :key="route.name" />
        </Transition>
      </RouterView>
    </main>
  </div>
</template>
