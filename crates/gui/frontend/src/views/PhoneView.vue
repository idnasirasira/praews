<script setup>
import { useAppStore } from "../composables/useAppStore";

const store = useAppStore();
</script>

<template>
  <section class="pane pane-mobile view-pane" aria-labelledby="mobile-title">
    <div class="pane-head">
      <div>
        <h2 id="mobile-title">Phone</h2>
        <p class="view-lede">Reach this machine from your phone on the same Wi‑Fi.</p>
      </div>
    </div>

    <p class="copy">{{ store.dnsHint }}</p>
    <div class="link-stack">
      <a :href="store.rootCaUrl" target="_blank" rel="noreferrer">Install Root CA</a>
      <a :href="store.mobileUrl" target="_blank" rel="noreferrer">{{ store.mobileUrl }}</a>
    </div>

    <div class="phone-actions">
      <button class="btn btn-chip" type="button" @click="store.trustCa()">
        Trust CA on this Mac
      </button>
    </div>
  </section>
</template>
