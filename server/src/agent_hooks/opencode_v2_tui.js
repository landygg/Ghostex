// ghostex-opencode-session-plugin-marker v3
import { spawn } from "node:child_process";

// CDXC:AgentHooks 2026-09-25 WHY:
// The v2 service is shared by unrelated terminal clients. Only the TUI owns both
// the current conversation and the Ghostex terminal environment.
export default {
  id: "ghostex-session",
  setup(ctx) {
    if (!process.env.GHOSTEX_SESSION_ID || process.env.GHOSTEX_OPENCODE_HOOKS_DISABLED === "1") return;
    const env = process.env;
    if (env.GHOSTEX_INTERNAL_PROMPT_GENERATION === "1" || env.GHOSTEX_INTERNAL_TITLE_GENERATION === "1") return;
    if (!(env.VSMUX_SESSION_STATE_FILE || env.GHOSTEX_SESSION_STATE_FILE || env.ghostex_SESSION_STATE_FILE)
      && !(env.GHOSTEX_GLOBAL_SESSION_REF && env.GHOSTEX_GXSERVER_BASE_URL && env.GHOSTEX_GXSERVER_AUTH_TOKEN_FILE)) return;
    let previous = "";
    let currentSession = "";
    let closed = false;
    // A fresh TUI has no conversation until its first prompt; create its own empty
    // conversation so the GPUI composer can submit that first prompt through the API.
    const startup = setTimeout(async () => {
      if (ctx.ui.router.current().type !== "home" || process.argv.some(arg => /^(?:--session(?:=|$)|-s|--continue(?:=|$)|-c$|--fork(?:=|$))/.test(arg))) return;
      try {
        const info = await ctx.client.session.create({ location: ctx.data.location.default() });
        if (!closed && ctx.ui.router.current().type === "home") ctx.ui.router.navigate({type:"session",sessionID:info.id});
      } catch { /* The TUI's connection and provider setup remain available. */ }
    }, 1500);
    let pending = false;
    let activeChild;
    let timeout;
    let retryAt = 0;
    const failures = new Set();
    const failed = reason => {
      retryAt = Date.now() + 5000;
      if (failures.has(reason)) return;
      failures.add(reason);
      console.error(`[Ghostex] OpenCode hook failed: ${reason}`);
    };
    const report = () => {
      if (closed || pending || Date.now() < retryAt) return;
      const route = ctx.ui.router.current();
      if (route.type !== "session") return;
      const id = ctx.data.session.root(route.sessionID);
      const info = ctx.data.session.get(id);
      if (!info || info.parentID) return;
      const permissions = ctx.data.session.permission.list(id) ?? [];
      const forms = ctx.data.session.form.list(id) ?? [];
      const running = ctx.data.session.status(id) === "running";
      const event = id !== currentSession ? "SessionStart" : permissions.length ? "PermissionRequest" : forms.length ? "AskUserQuestion" : running ? "SessionBusy" : "Stop";
      const identity = JSON.stringify([id, event, info.title, info.time.updated]);
      if (identity === previous) return;
      pending = true;
      const payload = { agent: "opencode", session_id: id, cwd: info.location.directory,
        hook_event_name: event, event, title: info.title };
      let failure;
      let finished = false;
      const finish = reason => {
        if (finished) return;
        finished = true;
        clearTimeout(timeout);
        activeChild = undefined;
        pending = false;
        if (closed) return;
        if (reason) failed(reason);
        else { previous = identity; currentSession = id; failures.clear(); retryAt = 0; }
      };
      try {
        const child = process.platform === "win32"
          ? spawn(__GXSERVER__, ["agent-hook-notify-native", __NOTIFY__, "opencode"], { windowsHide: true, stdio: ["pipe", "ignore", "ignore"] })
          : spawn(__NOTIFY__, [], { stdio: ["pipe", "ignore", "ignore"] });
        activeChild = child;
        child.stdin.on("error", error => { failure = error.code || error.name; child.kill(); });
        child.once("error", error => finish(error.code || error.name));
        child.once("close", (code, signal) => finish(failure || (code === 0 ? null : signal || `exit ${code}`)));
        timeout = setTimeout(() => { failure = "timeout"; child.kill(); }, 5000);
        child.stdin.end(JSON.stringify(payload));
      } catch (error) { activeChild?.kill(); finish(error.code || error.name || "spawn failed"); }
    };
    const timer = setInterval(report, 500);
    report();
    return () => { closed = true; clearTimeout(startup); clearInterval(timer); clearTimeout(timeout); activeChild?.kill(); };
  },
};
