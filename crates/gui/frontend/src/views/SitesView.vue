<script setup>
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../composables/useAppStore";
import { siteUrl } from "../lib/urls";

const store = useAppStore();
const domain = ref("");
const projectPath = ref("");
const adding = ref(false);
const opening = ref("");

const sites = computed(() => store.state.info?.settings?.vhosts || []);
const httpPort = computed(() => store.state.info?.settings?.http_port ?? 8888);
const httpsPort = computed(() => store.state.info?.settings?.https_port ?? 8443);

onMounted(async () => {
  if (!store.state.booted) {
    await store.bootAppData();
  } else {
    await store.refreshInfo();
  }
});

function urlFor(domainName, https = true) {
  return siteUrl(domainName, {
    https,
    httpPort: httpPort.value,
    httpsPort: httpsPort.value,
  });
}

async function openInBrowser(domainName, https = true) {
  const url = urlFor(domainName, https);
  opening.value = `${domainName}:${https ? "https" : "http"}`;
  try {
    await invoke("open_in_browser", { url });
    store.setProgress(`Opened ${url}`, "ok");
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  opening.value = "";
}

async function onSubmit() {
  const d = domain.value.trim();
  const p = projectPath.value.trim();
  if (!d || !p) {
    store.setProgress("Domain and path required.", "error");
    return;
  }
  adding.value = true;
  try {
    await store.addSite(d, p);
    domain.value = "";
    projectPath.value = "";
  } catch {
    /* progress already set */
  }
  adding.value = false;
}
</script>

<template>
  <section class="pane pane-sites view-pane" aria-labelledby="sites-title">
    <div class="pane-head">
      <div>
        <h2 id="sites-title">Sites</h2>
        <p class="view-lede">
          Map a local project folder to a <code>*.test</code> host, then open it in your browser.
        </p>
      </div>
    </div>

    <form class="site-form site-form-add" autocomplete="off" @submit.prevent="onSubmit">
      <input
        id="site-domain"
        v-model="domain"
        name="domain"
        placeholder="myapp.test"
        required
      />
      <input
        id="site-path"
        v-model="projectPath"
        name="path"
        placeholder="/path/to/project"
        required
      />
      <button class="btn btn-go" type="submit" :disabled="adding">
        {{ adding ? "Adding…" : "Add site" }}
      </button>
    </form>

    <div v-if="sites.length === 0" class="sites-empty">
      <p>No sites yet. Add a domain and project path above.</p>
    </div>

    <ul v-else class="site-list">
      <li v-for="site in sites" :key="site.domain" class="site-row">
        <div class="site-meta">
          <span class="domain">{{ site.domain }}</span>
          <span class="path" :title="site.public_path">{{ site.public_path }}</span>
        </div>
        <div class="site-actions">
          <button
            class="btn btn-go btn-open"
            type="button"
            :disabled="opening === `${site.domain}:https`"
            @click="openInBrowser(site.domain, true)"
          >
            Open in browser
          </button>
          <button
            class="btn btn-ghost"
            type="button"
            :disabled="opening === `${site.domain}:http`"
            @click="openInBrowser(site.domain, false)"
          >
            HTTP
          </button>
        </div>
      </li>
    </ul>
  </section>
</template>
