export function lampClass(status) {
  if (!status) return "bad";
  if (typeof status === "string") return status === "Stopped" ? "bad" : "ok";
  if (status.Running) return "ok";
  if (status.Error) return "bad";
  if (status.Starting !== undefined) return "warn";
  return "bad";
}

export function statusText(status) {
  if (!status) return "—";
  if (typeof status === "string") return status.toLowerCase();
  if (status.Running) return `pid ${status.Running.pid} · :${status.Running.port}`;
  if (status.Error) return String(status.Error);
  if (status.Starting !== undefined) return "starting";
  return "—";
}
