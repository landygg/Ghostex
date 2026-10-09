import { ConvexError, v } from "convex/values";
import { internal } from "./_generated/api";
import type { Id } from "./_generated/dataModel";
import { action, internalMutation, mutation } from "./_generated/server";
import { inviteExpiry, newSecret, requireMember, sha256Hex } from "./lib/auth";
import { removeMemberKey } from "./linearKeys";

/**
 * Makes a one-time invite code. Ghostex wraps it in the invite link (`https://<deployment>.convex.site/join?code=…`).
 */
export const create = action({
  args: { memberToken: v.string() },
  handler: async (ctx, args): Promise<{ code: string; expiresAt: number }> => {
    const code = newSecret("gxi_", 18);
    const expiresAt: number = await ctx.runMutation(internal.invites.insertInvite, {
      memberToken: args.memberToken,
      codeHash: await sha256Hex(code),
    });
    return { code, expiresAt };
  },
});

export const insertInvite = internalMutation({
  args: { memberToken: v.string(), codeHash: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    const now = Date.now();
    const expiresAt = inviteExpiry(now);
    await ctx.db.insert("invites", {
      teamId: me.teamId,
      codeHash: args.codeHash,
      createdBy: me._id,
      createdAt: now,
      expiresAt,
    });
    return expiresAt;
  },
});

/**
 * Exchanges an invite code for a new member token. The code works once.
 */
export const join = action({
  args: { code: v.string(), name: v.string() },
  handler: async (
    ctx,
    args,
  ): Promise<{ teamId: Id<"teams">; teamName: string; memberId: Id<"members">; memberToken: string }> => {
    const memberToken = newSecret("gxm_", 32);
    const joined: { teamId: Id<"teams">; teamName: string; memberId: Id<"members"> } = await ctx.runMutation(
      internal.invites.consumeInvite,
      {
        codeHash: await sha256Hex(args.code.trim()),
        name: args.name,
        tokenHash: await sha256Hex(memberToken),
      },
    );
    return { ...joined, memberToken };
  },
});

export const consumeInvite = internalMutation({
  args: { codeHash: v.string(), name: v.string(), tokenHash: v.string() },
  handler: async (ctx, args) => {
    const name = args.name.trim();
    if (!name) throw new ConvexError("Pass your name.");
    const invite = await ctx.db
      .query("invites")
      .withIndex("by_code_hash", (q) => q.eq("codeHash", args.codeHash))
      .unique();
    const now = Date.now();
    if (!invite || invite.usedAt !== undefined || invite.expiresAt < now) {
      throw new ConvexError("This invite link is not valid any more. Ask a teammate for a new one.");
    }
    const team = await ctx.db.get(invite.teamId);
    if (!team) throw new ConvexError("This invite's team no longer exists.");
    const memberId = await ctx.db.insert("members", {
      teamId: team._id,
      name,
      role: "member",
      tokenHash: args.tokenHash,
      joinedAt: now,
    });
    await ctx.db.patch(invite._id, { usedAt: now, usedBy: memberId });
    return { teamId: team._id, teamName: team.name, memberId };
  },
});

/** Removes the caller from the team; their token stops working and their own Linear key is deleted. */
export const leave = mutation({
  args: { memberToken: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    await removeMemberKey(ctx, me);
    await ctx.db.patch(me._id, { removedAt: Date.now() });
    return null;
  },
});
