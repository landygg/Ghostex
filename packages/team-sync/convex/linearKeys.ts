import { ConvexError, v } from "convex/values";
import type { Doc, Id } from "./_generated/dataModel";
import type { MutationCtx, QueryCtx } from "./_generated/server";
import { internalQuery, mutation } from "./_generated/server";
import { isOwner, requireMember } from "./lib/auth";

/**
 * The team's Linear key and each member's own key (the `linearKeys` table). Owners set or remove the team key (`ghostex team linear-connect`, Settings); a member stores or removes only their own. Every public function here answers with the keys' status, never with a key: only `forFlow`, an internal query the Slack flow runs, reads them.
 *
 * CDXC:TeamSync 2026-10-09 DECISION:
 * User: "let's just use 1 key from the owner but also allow the user to override by setting their own key". Tickets the Slack flow creates for a member with their own key show that member as the creator in Linear; everything else uses the team key.
 *
 * SEE-ALSO: server/src/team_sync/linear_keys.rs (verifies a key with Linear before it is stored, and keeps a member's key in sync with their workspace's key), slackFlow.ts.
 */

const MAX_KEY_CHARS = 200;
const MAX_NAME_CHARS = 200;

function cleanKey(key: string): string {
  const trimmed = key.trim();
  if (!/^lin_(api|oauth)_\S+$/.test(trimmed) || trimmed.length > MAX_KEY_CHARS) {
    throw new ConvexError("Pass a Linear API key (starts with lin_api_).");
  }
  return trimmed;
}

function cleanName(name: string | undefined): string | undefined {
  const trimmed = name?.trim();
  return trimmed ? trimmed.slice(0, MAX_NAME_CHARS) : undefined;
}

async function keyRow(ctx: QueryCtx, teamId: Id<"teams">, memberId: Id<"members"> | undefined): Promise<Doc<"linearKeys"> | null> {
  return await ctx.db
    .query("linearKeys")
    .withIndex("by_team_member", (q) => q.eq("teamId", teamId).eq("memberId", memberId))
    .unique();
}

async function storeKey(
  ctx: MutationCtx,
  me: Doc<"members">,
  memberId: Id<"members"> | undefined,
  key: string,
  linearUserName: string | undefined,
): Promise<void> {
  const fields = { key: cleanKey(key), linearUserName: cleanName(linearUserName), setBy: me._id, updatedAt: Date.now() };
  const row = await keyRow(ctx, me.teamId, memberId);
  if (row) await ctx.db.patch(row._id, fields);
  else await ctx.db.insert("linearKeys", { teamId: me.teamId, memberId, ...fields });
}

async function removeKey(ctx: MutationCtx, teamId: Id<"teams">, memberId: Id<"members"> | undefined): Promise<boolean> {
  const row = await keyRow(ctx, teamId, memberId);
  if (row) await ctx.db.delete(row._id);
  return row !== null;
}

/** Removes a member's own key; `invites:leave` runs it so a member who leaves takes their key along. */
export async function removeMemberKey(ctx: MutationCtx, member: Doc<"members">): Promise<void> {
  await removeKey(ctx, member.teamId, member._id);
}

/** Which keys are set, for `teams:info` and the Settings Team rows. Never a key. */
export async function linearKeyStatus(ctx: QueryCtx, me: Doc<"members">) {
  const team = await keyRow(ctx, me.teamId, undefined);
  const mine = await keyRow(ctx, me.teamId, me._id);
  const setBy = team ? await ctx.db.get(team.setBy) : null;
  return {
    team: team
      ? { setByName: setBy?.name ?? null, linearUserName: team.linearUserName ?? null, updatedAt: team.updatedAt }
      : null,
    mine: mine ? { linearUserName: mine.linearUserName ?? null, updatedAt: mine.updatedAt } : null,
  };
}

function requireOwner(me: Doc<"members">): void {
  if (!isOwner(me)) throw new ConvexError("Only the team's owners can set or remove the team's Linear key.");
}

/** Sets the team's Linear key (owners only). `linearUserName` is whose account it acts as. */
export const setTeamKey = mutation({
  args: { memberToken: v.string(), key: v.string(), linearUserName: v.optional(v.string()) },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    requireOwner(me);
    await storeKey(ctx, me, undefined, args.key, args.linearUserName);
    return await linearKeyStatus(ctx, me);
  },
});

export const removeTeamKey = mutation({
  args: { memberToken: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    requireOwner(me);
    await removeKey(ctx, me.teamId, undefined);
    return await linearKeyStatus(ctx, me);
  },
});

/** Stores the caller's own Linear key, which the Slack flow creates the caller's tickets with. */
export const setMyKey = mutation({
  args: { memberToken: v.string(), key: v.string(), linearUserName: v.optional(v.string()) },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    await storeKey(ctx, me, me._id, args.key, args.linearUserName);
    return await linearKeyStatus(ctx, me);
  },
});

export const removeMyKey = mutation({
  args: { memberToken: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    await removeKey(ctx, me.teamId, me._id);
    return await linearKeyStatus(ctx, me);
  },
});

/**
 * The keys one Slack request uses: `readKey` finds and reads tickets (the team key, or the requester's own while the team has none) and `createKey` creates them (the requester's own, else the team key).
 */
export const forFlow = internalQuery({
  args: { teamId: v.id("teams"), memberId: v.optional(v.id("members")) },
  handler: async (ctx, args): Promise<{ readKey: string | null; createKey: string | null }> => {
    const team = (await keyRow(ctx, args.teamId, undefined))?.key ?? null;
    const member = args.memberId ? ((await keyRow(ctx, args.teamId, args.memberId))?.key ?? null) : null;
    return { readKey: team ?? member, createKey: member ?? team };
  },
});
