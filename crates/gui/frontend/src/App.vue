<script setup>
import { onMounted } from "vue";
import { useAppStore } from "./composables/useAppStore";

const { bindListeners } = useAppStore();

onMounted(() => {
  bindListeners();
});
</script>

<template>
  <div class="atmosphere" aria-hidden="true" />
  <RouterView v-slot="{ Component, route }">
    <Transition name="fade" mode="out-in">
      <component :is="Component" :key="route.meta.bare ? 'setup' : 'app'" />
    </Transition>
  </RouterView>
</template>
