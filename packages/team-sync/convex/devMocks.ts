import type { HttpRouter } from "convex/server";
import { v } from "convex/values";
import { internal } from "./_generated/api";
import { httpAction, internalMutation, internalQuery } from "./_generated/server";
import { upsertMessage } from "./slackIntake";
import { findThread, upsertThread } from "./slackThreads";

/**
 * Stand-ins for the Slack Web API and Linear's GraphQL API, served by this deployment at `/dev-mocks/slack/<method>` and `/dev-mocks/linear`, so the Slack command flow can be driven end to end without posting into a real Slack workspace or creating real Linear tickets.
 *
 * CDXC:TeamSync 2026-10-09 WHY:
 * The deployment runs in Convex's cloud, so a mock on the tester's computer is unreachable; the mock lives here instead and answers only while `GHOSTEX_DEV_MOCKS=1` is set on the deployment (point `SLACK_API_BASE_URL` at `<site>/dev-mocks/slack` and `LINEAR_API_URL` at `<site>/dev-mocks/linear`). Every call is recorded in `devMockCalls`.
 */

function enabled(): boolean {
  return process.env.GHOSTEX_DEV_MOCKS === "1";
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function fakeTs(): string {
  const now = Date.now();
  return `${Math.floor(now / 1000)}.${String(now % 1000).padStart(3, "0")}${String(Math.floor(Math.random() * 1000)).padStart(3, "0")}`;
}

/** `URANA` → "Rana": test users are named by their id. */
function mockName(userId: string): string {
  const name = userId.replace(/^U/, "").toLowerCase();
  return name.charAt(0).toUpperCase() + name.slice(1);
}

export const record = internalMutation({
  args: { service: v.union(v.literal("slack"), v.literal("linear")), method: v.string(), body: v.string() },
  handler: async (ctx, args) => {
    await ctx.db.insert("devMockCalls", { ...args, at: Date.now() });
    return (await ctx.db.query("devMockCalls").collect()).filter(
      (call) => call.service === args.service && call.method === args.method,
    ).length;
  },
});

export const storedThread = internalQuery({
  args: { channel: v.string(), ts: v.string() },
  handler: async (ctx, args) => {
    const team = await ctx.db.query("teams").first();
    if (!team) return [];
    const thread = await findThread(ctx, team._id, args.channel, args.ts);
    if (!thread) return [];
    const messages = await ctx.db
      .query("slackMessages")
      .withIndex("by_thread_ts", (q) => q.eq("threadId", thread._id))
      .collect();
    return messages
      .filter((message) => message.deletedAt === undefined)
      .map((message) => ({
        ts: message.ts,
        thread_ts: thread.threadTs,
        user: message.userId,
        bot_id: message.botId,
        text: message.text,
        files: message.files.map((file) => ({ id: file.id, name: file.name, mimetype: file.mimetype, url_private: file.urlPrivate })),
      }));
  },
});

/** Seeds a Slack thread the mock's `conversations.replies` returns (a real Slack thread exists before anyone mentions Ghostex). */
export const seedThread = internalMutation({
  args: {
    channelId: v.string(),
    threadTs: v.string(),
    messages: v.array(
      v.object({
        ts: v.string(),
        user: v.string(),
        text: v.string(),
        files: v.optional(v.array(v.object({ id: v.string(), name: v.string(), mimetype: v.string(), url_private: v.string() }))),
      }),
    ),
  },
  handler: async (ctx, args) => {
    const team = await ctx.db.query("teams").first();
    if (!team) throw new Error("No team.");
    const thread = await upsertThread(ctx, team._id, { channelId: args.channelId, threadTs: args.threadTs });
    for (const message of args.messages) await upsertMessage(ctx, team, thread, { ...message, thread_ts: args.threadTs });
    return thread._id;
  },
});

/** The calls the mocks received since `since` (ms), oldest first. */
export const calls = internalQuery({
  args: { since: v.optional(v.number()) },
  handler: async (ctx, args) =>
    (await ctx.db.query("devMockCalls").collect())
      .filter((call) => call.at >= (args.since ?? 0))
      .map((call) => ({ service: call.service, method: call.method, body: JSON.parse(call.body), at: call.at })),
});

const ONE_PIXEL_PNG = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg=="),
  (character) => character.charCodeAt(0),
);

