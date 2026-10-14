<script setup>
import { ref } from "vue";
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
const projectPath = ref("");
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
</template>
