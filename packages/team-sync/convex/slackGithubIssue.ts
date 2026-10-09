import { v } from "convex/values";
import { internal } from "./_generated/api";
import { internalMutation } from "./_generated/server";
import { isOnline, memberForSlackUser } from "./slackFlowState";

/**
 * A GitHub team's "no ticket in the thread" step. Convex has no GitHub token, so the requester's Ghostex creates the issue with `gh` when it claims the command (server/src/team_sync/slack_request.rs, `action: "createIssue"`), and the Slack flow runs again with the new issue (`continueWithIssue`, called from slackFlowReport.ts).
 *
 * CDXC:WorkMode 2026-10-09 DECISION:
 * User: with GitHub as the primary tracker the Slack command still never works without a ticket: with no GitHub issue in the thread, one is created on the requester's Ghostex with `gh`.
 */
export const queueIssueCreation = internalMutation({
  args: { requestId: v.id("slackRequests"), payload: v.any() },
  handler: async (ctx, args) => {
    const request = await ctx.db.get(args.requestId);
    if (!request) throw new Error("The Slack request is gone.");
    const now = Date.now();
    const requester = await memberForSlackUser(ctx, request.teamId, request.slackUserId);
    const commandId = await ctx.db.insert("commands", {
      teamId: request.teamId,
      memberId: requester?._id,
      type: "slack.request",
      payload: args.payload,
      source: "slack",
      status: requester ? "pending" : "unassigned",
      slackUserId: request.slackUserId,
      createdAt: now,
    });
    return { commandId, assigned: requester !== null, online: isOnline(requester, now) };
  },
});

/** The issue exists: run the flow for the request with it, as if the thread had named it. */
export const continueWithIssue = internalMutation({
  args: { requestId: v.string(), ticket: v.string() },
  handler: async (ctx, args) => {
    const requestId = ctx.db.normalizeId("slackRequests", args.requestId);
    if (!requestId || !/^[^/\s]+\/[^#\s]+#\d+$/.test(args.ticket)) return null;
    await ctx.db.patch(requestId, { status: "received", updatedAt: Date.now() });
    await ctx.scheduler.runAfter(0, internal.slackFlow.handleRequest, {
      requestId,
      chosenTickets: [args.ticket],
      createdTicket: args.ticket,
    });
    return null;
  },
});