const slackMock = httpAction(async (ctx, request) => {
  if (!enabled()) return new Response("Not found", { status: 404 });
  const url = new URL(request.url);
  const method = url.pathname.replace(/^\/dev-mocks\/slack\//, "");
  if (method.startsWith("files/")) {
    await ctx.runMutation(internal.devMocks.record, { service: "slack", method: "files.download", body: JSON.stringify({ path: method }) });
    return new Response(ONE_PIXEL_PNG, { status: 200, headers: { "content-type": "image/png" } });
  }
  const text = request.method === "POST" ? await request.text() : "";
  const body: Record<string, any> =
    request.method === "GET" ? Object.fromEntries(url.searchParams) : text ? JSON.parse(text) : {};
  await ctx.runMutation(internal.devMocks.record, { service: "slack", method, body: JSON.stringify(body) });
  switch (method) {
    case "chat.postMessage":
      return json({ ok: true, channel: body.channel, ts: fakeTs() });
    case "chat.postEphemeral":
      return json({ ok: true, message_ts: fakeTs() });
    case "reactions.add":
      return json({ ok: true });
    case "chat.getPermalink":
      return json({ ok: true, channel: body.channel, permalink: `https://mock.slack.test/archives/${body.channel}/p${String(body.message_ts).replace(".", "")}` });
    case "conversations.replies": {
      const messages = await ctx.runQuery(internal.devMocks.storedThread, { channel: body.channel, ts: body.ts });
      return json({ ok: true, messages, has_more: false });
    }
    case "users.info":
      return json({ ok: true, user: { id: body.user, name: mockName(body.user).toLowerCase(), real_name: mockName(body.user), profile: { display_name: mockName(body.user) } } });
    default:
      // response_url replies (`/dev-mocks/slack/response_url/…`) and anything else.
      return json({ ok: true });
  }
});

function slug(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
}

const MOCK_TEAMS = [
  { id: "team-spx", key: "SPX", name: "ShortPoint" },
  { id: "team-con", key: "CON", name: "Connectors" },
];

/** `Release 10.4` is SPX's; a link to it ends in its slug id `a1b2c3d4e5f6`. */
const MOCK_PROJECTS = [
  { id: "project-104", name: "Release 10.4", slugId: "a1b2c3d4e5f6", url: "https://linear.app/mock/project/release-104-a1b2c3d4e5f6", teams: { nodes: [{ id: "team-spx" }] } },
];

function mockIssue(identifier: string, title: string, description: string | null, projectId?: string) {
  return {
    identifier,
    title,
    url: `https://linear.app/mock/issue/${identifier.toLowerCase()}/${slug(title)}`,
    branchName: `yahia/${identifier.toLowerCase()}-${slug(title)}`,
    description,
    team: { key: identifier.split("-")[0], name: MOCK_TEAMS.find((team) => team.key === identifier.split("-")[0])?.name ?? "Team" },
    project: MOCK_PROJECTS.find((project) => project.id === projectId) ? { name: MOCK_PROJECTS.find((project) => project.id === projectId)!.name } : null,
    assignee: null,
    state: { name: "Todo" },
    labels: { nodes: [] },
    comments: { nodes: [{ body: "Mock comment.", createdAt: "2026-10-09T10:00:00.000Z", user: { name: "Omar" } }] },
    attachments: { nodes: [] },
  };
}

const linearMock = httpAction(async (ctx, request) => {
  if (!enabled()) return new Response("Not found", { status: 404 });
  const body = (await request.json()) as { query: string; variables: Record<string, any> };
  const kind = body.query.includes("issueCreate")
    ? "issueCreate"
    : body.query.includes("teams(first: 250)")
      ? "teams"
      : body.query.includes("projects(") || body.query.includes("project(")
        ? "projects"
        : "issue";
  // The key's last characters only, so a test can tell which key (the team's or a member's) made the call.
  const keyTail = (request.headers.get("authorization") ?? "").slice(-6);
  const count = await ctx.runMutation(internal.devMocks.record, { service: "linear", method: kind, body: JSON.stringify({ ...body, keyTail }) });
  if (kind === "teams") return json({ data: { teams: { nodes: MOCK_TEAMS } } });
  if (kind === "projects") {
    const vars = body.variables as { slug?: string; name?: string; id?: string };
    const found = MOCK_PROJECTS.filter(
      (project) => project.slugId === vars.slug || project.id === vars.id || project.name.toLowerCase() === vars.name?.toLowerCase(),
    );
    return json({ data: vars.id ? { project: found[0] ?? null } : { projects: { nodes: found } } });
  }
  if (kind === "issueCreate") {
    const input = body.variables.input as { teamId: string; title: string; description?: string; projectId?: string };
    const key = MOCK_TEAMS.find((team) => team.id === input.teamId)?.key ?? "SPX";
    const issue = mockIssue(`${key}-${1252 + count}`, input.title, input.description ?? null, input.projectId);
    return json({ data: { issueCreate: { success: true, issue } } });
  }
  const id = String(body.variables.id ?? "");
  if (!/^(SPX|CON)-\d+$/.test(id)) return json({ data: null, errors: [{ message: "Entity not found: Issue" }] });
  return json({ data: { issue: mockIssue(id, `Mock ticket ${id}`, "Mock description.") } });
});

export function registerDevMocks(http: HttpRouter): void {
  http.route({ pathPrefix: "/dev-mocks/slack/", method: "POST", handler: slackMock });
  http.route({ pathPrefix: "/dev-mocks/slack/", method: "GET", handler: slackMock });
  http.route({ path: "/dev-mocks/linear", method: "POST", handler: linearMock });
}
