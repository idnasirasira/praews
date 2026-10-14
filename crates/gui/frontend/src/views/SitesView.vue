<script setup>
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../composables/useAppStore";
import { siteUrl } from "../lib/urls";

const store = useAppStore();
const domain = ref("");
const projectPath = ref("");
const adding = ref(false);
const opening = ref("");
const picking = ref(false);
const htdocsRoot = ref("");
const htdocsFolders = ref([]);
const loadingFolders = ref(false);

const sites = computed(() => store.state.info?.settings?.vhosts || []);
const httpPort = computed(() => store.state.info?.settings?.http_port ?? 8888);
const httpsPort = computed(() => store.state.info?.settings?.https_port ?? 8443);
const resolverOk = computed(() => store.state.info?.macos_resolver_ok !== false);
const dnsPort = computed(() => store.state.info?.dns_port ?? 53535);
const registeredPaths = computed(
  () => new Set(sites.value.map((s) => s.project_path || s.public_path))
);

watch(
  () => store.state.info?.htdocs_root,
  (root) => {
    if (root) htdocsRoot.value = root;
  },
  { immediate: true }
);

onMounted(async () => {
  if (!store.state.booted) {
    await store.bootAppData();
  } else {
    await store.refreshInfo();
  }
  await refreshHtdocsFolders();
});

async function refreshHtdocsFolders() {
  loadingFolders.value = true;
  try {
    const result = await invoke("list_htdocs_folders");
    htdocsRoot.value = result.htdocs_root || "";
    htdocsFolders.value = result.folders || [];
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  loadingFolders.value = false;
}

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

async function pickFolder() {
  picking.value = true;
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose project folder",
      defaultPath: projectPath.value.trim() || htdocsRoot.value || undefined,
    });
    if (typeof selected === "string" && selected) {
      projectPath.value = selected;
      if (!domain.value.trim()) {
        const base = selected.split(/[/\\]/).filter(Boolean).pop() || "";
        const slug = base
          .toLowerCase()
          .replace(/[^a-z0-9-]+/g, "-")
          .replace(/^-+|-+$/g, "");
        if (slug) domain.value = `${slug}.test`;
      }
    }
  } catch (err) {
    store.setProgress(String(err), "error");
  }
  picking.value = false;
}

function useFolder(folder) {
  projectPath.value = folder.path;
  if (!domain.value.trim()) {
    domain.value = folder.suggested_domain;
  }
}

function isRegistered(folder) {
  return registeredPaths.value.has(folder.path);
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
    await refreshHtdocsFolders();
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

    <div v-if="!resolverOk" class="resolver-banner" role="status">
      <p>
        Browser hang on <code>*.test</code> is usually DNS.
        macOS needs <code>/etc/resolver/test</code> with
        <code>port {{ dnsPort }}</code> (praews listens there, not :53).
      </p>
      <button
        class="btn btn-go"
        type="button"
        :disabled="store.state.busy"
        @click="store.ensureDnsResolver()"
      >
        Fix *.test DNS…
      </button>
    </div>

    <form class="site-form site-form-add" autocomplete="off" @submit.prevent="onSubmit">
      <input
        id="site-domain"
        v-model="domain"
        name="domain"
        placeholder="myapp.test"
        required
      />
      <div class="input-group path-pick">
        <input
          id="site-path"
          v-model="projectPath"
          name="path"
          placeholder="/path/to/project"
          required
        />
        <button
          class="btn btn-chip"
          type="button"
          :disabled="picking"
          @click="pickFolder"
        >
          {{ picking ? "…" : "Browse…" }}
        </button>
      </div>
      <button class="btn btn-go" type="submit" :disabled="adding">
        {{ adding ? "Adding…" : "Add site" }}
      </button>
    </form>
  </section>

  <section class="pane view-pane root-pane" aria-labelledby="htdocs-folders-title">
    <div class="pane-head">
      <div>
        <h2 id="htdocs-folders-title">In active htdocs</h2>
        <p class="view-lede">
          Folders under
          <code class="inline-path">{{ htdocsRoot || "…" }}</code>
        </p>
      </div>
      <button class="btn btn-ghost" type="button" :disabled="loadingFolders" @click="refreshHtdocsFolders">
        Refresh
      </button>
    </div>

    <div v-if="loadingFolders" class="sites-empty">
      <p>Scanning htdocs…</p>
    </div>
    <div v-else-if="htdocsFolders.length === 0" class="sites-empty">
      <p>No project folders yet in this htdocs directory.</p>
    </div>
    <ul v-else class="htdocs-folder-list" aria-label="Folders in htdocs">
      <li v-for="folder in htdocsFolders" :key="folder.path" class="htdocs-folder-row">
        <div class="site-meta">
          <span class="domain">{{ folder.name }}</span>
          <span class="path" :title="folder.path">{{ folder.path }}</span>
        </div>
        <div class="site-actions">
          <span v-if="isRegistered(folder)" class="catalog-hint">Already a site</span>
          <button
            v-else
            class="btn btn-chip"
            type="button"
            @click="useFolder(folder)"
          >
            Use for new site
          </button>
        </div>
      </li>
    </ul>
  </section>

  <section class="pane view-pane root-pane" aria-labelledby="registered-sites-title">
    <div class="pane-head">
      <div>
        <h2 id="registered-sites-title">Registered sites</h2>
        <p class="view-lede">Virtual hosts currently wired into Nginx.</p>
      </div>
    </div>

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
