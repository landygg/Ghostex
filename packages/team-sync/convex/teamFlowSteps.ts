import { ConvexError, v } from "convex/values";
import type { Doc } from "./_generated/dataModel";
import { mutation, query } from "./_generated/server";
import { requireMember } from "./lib/auth";
import { canEditTeamFlow } from "./teamFlow";

/**
 * The team's flow steps (`teamFlowSteps`): one list for the whole team, read by every teammate's Work page and Settings, changed only by owners (`canEditTeamFlow`). `steps: null` means the team has none yet and uses Ghostex's default flow.
 *
 * CDXC:TeamSync 2026-10-09 WHY:
 * The checks here are the ones gxserver runs before it sends the steps (`validate_team_flow_steps` in server/src/work_mode/team_flow.rs), repeated so a teammate's older Ghostex cannot store steps the others can't draw. `RULE_KINDS` mirrors `TEAM_FLOW_RULES` there.
 */

const MAX_STEPS = 20;
const MAX_LABEL_CHARS = 40;
const MAX_ID_CHARS = 40;
const RULE_KINDS = [
  "ticketExists",
  "slackWorkingThread",
  "sessionLinked",
  "pullRequestExists",
  "reviewCommentsResolved",
  "pullRequestApproved",
  "checksPassing",
  "pullRequestLabel",
  "pullRequestMerged",
  "ticketState",
  "videoApproved",
  "slackValidationPost",
];

const stepValidator = v.object({
  id: v.string(),
  label: v.string(),
  rule: v.object({
    kind: v.string(),
    label: v.optional(v.string()),
    states: v.optional(v.array(v.string())),
  }),
});

type Step = Doc<"teamFlowSteps">["steps"][number];

function validateSteps(steps: Step[]): Step[] {
  if (steps.length === 0 || steps.length > MAX_STEPS) throw new ConvexError(`A team flow has 1 to ${MAX_STEPS} steps.`);
  const ids = new Set<string>();
  return steps.map((step, index) => {
    const label = step.label.trim();
    if (!label) throw new ConvexError(`Step ${index + 1} has no label.`);
    if ([...label].length > MAX_LABEL_CHARS) throw new ConvexError(`"${label}" is longer than ${MAX_LABEL_CHARS} characters.`);
    const id = step.id.trim();
    if (!/^[a-z0-9-]+$/.test(id) || id.length > MAX_ID_CHARS || ids.has(id)) {
      throw new ConvexError(`"${label}" needs a unique id of lowercase letters, digits and dashes.`);
    }
    ids.add(id);
    const { kind } = step.rule;
    if (!RULE_KINDS.includes(kind)) throw new ConvexError(`"${label}" uses an unknown rule "${kind}".`);
    const rule: Step["rule"] = { kind };
    if (kind === "pullRequestLabel") {
      const wanted = step.rule.label?.trim();
      if (!wanted) throw new ConvexError(`"${label}" needs the PR label to look for.`);
      rule.label = wanted;
    }
    if (kind === "ticketState") {
      const states = (step.rule.states ?? []).map((state) => state.trim()).filter(Boolean);
      if (states.length === 0) throw new ConvexError(`"${label}" needs the ticket states that count as done.`);
      rule.states = states;
    }
    return { id, label, rule };
  });
}

/** The team's steps (`null` when it has none) and whether the caller may change them. */
export const get = query({
  args: { memberToken: v.string() },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    const row = await ctx.db
      .query("teamFlowSteps")
      .withIndex("by_team", (q) => q.eq("teamId", me.teamId))
      .unique();
    return { steps: row?.steps ?? null, updatedAt: row?.updatedAt ?? null, canEdit: canEditTeamFlow(me) };
  },
});

/** Replaces the team's steps (owners only); `reset: true` removes them so the team uses the default flow again. */
export const set = mutation({
  args: { memberToken: v.string(), steps: v.optional(v.array(stepValidator)), reset: v.optional(v.boolean()) },
  handler: async (ctx, args) => {
    const me = await requireMember(ctx, args.memberToken);
    if (!canEditTeamFlow(me)) throw new ConvexError("Only the team's owners can change the team flow.");
    const row = await ctx.db
      .query("teamFlowSteps")
      .withIndex("by_team", (q) => q.eq("teamId", me.teamId))
      .unique();
    if (args.reset) {
      if (row) await ctx.db.delete(row._id);
      return { steps: null, updatedAt: null, canEdit: true };
    }
    if (!args.steps) throw new ConvexError("Pass steps, or reset: true.");
    const fields = { steps: validateSteps(args.steps), updatedAt: Date.now(), updatedBy: me._id };
    if (row) await ctx.db.patch(row._id, fields);
    else await ctx.db.insert("teamFlowSteps", { teamId: me.teamId, ...fields });
    return { steps: fields.steps, updatedAt: fields.updatedAt, canEdit: true };
  },
});
