import { ConvexError, v } from "convex/values";
import type { Doc, Id } from "./_generated/dataModel";
import type { QueryCtx } from "./_generated/server";
import { internalQuery, mutation, query } from "./_generated/server";
import { isOwner, requireMember } from "./lib/auth";

const MAX_INSTRUCTIONS_CHARS = 50_000;
const MAX_CHANNELS = 200;

export type TeamFlowSettings = {
  workingChannelId: string | null;
  watchOnlyChannelIds: string[];
  channelRepos: {
    channelId: string;
    repo: string | null;
    project: string | null;
    linearTeamKey: string | null;
    /** The Linear project (release) new tickets from this channel go to: its name, id or link. */
    linearProject: string | null;
  }[];
  linearTeamKey: string | null;
  defaultRunPlace: "cloud" | "local";
  qcOwnerSlackUserId: string | null;
  instructions: string | null;
  /** `null`: never picked (Linear when the team has a Linear key, otherwise GitHub). */
  tracker: "linear" | "github" | null;
  updatedAt: number | null;
};

async function settingsRow(ctx: QueryCtx, teamId: Id<"teams">): Promise<Doc<"teamFlowSettings"> | null> {
  return await ctx.db
    .query("teamFlowSettings")
    .withIndex("by_team", (q) => q.eq("teamId", teamId))
    .unique();
}

/**
 * The settings with their defaults filled in.
 *
 * CDXC:TeamSync 2026-10-09 DECISION:
 * User: new work from Slack without "cloud" or "local" runs in the cloud by default.
 */
export async function readTeamFlow(ctx: QueryCtx, teamId: Id<"teams">): Promise<TeamFlowSettings> {
  const row = await settingsRow(ctx, teamId);
  return {
    workingChannelId: row?.workingChannelId ?? null,
    watchOnlyChannelIds: row?.watchOnlyChannelIds ?? [],
    channelRepos: (row?.channelRepos ?? []).map((entry) => ({
      channelId: entry.channelId,
      repo: entry.repo ?? null,
      project: entry.project ?? null,
      linearTeamKey: entry.linearTeamKey ?? null,
      linearProject: entry.linearProject ?? null,
    })),
    linearTeamKey: row?.linearTeamKey ?? null,
    defaultRunPlace: row?.defaultRunPlace ?? "cloud",
    qcOwnerSlackUserId: row?.qcOwnerSlackUserId ?? null,
    instructions: row?.instructions ?? null,
    tracker: row?.tracker ?? null,
    updatedAt: row?.updatedAt ?? null,
  };
}

/**
 * Whether a member may change the team flow (these settings and the team-flow steps in teamFlowSteps.ts). `get` reports it as `canEdit` and `set` enforces it, so Settings and the CLI follow this one rule.
 *
 * CDXC:TeamSync 2026-10-09 DECISION:
 * User: team flow settings are "Owners only". Members see them read-only.
 */
export function canEditTeamFlow(member: Doc<"members">): boolean {
  return isOwner(member);
}

/** The team's flow settings, for `ghostex team flow` and the Team flow settings page. */
export const get = query({
  args: { memberToken: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    return { ...(await readTeamFlow(ctx, me.teamId)), canEdit: canEditTeamFlow(me) };
  },
});

export const getForTeam = internalQuery({
  args: { teamId: v.id("teams") },
  handler: async (ctx, args) => await readTeamFlow(ctx, args.teamId),
});

