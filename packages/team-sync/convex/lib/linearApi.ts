/**
 * The Linear calls the Slack command flow makes: the team keys that tell a ticket ID from "UTF-8", a ticket's details, and creating a ticket. Each takes the key to call with (`linearKeys:forFlow`: the team's key, or the requester's own for creating their tickets).
 *
 * CDXC:TeamSync 2026-10-09 WHY:
 * The ticket is found or created in Convex, not on the requester's computer, because the flow must work while that computer is off: the requester is told "I've created SPX-1253 and its working thread" and the session starts when their Ghostex is back. `LINEAR_API_URL` points the calls at a mock for tests.
 */

export type LinearTeam = { id: string; key: string; name: string };

/** A Linear project (a release or initiative) and the teams whose tickets it may hold. */
export type LinearProject = { id: string; name: string; url: string; teamIds: string[] };

export type LinearIssue = {
  identifier: string;
  title: string;
  url: string;
  branchName: string | null;
  description: string | null;
  teamKey: string | null;
  teamName: string | null;
  projectName: string | null;
  assignee: string | null;
  state: string | null;
  labels: string[];
  comments: { author: string | null; body: string; createdAt: string }[];
  attachments: { title: string | null; url: string }[];
};

async function linearGraphql(key: string, query: string, variables: Record<string, unknown>): Promise<any> {
  // Personal API keys go in as they are; OAuth tokens need the Bearer scheme.
  const authorization = key.startsWith("lin_oauth_") ? `Bearer ${key}` : key;
  const response = await fetch(process.env.LINEAR_API_URL ?? "https://api.linear.app/graphql", {
    method: "POST",
    headers: { authorization, "content-type": "application/json" },
    body: JSON.stringify({ query, variables }),
  });
  const body = (await response.json().catch(() => null)) as { data?: any; errors?: { message?: string }[] } | null;
  if (!response.ok || !body || body.errors?.length) {
    throw new Error(`Linear: ${body?.errors?.[0]?.message ?? `HTTP ${response.status}`}`);
  }
  return body.data;
}

export async function linearTeams(key: string): Promise<LinearTeam[]> {
  const data = await linearGraphql(key, "query { teams(first: 250) { nodes { id key name } } }", {});
  return (data?.teams?.nodes ?? []) as LinearTeam[];
}

const ISSUE_FIELDS = `identifier title url branchName description
  team { key name } project { name } assignee { name } state { name }
  labels(first: 20) { nodes { name } }
  comments(first: 50) { nodes { body createdAt user { name } } }
  attachments(first: 20) { nodes { title url } }`;

function parseIssue(issue: any): LinearIssue | null {
  if (!issue?.identifier) return null;
  return {
    identifier: issue.identifier,
    title: issue.title ?? issue.identifier,
    url: issue.url ?? "",
    branchName: issue.branchName ?? null,
    description: issue.description ?? null,
    teamKey: issue.team?.key ?? null,
    teamName: issue.team?.name ?? null,
    projectName: issue.project?.name ?? null,
    assignee: issue.assignee?.name ?? null,
    state: issue.state?.name ?? null,
    labels: (issue.labels?.nodes ?? []).map((label: any) => String(label.name)),
    comments: (issue.comments?.nodes ?? []).map((comment: any) => ({
      author: comment.user?.name ?? null,
      body: String(comment.body ?? ""),
      createdAt: String(comment.createdAt ?? ""),
    })),
    attachments: (issue.attachments?.nodes ?? []).map((attachment: any) => ({
      title: attachment.title ?? null,
      url: String(attachment.url ?? ""),
    })),
  };
}

export async function linearIssue(key: string, identifier: string): Promise<LinearIssue | null> {
  const data = await linearGraphql(key, `query($id: String!) { issue(id: $id) { ${ISSUE_FIELDS} } }`, { id: identifier }).catch(
    (error: Error) => {
      // Linear answers an unknown identifier with an "Entity not found" error.
      if (/not found/i.test(error.message)) return null;
      throw error;
    },
  );
  return parseIssue(data?.issue);
}

const PROJECT_FIELDS = "id name url teams(first: 50) { nodes { id } }";

function parseProject(project: any): LinearProject | null {
  if (!project?.id) return null;
  return {
    id: String(project.id),
    name: String(project.name ?? ""),
    url: String(project.url ?? ""),
    teamIds: (project.teams?.nodes ?? []).map((team: any) => String(team.id)),
  };
}

/**
 * The Linear project a channel mapping or a thread names: a project link (`linear.app/<workspace>/project/<name>-<slug id>`), its id, or its name.
 */
export async function findLinearProject(key: string, ref: string): Promise<LinearProject | null> {
  const text = ref.trim();
  if (!text) return null;
  const link = /linear\.app\/[^/\s>|]+\/project\/([A-Za-z0-9-]+)/.exec(text);
  if (link) {
    // The link's last `-` part is the project's slug id; the words before it are its name and may change.
    const slugId = link[1].split("-").pop() ?? link[1];
    const data = await linearGraphql(key, `query($slug: String!) { projects(first: 1, filter: { slugId: { eq: $slug } }) { nodes { ${PROJECT_FIELDS} } } }`, {
      slug: slugId,
    });
    return parseProject(data?.projects?.nodes?.[0]);
  }
  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(text)) {
    const data = await linearGraphql(key, `query($id: String!) { project(id: $id) { ${PROJECT_FIELDS} } }`, { id: text }).catch((error: Error) => {
      if (/not found/i.test(error.message)) return null;
      throw error;
    });
    return parseProject(data?.project);
  }
  const data = await linearGraphql(key, `query($name: String!) { projects(first: 1, filter: { name: { eqIgnoreCase: $name } }) { nodes { ${PROJECT_FIELDS} } } }`, {
    name: text,
  });
  return parseProject(data?.projects?.nodes?.[0]);
}

export async function createLinearIssue(key: string, input: {
  teamId: string;
  title: string;
  description: string;
  assigneeId?: string;
  projectId?: string;
}): Promise<LinearIssue> {
  const data = await linearGraphql(
    key,
    `mutation($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { ${ISSUE_FIELDS} } } }`,
    { input },
  );
  const issue = data?.issueCreate?.success ? parseIssue(data.issueCreate.issue) : null;
  if (!issue) throw new Error("Linear did not create the ticket.");
  return issue;
}
