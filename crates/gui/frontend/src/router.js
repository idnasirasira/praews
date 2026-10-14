import { createRouter, createWebHashHistory } from "vue-router";
import { invoke } from "@tauri-apps/api/core";

const routes = [
  {
    path: "/setup",
    name: "setup",
    component: () => import("./views/SetupView.vue"),
    meta: { bare: true },
  },
  {
    path: "/",
    component: () => import("./layouts/AppLayout.vue"),
    children: [
      {
        path: "",
        name: "dashboard",
        component: () => import("./views/DashboardView.vue"),
      },
      {
        path: "sites",
        name: "sites",
        component: () => import("./views/SitesView.vue"),
      },
      {
        path: "phone",
        name: "phone",
        component: () => import("./views/PhoneView.vue"),
      },
      {
        path: "logs",
        name: "logs",
        component: () => import("./views/LogsView.vue"),
      },
      {
        path: "php",
        name: "php",
        component: () => import("./views/PhpView.vue"),
      },
      {
        path: "tools",
        name: "tools",
        component: () => import("./views/ToolsView.vue"),
      },
      {
        path: "settings",
        redirect: { name: "tools" },
      },
      {
        path: "ports",
        name: "ports",
        component: () => import("./views/PortsView.vue"),
      },
    ],
  },
];

const router = createRouter({
  // Hash history works with Tauri's custom protocol; HTML5 history does not.
  history: createWebHashHistory(),
  routes,
});

let bootChecked = false;

router.beforeEach(async (to) => {
  if (bootChecked && to.name !== "setup") return true;
  try {
    const status = await invoke("get_setup_status");
    bootChecked = true;
    if (!status.ready && to.name !== "setup") {
      return { name: "setup" };
    }
    if (status.ready && to.name === "setup") {
      return { name: "dashboard" };
    }
  } catch (err) {
    console.error("setup status check failed", err);
    bootChecked = true;
    if (to.name !== "setup") return { name: "setup" };
  }
  return true;
});

export function markSetupComplete() {
  bootChecked = true;
}

export default router;
