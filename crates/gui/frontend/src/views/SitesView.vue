<script setup>
import { computed, ref } from "vue";
import { useAppStore } from "../composables/useAppStore";
import { siteUrl } from "../lib/urls";

const store = useAppStore();
const domain = ref("");
const projectPath = ref("");

const httpPort = computed(() => store.state.info?.settings?.http_port ?? 8888);
const httpsPort = computed(() => store.state.info?.settings?.https_port ?? 8443);

function openSite(domainName, https) {
  const url = siteUrl(domainName, {
    https,
    httpPort: httpPort.value,
    httpsPort: httpsPort.value,
  });
  window.open(url, "_blank", "noopener,noreferrer");
}

async function onSubmit() {
  const d = domain.value.trim();
  const p = projectPath.value.trim();
  if (!d || !p) {
    store.setProgress("Domain and path required.", "error");
    return;
  }
  try {
    await store.addSite(d, p);
    domain.value = "";
    projectPath.value = "";
  } catch {
    /* progress already set */
  }
}
</script>

<template>
  <section class="pane pane-sites view-pane" aria-labelledby="sites-title">
    <div class="pane-head">
      <div>
        <h2 id="sites-title">Sites</h2>
        <p class="view-lede">Map a local folder to a <code>*.test</code> host.</p>
      </div>
    </div>

    <ul class="site-list">
      <li v-for="site in store.sites" :key="site.domain" class="site-row">
        <div class="site-meta">
          <span class="domain">{{ site.domain }}</span>
          <span class="path" :title="site.public_path">{{ site.public_path }}</span>
        </div>
        <div class="site-actions">
          <button class="btn btn-ghost" type="button" @click="openSite(site.domain, false)">
            HTTP
          </button>
          <button class="btn btn-ghost" type="button" @click="openSite(site.domain, true)">
            HTTPS
          </button>
        </div>
      </li>
    </ul>

    <form class="site-form" autocomplete="off" @submit.prevent="onSubmit">
      <input v-model="domain" name="domain" placeholder="myapp.test" required />
      <input v-model="projectPath" name="path" placeholder="/path/to/project" required />
      <button class="btn btn-ghost" type="submit">Add</button>
    </form>
  </section>
</template>