function channelId(text: string, what: string): string {
  const trimmed = text.trim().replace(/^<#([A-Z0-9]+)(\|[^>]*)?>$/, "$1");
  if (!/^[CGD][A-Z0-9]{6,}$/.test(trimmed)) {
    throw new ConvexError(`${what} must be a Slack channel ID like C0123456789 (channel details → About → Channel ID), not "${text}".`);
  }
  return trimmed;
}

function optionalText(value: string | null | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

/**
 * Changes the team's flow settings. Every field is optional; `null` clears a single value. `channelRepos` replaces the whole mapping, `mapChannel` / `unmapChannel` change one row.
 */
export const set = mutation({
  args: {
    memberToken: v.string(),
    workingChannelId: v.optional(v.union(v.string(), v.null())),
    watchOnlyChannelIds: v.optional(v.array(v.string())),
    channelRepos: v.optional(
      v.array(
        v.object({
          channelId: v.string(),
          repo: v.optional(v.union(v.string(), v.null())),
          project: v.optional(v.union(v.string(), v.null())),
          linearTeamKey: v.optional(v.union(v.string(), v.null())),
          linearProject: v.optional(v.union(v.string(), v.null())),
        }),
      ),
    ),
    mapChannel: v.optional(
      v.object({
        channelId: v.string(),
        repo: v.optional(v.union(v.string(), v.null())),
        project: v.optional(v.union(v.string(), v.null())),
        linearTeamKey: v.optional(v.union(v.string(), v.null())),
        linearProject: v.optional(v.union(v.string(), v.null())),
      }),
    ),
    unmapChannel: v.optional(v.string()),
    linearTeamKey: v.optional(v.union(v.string(), v.null())),
    defaultRunPlace: v.optional(v.union(v.literal("cloud"), v.literal("local"))),
    qcOwnerSlackUserId: v.optional(v.union(v.string(), v.null())),
    instructions: v.optional(v.union(v.string(), v.null())),
    /**
     * CDXC:WorkMode 2026-10-09 DECISION:
     * User: "in settings we need to say what is the primary for that workspace (Linear Tickets & Projects or Github Issues & Projects - Need to pick just 1)". A team workspace's choice lives here, with the other team flow settings (owners only).
     */
    tracker: v.optional(v.union(v.literal("linear"), v.literal("github"), v.null())),
  },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    if (!canEditTeamFlow(me)) throw new ConvexError("Only the team's owners can change the team flow.");
    const row = await settingsRow(ctx, me.teamId);
    type Mapping = Doc<"teamFlowSettings">["channelRepos"][number];
    const mapping = (entry: {
      channelId: string;
      repo?: string | null;
      project?: string | null;
      linearTeamKey?: string | null;
      linearProject?: string | null;
    }): Mapping => {
      const repo = optionalText(entry.repo)?.replace(/^https?:\/\/github\.com\//i, "").replace(/\.git$/, "");
      if (repo !== undefined && !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
        throw new ConvexError(`The repo must look like owner/name, not "${entry.repo}".`);
      }
      return {
        channelId: channelId(entry.channelId, "The channel"),
        repo: repo?.toLowerCase(),
        project: optionalText(entry.project),
        linearTeamKey: optionalText(entry.linearTeamKey)?.toUpperCase(),
        linearProject: optionalText(entry.linearProject),
      };
    };
    let channelRepos: Mapping[] = row?.channelRepos ?? [];
    if (args.channelRepos !== undefined) channelRepos = args.channelRepos.map(mapping);
    if (args.mapChannel !== undefined) {
      const next = mapping(args.mapChannel);
      channelRepos = [...channelRepos.filter((entry) => entry.channelId !== next.channelId), next];
    }
    if (args.unmapChannel !== undefined) {
      const removed = channelId(args.unmapChannel, "The channel");
      channelRepos = channelRepos.filter((entry) => entry.channelId !== removed);
    }
    if (channelRepos.length > MAX_CHANNELS) throw new ConvexError(`At most ${MAX_CHANNELS} channels can be mapped.`);
    if (args.instructions && args.instructions.length > MAX_INSTRUCTIONS_CHARS) {
      throw new ConvexError(`The team instructions are longer than ${MAX_INSTRUCTIONS_CHARS} characters.`);
    }
    const fields = {
      workingChannelId:
        args.workingChannelId === undefined
          ? row?.workingChannelId
          : args.workingChannelId === null
            ? undefined
            : channelId(args.workingChannelId, "The working channel"),
      watchOnlyChannelIds:
        args.watchOnlyChannelIds === undefined
          ? (row?.watchOnlyChannelIds ?? [])
          : [...new Set(args.watchOnlyChannelIds.filter((id) => id.trim()).map((id) => channelId(id, "A watch-only channel")))],
      channelRepos,
      linearTeamKey:
        args.linearTeamKey === undefined ? row?.linearTeamKey : optionalText(args.linearTeamKey)?.toUpperCase(),
      defaultRunPlace: args.defaultRunPlace ?? row?.defaultRunPlace ?? "cloud",
      qcOwnerSlackUserId:
        args.qcOwnerSlackUserId === undefined ? row?.qcOwnerSlackUserId : optionalText(args.qcOwnerSlackUserId),
      instructions: args.instructions === undefined ? row?.instructions : optionalText(args.instructions),
      tracker: args.tracker === undefined ? row?.tracker : (args.tracker ?? undefined),
      updatedAt: Date.now(),
      updatedBy: me._id,
    };
    if (fields.workingChannelId && fields.watchOnlyChannelIds.includes(fields.workingChannelId)) {
      throw new ConvexError("The working channel cannot also be watch-only.");
    }
    if (row) await ctx.db.replace(row._id, { teamId: me.teamId, ...fields });
    else await ctx.db.insert("teamFlowSettings", { teamId: me.teamId, ...fields });
    return { ...(await readTeamFlow(ctx, me.teamId)), canEdit: true };
  },
});
