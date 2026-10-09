import {
  IconAlertTriangle,
  IconArrowsSort,
  IconBox,
  IconChevronDown,
  IconCircleDot,
  IconCopy,
  IconFolder,
  IconGitPullRequest,
  IconHash,
  IconLayoutSidebar,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconX,
} from "@tabler/icons-react";
import { useMemo } from "react";
import {
  Avatar,
  Button,
  Dropdown,
  LiveDot,
  MenuItem,
  PullRequestChip,
  Spinner,
  StatusGlyph,
  Toggle,
  cx,
} from "./components";
import {
  relativeTime,
  STATUS_FILTERS,
  statusMatches,
  type StatusFilter,
} from "./format";
import type {
  WorkItem,
  WorkItemLink,
  WorkList as WorkListData,
  WorkTracker,
} from "./types";

export interface WorkFilters {
  search: string;
  assignedToMe: boolean;
  linearIssues: boolean;
  githubIssues: boolean;
  pullRequests: boolean;
  status: StatusFilter;
  projectId: string;
  /** A Linear project's or a GitHub Project's name, whichever the workspace's tracker uses. */
  trackerProject: string;
  inSidebar: boolean;
}

/**
 * CDXC:WorkMode 2026-10-09 DECISION:
 * User: the Work list opens with "Assigned to me" turned on; the other filters (the kinds, status
 * open, repo, project, "In my sidebar") start wide open.
 */
export const DEFAULT_WORK_FILTERS: WorkFilters = {
  search: "",
  assignedToMe: true,
  linearIssues: true,
  githubIssues: true,
  pullRequests: true,
  status: "open",
  projectId: "",
  trackerProject: "",
  inSidebar: false,
};

/** The project an item shows: a Linear project, or a GitHub Project in a GitHub workspace. */
export function trackerProjectOf(
  item: WorkItem,
  tracker: WorkTracker,
): WorkItemLink | undefined {
  return tracker === "github" ? item.githubProject : item.linearProject;
}

