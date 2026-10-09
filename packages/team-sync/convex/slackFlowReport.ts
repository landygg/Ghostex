import { v } from "convex/values";
import { internal } from "./_generated/api";
import { internalAction } from "./_generated/server";
import { postMessage, postPrivateNote } from "./lib/slackApi";

/**
 * Step 5 for the session part: once the requester's Ghostex finished a `slack.request` command, the working thread says where Claude runs and the requester gets a private summary of everything Ghostex did.
 *
 * CDXC:TeamSync 2026-10-09 DECISION:
 * User: the source thread gets two messages, ever: the working thread's link now and the final result (PR, QC package version, video) at the end; milestones go to the working thread. So nothing here posts in the source thread.
 */
export const reportCommand = internalAction({
  args: { commandId: v.id("commands") },
  handler: async (ctx, args): Promise<null> => {
    await ctx.runMutation(internal.slackFlowState.recordSessionResult, { commandId: args.commandId });
    const report = await ctx.runQuery(internal.slackFlowState.commandReport, { commandId: args.commandId });
    if (!report) return null;
    const { command, memberName } = report;
    const payload = (command.payload ?? {}) as {
      action?: "start" | "message";
      ticket?: { key?: string; url?: string | null; teamName?: string | null; projectName?: string | null };
      runPlace?: "cloud" | "local";
      requester?: { slackUserId?: string };
      source?: { channelId?: string; threadTs?: string | null };
      workingThread?: { channelId?: string; threadTs?: string; permalink?: string | null };
      createdTicket?: boolean;
      openedWorkingThread?: boolean;
    };
    const ticket = payload.ticket?.key ?? "the ticket";
    const ticketLink = payload.ticket?.url ? `<${payload.ticket.url}|${ticket}>` : ticket;
    const working = payload.workingThread;
    const requester = payload.requester?.slackUserId;
    const note = async (text: string) => {
      if (!requester || !payload.source?.channelId) return;
      // A private note needs the bot in the channel; the working thread already carries the same news, so a refusal is fine.
      await postPrivateNote({
        channel: payload.source.channelId,
        user: requester,
        threadTs: payload.source.threadTs ?? undefined,
        text,
      }).catch(() => undefined);
    };
    const inWorkingThread = async (text: string) => {
      if (working?.channelId && working.threadTs) {
        await postMessage({ channel: working.channelId, threadTs: working.threadTs, text }).catch(() => undefined);
      }
    };
    const result = (command.result ?? {}) as {
      runPlace?: string;
      reused?: boolean;
      branch?: string | null;
      sessionUrl?: string | null;
      machine?: string | null;
      runner?: string | null;
    };

    if ((payload as { action?: string }).action === "createIssue") {
      // A GitHub team's ticket, created by the requester's Ghostex: run the flow again with it.
      const created = (command.result ?? {}) as { ticket?: string };
      const requestId = (command.payload as { requestId?: string } | null)?.requestId;
      if (command.status === "done" && created.ticket && requestId) {
        await ctx.runMutation(internal.slackGithubIssue.continueWithIssue, { requestId, ticket: created.ticket });
      } else if (command.status !== "cancelled") {
        await note(`Couldn't create a GitHub issue for this: ${command.error ?? "it failed"}.`);
      }
      return null;
    }
    if (payload.action === "message") {
      if (command.status === "failed") await note(`Couldn't send your message to ${ticket}'s session: ${command.error ?? "it failed"}.`);
      return null;
    }
    if (command.status === "cancelled") return null;
    if (command.status !== "done") {
      const text = `Couldn't start Claude for ${ticketLink}: ${command.error ?? "it failed"}.`;
      await inWorkingThread(text);
      await note(text);
      return null;
    }

    const where = memberName ? `${memberName}'s computer` : "the requester's computer";
    const branch = result.branch ? ` on \`${result.branch}\`` : "";
    const open = result.sessionUrl ? ` · <${result.sessionUrl}|Open session>` : "";
    const started = result.reused
      ? `Sent the request to ${ticket}'s session on ${where}.`
      : result.runPlace === "cloud"
        ? `Started Claude in the cloud${branch}${open}`
        : `Started Claude on ${where}${result.machine ? ` (${result.machine})` : ""}${branch}.`;
    await inWorkingThread(started);

    const lines = ["On it:"];
    if (payload.createdTicket) {
      const team =
        (payload.ticket?.teamName ? ` in ${payload.ticket.teamName}` : "") +
        (payload.ticket?.projectName ? ` (project ${payload.ticket.projectName})` : "");
      lines.push(
        payload.source?.threadTs
          ? `• No ticket in this thread, so I created ${ticketLink}${team} and linked this thread to it.`
          : `• Created ${ticketLink}${team}.`,
      );
    }
    if (payload.openedWorkingThread && working?.channelId) {
      const qc = report.qcOwnerSlackUserId && report.qcOwnerSlackUserId !== requester ? ` and <@${report.qcOwnerSlackUserId}>` : "";
      lines.push(`• Opened its ${working.permalink ? `<${working.permalink}|working thread>` : "working thread"} in <#${working.channelId}> and tagged you${qc}.`);
    }
    lines.push(`• ${started}`);
    await note(lines.join("\n"));
    return null;
  },
});

/** Replaces a private note through its `response_url` (after a button click). */
export const replaceNote = internalAction({
  args: { responseUrl: v.string(), text: v.string() },
  handler: async (_ctx, args): Promise<null> => {
    await postPrivateNote({ channel: "", user: "", text: args.text, responseUrl: args.responseUrl, replaceOriginal: true }).catch(
      () => undefined,
    );
    return null;
  },
});
