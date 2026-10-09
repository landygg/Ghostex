/**
 * The Work page's wire types: what gxserver's `/api/listWorkItems`, `/api/readWorkItem` and
 * `/api/readTeamFlow` answer (server/src/work_mode/items.rs, item_details.rs, team_flow.rs), and
 * what the desktop's bridge adds (apps/desktop/src/app/work_view/bridge.rs).
 */

export type WorkItemKind = "linearIssue" | "githubIssue" | "pullRequest";

/** Linear tickets & projects, or GitHub issues & projects. */
export type WorkTracker = "linear" | "github";

export type WorkStatusGroup =
  | "backlog"
  | "todo"
  | "progress"
  | "review"
  | "done"
  | "canceled"
  | "draft"
  | "open"
  | "merged"
  | "closed";

export type WorkChecksState = "passing" | "failing" | "pending";

export interface WorkItemStatus {
  group: WorkStatusGroup;
  name: string;
}

export interface WorkItemPerson {
  name: string;
  avatarUrl?: string;
  isMe: boolean;
}

export interface WorkItemLink {
  name: string;
  url?: string;
}

export interface WorkItemPullRequest {
  number: number;
  url?: string;
  state: "open" | "draft" | "merged" | "closed";
  checks?: WorkChecksState;
  reviewDecision?: string;
  title?: string;
}

export interface WorkItemSession {
  projectId: string;
  sessionId: string;
  title: string;
  working: boolean;
  lifecycle: string;
  agentId?: string;
}

export interface WorkItem {
  key: string;
  kind: WorkItemKind;
  id: string;
  title: string;
  url?: string;
  updatedAt?: string;
  status: WorkItemStatus;
  projectId?: string;
  projectName?: string;
  linearProject?: WorkItemLink;
  /** The GitHub Project a GitHub issue or PR is in (a GitHub workspace only). */
  githubProject?: WorkItemLink;
  cycle?: string;
  labels: string[];
  assignee?: WorkItemPerson;
  assignedToMe: boolean;
  pullRequest?: WorkItemPullRequest;
  ticket?: string;
  noTicket: boolean;
  branchName?: string;
  sessions: WorkItemSession[];
  linearIssue?: string;
  githubIssue?: number;
  pullRequestRef?: string;
  /** The ticket's Slack threads, from a Work workspace's team (server/src/team_sync/work_page.rs). */
  slackThreadCount?: number;
}

export interface WorkProject {
  projectId: string;
  name: string;
  repo?: string | null;
}

export interface WorkList {
  items: WorkItem[];
  projects: WorkProject[];
  viewer: { githubLogin?: string | null };
  linearConfigured: boolean;
  /** The workspace's primary tracker (server/src/work_mode/tracker.rs): whose tickets the list shows. */
  tracker?: WorkTracker;
  /** Whether `gh` may read GitHub Projects, the command that lets it, and whether its notice was closed. */
  githubProjects?: {
    access: "granted" | "missingScope" | "unknown";
    command: string;
    noticeDismissed: boolean;
  };
  ghAvailable: boolean;
  errors: string[];
  generatedAt: string;
  refreshing: boolean;
}

/** How a request names one item: exactly one of the ticket fields. */
export interface WorkItemRef {
  projectId?: string;
  linearIssue?: string;
  githubIssue?: number;
  pullRequest?: string;
}

export interface WorkComment {
  author?: string | null;
  avatarUrl?: string | null;
  body: string;
  createdAt?: string | null;
}

export interface LinearIssueDetails {
  identifier: string;
  title?: string;
  url?: string;
  description?: string | null;
  updatedAt?: string;
  branchName?: string;
  priority?: string;
  stateName?: string;
  stateType?: string;
  assignee?: { name?: string; avatarUrl?: string; isMe: boolean } | null;
  creator?: string | null;
  team?: { key?: string; name?: string } | null;
  linearProject?: { name?: string; url?: string } | null;
  cycle?: string | null;
  labels?: string[] | null;
  comments: WorkComment[];
  commentCount: number;
  attachments: {
    title: string;
    subtitle?: string | null;
    url: string;
    source?: string | null;
  }[];
}

export interface GithubIssueDetails {
  number: number;
  title?: string;
  url?: string;
  state?: string;
  body?: string;
  author?: string | null;
  assignees?: string[] | null;
  labels?: string[] | null;
  comments: WorkComment[];
  commentCount: number;
}