export function filterWorkItems(
  items: WorkItem[],
  filters: WorkFilters,
  tracker: WorkTracker = "linear",
): WorkItem[] {
  const query = filters.search.trim().toLowerCase();
  return items.filter((item) => {
    if (filters.assignedToMe && !item.assignedToMe) return false;
    if (item.kind === "linearIssue" && !filters.linearIssues) return false;
    if (item.kind === "githubIssue" && !filters.githubIssues) return false;
    if (item.kind === "pullRequest" && !filters.pullRequests) return false;
    if (!statusMatches(filters.status, item.status.group)) return false;
    if (filters.projectId && item.projectId !== filters.projectId) return false;
    if (
      filters.trackerProject &&
      trackerProjectOf(item, tracker)?.name !== filters.trackerProject
    )
      return false;
    if (filters.inSidebar && item.sessions.length === 0) return false;
    if (query) {
      const haystack = [
        item.id,
        item.title,
        item.projectName,
        trackerProjectOf(item, tracker)?.name,
        item.assignee?.name,
        item.branchName,
        item.pullRequest ? `#${item.pullRequest.number}` : "",
        ...item.labels,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      if (!haystack.includes(query)) return false;
    }
    return true;
  });
}

function activeFilterCount(filters: WorkFilters): number {
  let count = 0;
  if (filters.assignedToMe) count += 1;
  if (!filters.linearIssues || !filters.githubIssues || !filters.pullRequests)
    count += 1;
  if (filters.status !== "open") count += 1;
  if (filters.projectId) count += 1;
  if (filters.trackerProject) count += 1;
  if (filters.inSidebar) count += 1;
  return count;
}

export function WorkListView({
  data,
  loading,
  error,
  filters,
  onFiltersChange,
  onRefresh,
  onOpen,
  onNewTicket,
  newTicketError,
  onDismissNotice,
  now,
}: {
  data: WorkListData | null;
  loading: boolean;
  error: string | null;
  filters: WorkFilters;
  onFiltersChange: (filters: WorkFilters) => void;
  onRefresh: () => void;
  onOpen: (item: WorkItem) => void;
  /** Opens the app's native Create Linear Ticket dialog (apps/desktop/src/app/work_view/bridge.rs). */
  onNewTicket: () => void;
  newTicketError: string | null;
  /** Closes a notice for good (gxserver remembers it). */
  onDismissNotice: (notice: string) => void;
  now: number;
}) {
  const items = data?.items ?? [];
  // CDXC:WorkMode 2026-10-09 DECISION:
  // User: the Work page lists the workspace's primary tracker's tickets (plus PRs); the other tracker's type filter is hidden, and the project filter is "All projects" for the primary's kind of project.
  const tracker: WorkTracker = data?.tracker ?? "linear";
  const visible = useMemo(
    () => filterWorkItems(items, filters, tracker),
    [items, filters, tracker],
  );
  const trackerProjects = useMemo(
    () =>
      [
        ...new Set(
          items
            .map((item) => trackerProjectOf(item, tracker)?.name)
            .filter((name): name is string => Boolean(name)),
        ),
      ].sort((a, b) => a.localeCompare(b)),
    [items, tracker],
  );
  const projects = data?.projects ?? [];
  const set = (patch: Partial<WorkFilters>) =>
    onFiltersChange({ ...filters, ...patch });
  const projectLabel =
    projects.find((project) => project.projectId === filters.projectId)?.name ??
    "All repos";
  const workspaceLabel =
    projects.length === 1
      ? (projects[0]?.name ?? "")
      : projects.length > 1
        ? `${projects.length} projects`
        : "";
  const statusLabel =
    STATUS_FILTERS.find((option) => option.value === filters.status)?.label ??
    "Open";

  return (
    <div className="w-page work-list-page">
      <div className="w-headrow">
        <div>
          <div className="w-eyebrow">
            Work{workspaceLabel ? ` · ${workspaceLabel}` : ""}
          </div>
          <h1 className="w-title">Ongoing work</h1>
        </div>
        <div className="w-headrow-spacer" />
        {projects.length > 0 ? (
          <Button
            className="work-new-ticket"
            title={
              tracker === "github"
                ? "Create a GitHub issue"
                : "Create a Linear ticket"
            }
            onClick={onNewTicket}
          >
            <IconPlus size={14} />
            New ticket
          </Button>
        ) : null}
      </div>
      {newTicketError ? (
        <div className="w-notice is-error work-new-ticket-error">
          <IconAlertTriangle size={14} />
          <span>{newTicketError}</span>
        </div>
      ) : null}

      <div className="w-toolbar">
        <label className="w-search work-search">
          <IconSearch size={14} />
          <input
            value={filters.search}
            placeholder="Search work"
            onChange={(event) => set({ search: event.target.value })}
            spellCheck={false}
          />
          {filters.search ? (
            <button
              type="button"
              className="w-search-clear"
              aria-label="Clear search"
              onClick={() => set({ search: "" })}
            >
              <IconX size={12} />
            </button>
          ) : null}
        </label>
        <Button
          size="icon"
          title="Refresh"
          onClick={onRefresh}
          disabled={loading}
        >
          {loading ? <Spinner /> : <IconRefresh size={15} />}
        </Button>
      </div>

      <div className="w-toggles work-filters">
        <Toggle
          className="filter-assigned-to-me"
          pressed={filters.assignedToMe}
          onPressedChange={(assignedToMe) => set({ assignedToMe })}
        >
          <Avatar name="Me" size={15} />
          Assigned to me
        </Toggle>
        {tracker === "linear" ? (
          <Toggle
            className="filter-linear"
            pressed={filters.linearIssues}
            onPressedChange={(linearIssues) => set({ linearIssues })}
          >
            <IconCircleDot size={13} className="c-linear" />
            Linear issues
          </Toggle>
        ) : (
          <Toggle
            className="filter-gh-issues"
            pressed={filters.githubIssues}
            onPressedChange={(githubIssues) => set({ githubIssues })}
          >
            <IconCircleDot size={13} className="c-open" />
            GitHub issues
          </Toggle>
        )}
        <Toggle
          className="filter-prs"
          pressed={filters.pullRequests}
          onPressedChange={(pullRequests) => set({ pullRequests })}
        >
          <IconGitPullRequest size={13} className="c-open" />
          PRs
        </Toggle>
        <Dropdown
          className="filter-status"
          trigger={(open, toggle) => (
            <button
              type="button"
              className={cx("w-toggle", "is-on", open && "is-focus")}
              onClick={toggle}
            >
              {statusLabel}
              <IconChevronDown size={12} />
            </button>
          )}
        >
          {(close) =>
            STATUS_FILTERS.map((option) => (
              <MenuItem
                key={option.value}
                checked={filters.status === option.value}
                onSelect={() => {
                  set({ status: option.value });
                  close();
                }}
              >
                {option.label}
              </MenuItem>
            ))
          }
        </Dropdown>
        <Dropdown
          className="filter-project"
          trigger={(open, toggle) => (
            <button
              type="button"
              className={cx(
                "w-toggle",
                filters.projectId && "is-on",
                open && "is-focus",
              )}
              onClick={toggle}
            >
              <IconFolder size={13} />
              {projectLabel}
              <IconChevronDown size={12} />
            </button>
          )}
        >
          {(close) => (
            <>
              <MenuItem
                checked={!filters.projectId}
                onSelect={() => {
                  set({ projectId: "" });
                  close();
                }}
              >
                All repos
              </MenuItem>
              {projects.map((project) => (
                <MenuItem
                  key={project.projectId}
                  checked={filters.projectId === project.projectId}
                  onSelect={() => {
                    set({ projectId: project.projectId });
                    close();
                  }}
                >
                  {project.name}
                  {project.repo ? (
                    <span className="w-menu-hint">{project.repo}</span>
                  ) : null}
                </MenuItem>
              ))}
            </>
          )}
        </Dropdown>
        <Dropdown
          className="filter-tracker-project"
          trigger={(open, toggle) => (
            <button
              type="button"
              className={cx(
                "w-toggle",
                filters.trackerProject && "is-on",
                open && "is-focus",
              )}
              onClick={toggle}
            >
              <IconBox
                size={13}
                className={tracker === "linear" ? "c-linear" : undefined}
              />
              {filters.trackerProject || "All projects"}
              <IconChevronDown size={12} />
            </button>
          )}
        >
          {(close) => (
            <>
              <MenuItem
                checked={!filters.trackerProject}
                onSelect={() => {
                  set({ trackerProject: "" });
                  close();
                }}
              >
                All projects
              </MenuItem>
              {trackerProjects.map((name) => (
                <MenuItem
                  key={name}
                  checked={filters.trackerProject === name}
                  onSelect={() => {
                    set({ trackerProject: name });
                    close();
                  }}
                >
                  {name}
                </MenuItem>
              ))}
            </>
          )}
        </Dropdown>
        <Toggle
          className="filter-in-sidebar"
          pressed={filters.inSidebar}
          onPressedChange={(inSidebar) => set({ inSidebar })}
        >
          <IconLayoutSidebar size={13} />
          In my sidebar
        </Toggle>
      </div>

      <Notices data={data} error={error} />
      <GithubProjectsNotice data={data} onDismiss={onDismissNotice} />

      <div className="w-list-meta">
        <span>
          {listSummary(
            data,
            visible.length,
            activeFilterCount(filters),
            loading,
            now,
          )}
        </span>
        <span className="w-spacer" />
        <span className="w-sort">
          <IconArrowsSort size={12} />
          Recently updated
        </span>
      </div>

      {!data && loading ? (
        <SkeletonRows />
      ) : visible.length === 0 ? (
        <EmptyList
          data={data}
          filters={filters}
          onShowAll={() => set({ assignedToMe: false })}
        />
      ) : (
        <div className="w-list work-list" role="list">
          {visible.map((item) => (
            <WorkRow key={item.key} item={item} now={now} onOpen={onOpen} />
          ))}
        </div>
      )}
    </div>
  );
}

function listSummary(
  data: WorkListData | null,
  count: number,
  filtersOn: number,
  loading: boolean,
  now: number,
): string {
  if (!data) return loading ? "Loading your work…" : "";
  const updated = data.refreshing
    ? "updating…"
    : `updated ${relativeTime(data.generatedAt, now) || "now"}`;
  const filtered =
    filtersOn > 0 ? ` · ${filtersOn} filter${filtersOn === 1 ? "" : "s"}` : "";
  return `${count} open${filtered} · ${updated}`;
}

function Notices({
  data,
  error,
}: {
  data: WorkListData | null;
  error: string | null;
}) {
  const notices: string[] = [];
  if (error) notices.push(error);
  if (data && !data.linearConfigured && data.tracker !== "github") {
    notices.push(
      'Add a Linear API key to see Linear tickets: run "ghostex work-mode linear-key" or set it in Settings.',
    );
  }
  if (data && !data.ghAvailable)
    notices.push(
      "Install and sign in to the GitHub CLI (gh) to see pull requests and issues.",
    );
  for (const message of data?.errors ?? []) notices.push(message);
  if (notices.length === 0) return null;
  return (
    <div className="w-notices">
      {notices.map((notice) => (
        <div key={notice} className="w-notice">
          <IconAlertTriangle size={14} />
          <span>{notice}</span>
        </div>
      ))}
    </div>
  );
}

/**
 * CDXC:WorkMode 2026-10-09 DECISION:
 * User: when `gh` cannot read GitHub Projects, "Ask user to run that command if needed with a closable notice on the page that appears once": it shows the command with a Copy button, and closing it is remembered (gxserver, `/api/dismissWorkNotice`).
 */
function GithubProjectsNotice({
  data,
  onDismiss,
}: {
  data: WorkListData | null;
  onDismiss: (notice: string) => void;
}) {
  const status = data?.githubProjects;
  if (
    data?.tracker !== "github" ||
    status?.access !== "missingScope" ||
    status.noticeDismissed
  )
    return null;
  return (
    <div className="w-notice work-github-projects-notice">
      <IconAlertTriangle size={14} />
      <span className="w-notice-text">
        To show GitHub Projects, run <code>{status.command}</code>
      </span>
      <button
        type="button"
        className="w-notice-action work-github-projects-copy"
        title="Copy the command"
        onClick={() =>
          void navigator.clipboard
            ?.writeText(status.command)
            .catch(() => undefined)
        }
      >
        <IconCopy size={13} />
        Copy
      </button>
      <button
        type="button"
        className="w-notice-close work-github-projects-close"
        aria-label="Close"
        title="Close"
        onClick={() => onDismiss("githubProjectsScope")}
      >
        <IconX size={13} />
      </button>
    </div>
  );
}

function EmptyList({
  data,
  filters,
  onShowAll,
}: {
  data: WorkListData | null;
  filters: WorkFilters;
  onShowAll: () => void;
}) {
  if (
    data &&
    data.items.length > 0 &&
    filters.assignedToMe &&
    !data.items.some((item) => item.assignedToMe)
  ) {
    return (
      <div className="w-empty">
        <p>Nothing is assigned to you right now.</p>
        <Button size="sm" onClick={onShowAll}>
          Show everyone's work
        </Button>
      </div>
    );
  }
  return (
    <div className="w-empty">
      <p>
        {data && data.items.length > 0
          ? "No work matches these filters."
          : "No open work in these projects."}
      </p>
    </div>
  );
}

function SkeletonRows() {
  return (
    <div className="w-list" aria-hidden>
      {Array.from({ length: 6 }, (_, index) => (
        <div key={index} className="w-row is-skeleton">
          <span className="w-skel w-skel--glyph" />
          <span
            className="w-skel"
            style={{ width: `${55 + ((index * 17) % 35)}%` }}
          />
          <span className="w-skel w-skel--time" />
          <span className="w-skel w-skel--line2" />
        </div>
      ))}
    </div>
  );
}

function WorkRow({
  item,
  now,
  onOpen,
}: {
  item: WorkItem;
  now: number;
  onOpen: (item: WorkItem) => void;
}) {
  const working = item.sessions.filter((session) => session.working);
  return (
    <div
      role="listitem"
      tabIndex={0}
      className={cx(
        "w-row",
        `work-row-${item.key.replace(/[^a-z0-9]+/giu, "-").toLowerCase()}`,
      )}
      onClick={() => onOpen(item)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onOpen(item);
        }
      }}
    >
      <span className="w-kind">
        <StatusGlyph item={item} />
      </span>
      <div className="w-l1">
        <span className="w-id">{item.id}</span>
        <span className="w-row-title">{item.title}</span>
      </div>
      <span className="w-time">{relativeTime(item.updatedAt, now)}</span>
      <div className="w-l2">
        {item.projectName ? (
          <span className="w-meta">
            <IconFolder size={12} />
            {item.projectName}
          </span>
        ) : item.kind === "linearIssue" ? (
          <span className="w-meta is-faint">
            <IconFolder size={12} />
            No repo yet
          </span>
        ) : null}
        {item.linearProject ? (
          <span className="w-meta">
            <IconBox size={12} className="c-linear" />
            {item.linearProject.name}
          </span>
        ) : null}
        {item.githubProject ? (
          <span className="w-meta">
            <IconBox size={12} />
            {item.githubProject.name}
          </span>
        ) : null}
        {item.assignee ? (
          <span className="w-meta">
            <Avatar
              name={item.assignee.name}
              url={item.assignee.avatarUrl}
              size={16}
            />
            {item.assignee.isMe ? "You" : item.assignee.name}
          </span>
        ) : null}
        {item.pullRequest && item.kind !== "pullRequest" ? (
          <PullRequestChip pullRequest={item.pullRequest} />
        ) : null}
        {item.kind === "pullRequest" && item.pullRequest?.checks ? (
          <span
            className={cx(
              "w-meta",
              item.pullRequest.checks === "failing" ? "c-closed" : "",
            )}
          >
            {item.pullRequest.checks === "passing"
              ? "checks pass"
              : item.pullRequest.checks === "failing"
                ? "checks failing"
                : "checks running"}
          </span>
        ) : null}
        {item.ticket ? <span className="w-chip">{item.ticket}</span> : null}
        {item.slackThreadCount ? (
          <span className="w-meta">
            <IconHash size={12} className="c-slack" />
            {item.slackThreadCount}
          </span>
        ) : null}
        {item.noTicket ? (
          <span className="w-needs">
            <IconAlertTriangle size={12} />
            No ticket
          </span>
        ) : null}
        <span className="w-spacer" />
        {item.sessions.length > 0 ? (
          <span className="w-mine">
            <IconLayoutSidebar size={12} />
            In sidebar
          </span>
        ) : null}
        {working.length > 0 ? (
          <LiveDot
            title={`${working[0]?.title ?? "A session"} is working right now`}
          />
        ) : null}
      </div>
    </div>
  );
}
