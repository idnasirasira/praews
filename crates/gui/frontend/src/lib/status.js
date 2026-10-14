export function lampClass(status) {
  if (!status) return "bad";
  if (typeof status === "string") return status === "Stopped" ? "bad" : "ok";
  if (status.Running) return "ok";
  if (status.Error) return "bad";
  if (status.Starting !== undefined) return "warn";
  return "bad";
}

/** Singular stack status across Nginx + PHP + MariaDB. */
export function stackSummary(statuses) {
  const keys = ["Nginx", "Php", "MySql"];
  let running = 0;
  let starting = 0;
  let errored = 0;
  for (const key of keys) {
    const s = statuses?.[key];
    if (!s || s === "Stopped") continue;
    if (typeof s === "string") {
      if (s === "Starting") starting += 1;
      else if (s === "Error" || s.startsWith?.("Error")) errored += 1;
      else running += 1;
      continue;
    }
    if (s.Running) running += 1;
    else if (s.Starting !== undefined) starting += 1;
    else if (s.Error) errored += 1;
  }
  if (running === keys.length) return { kind: "running", label: "Running" };
  if (starting > 0 && running === 0) return { kind: "starting", label: "Starting" };
  if (running > 0) return { kind: "partial", label: `Partial ${running}/${keys.length}` };
  if (errored > 0) return { kind: "error", label: "Error" };
  return { kind: "stopped", label: "Stopped" };
}

export function isRunning(status) {
  if (!status) return false;
  if (typeof status === "string") return status !== "Stopped" && status !== "Error";
  return !!status.Running;
}

export function statusText(status) {
  if (!status) return "—";
  if (typeof status === "string") return status.toLowerCase();
  if (status.Running) return `pid ${status.Running.pid} · :${status.Running.port}`;
  if (status.Error) return String(status.Error);
  if (status.Starting !== undefined) return "starting";
  return "—";
}