export type WorkCheckStatus = "passed" | "failed" | "pending" | "skipped";

export interface WorkCheck {
  name: string;
  workflow?: string | null;
  status: WorkCheckStatus;
  url?: string | null;
  durationSeconds?: number | null;
}

export interface PullRequestDetails {
  number: number;
  title?: string;
  url: string;
  state: "open" | "draft" | "merged" | "closed";
  body?: string;
  author?: string | null;
  headBranch?: string | null;
  baseBranch?: string | null;
  labels?: string[] | null;
  reviewDecision?: string | null;
  reviews: { approved: number; changesRequested: number; commented: number };
  unresolvedReviewThreads?: number | null;
  checks: WorkCheck[];
  checksSummary: {
    total: number;
    passed: number;
    failed: number;
    pending: number;
    skipped: number;
  };
  additions?: number;
  deletions?: number;
  changedFiles?: number;
}

export interface WorkMedia {
  kind: "loom" | "youtube" | "video";
  url: string;
  embedUrl: string;
}

export type TeamFlowStepStatus =
  "done" | "current" | "pending" | "failed" | "unknown";

export interface TeamFlowStepState {
  id: string;
  label: string;
  status: TeamFlowStepStatus;
  detail: string;
  /** Where the step's evidence opens (the Slack working thread, the validation thread). */
  url?: string;
}

export interface SlackFile {
  id: string;
  name?: string | null;
  mimetype?: string | null;
  permalink?: string | null;
}

export interface SlackMessage {
  ts: string;
  userId?: string | null;
  botId?: string | null;
  authorName?: string | null;
  isApp: boolean;
  /** Slack's markup with mentions already named; links stay `<url|label>`. */
  text: string;
  files: SlackFile[];
  postedAt: number;
  editedAt?: number | null;
  media: WorkMedia[];
}

/** `working`: the ticket's one working thread; `watchOnly`: a watch-only (validation) channel. */
export type SlackThreadRole = "working" | "source" | "watchOnly";

export interface SlackThread {
  id: string;
  channelId: string;
  channelName?: string | null;
  threadTs: string;
  permalink?: string | null;
  role: SlackThreadRole;
  isWorkingThread: boolean;
  replyCount: number;
  /** Replies between the first message and the latest ones, shown only in Slack. */
  hiddenReplyCount: number;
  lastMessageAt?: number | null;
  messages: SlackMessage[];
}

/** A session the team's Convex project knows for the ticket (`ticketSessions`). */
export interface TeamSession {
  id: string;
  memberName?: string | null;
  isMe: boolean;
  runPlace: "cloud" | "local";
  status: "starting" | "running" | "failed" | "cancelled";
  projectId?: string | null;
  sessionId?: string | null;
  sessionUrl?: string | null;
  branch?: string | null;
  runnerName?: string | null;
  error?: string | null;
  createdAt: number;
  updatedAt: number;
}

/** A Work workspace's team data for one ticket; absent without a team connection. */
export interface WorkTeamDetails {
  connected: boolean;
  teamName?: string | null;
  ticket: string;
  threads: SlackThread[];
  sessions: TeamSession[];
}

export interface WorkItemDetails {
  item: WorkItem | null;
  linear?: LinearIssueDetails | null;
  githubIssue?: GithubIssueDetails | null;
  pullRequest?: PullRequestDetails | null;
  /** A PR's tickets (`SPX-1245`, `#218`): its sessions' links, its branch, Linear's attachment. */
  tickets?: string[];
  media: WorkMedia[];
  links: { title: string; subtitle?: string | null; url: string }[];
  teamFlow: { source: string; steps: TeamFlowStepState[] };
  team?: WorkTeamDetails | null;
  projects: WorkProject[];
  errors: string[];
}

export interface WorkAgent {
  id: string;
  name?: string | null;
  primary: boolean;
  iconDataUrl?: string | null;
}

/** The desktop's answer to `work.ready`. */
export interface WorkReady {
  projectIds: string[];
  agents: WorkAgent[];
  pendingOpen?: WorkItemRef | null;
}

/** `/api/startWorkOnTicket` (server/src/work_mode/start_work.rs). */
export interface StartWorkResult {
  projectId: string;
  sessionId: string;
  branch?: string;
  worktreePath?: string;
}
