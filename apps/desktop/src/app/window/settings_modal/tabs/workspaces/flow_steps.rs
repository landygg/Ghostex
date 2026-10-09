//! A Work workspace's team-flow steps: the steps the Work page draws as a tracker on each ticket
//! (server/src/work_mode/team_flow.rs). Reorder, rename, remove, add a step with one of the rules
//! the daemon knows, then Save; Reset to default drops the workspace's own steps. Reads
//! `/api/readTeamFlow` and writes `/api/updateTeamFlow`, both scoped to the workspace. In a
//! workspace connected to a team these are the team's steps, kept in its Convex project.
//!
//! CDXC:WorkMode 2026-10-09 DECISION:
//! User: the team-flow steps are editable in settings, with the user's own team flow as the
//! default.
//!
//! CDXC:TeamSync 2026-10-09 DECISION:
//! User: team flow settings are "Owners only". A team's steps are read-only for members
//! (`canEdit` from the daemon), with one line saying only owners can change them.
use super::super::super::catalog::SettingOption;
use super::super::super::fields::{
    ButtonVariant, RowSpec, setting_row, settings_button, settings_icon_button, settings_select,
};
use super::super::super::store::store_gxserver_rpc;
use super::*;

/// The daemon's own limits (`validate_team_flow_steps`).
const MAX_STEPS: usize = 20;
const MAX_LABEL_CHARS: usize = 40;

/// One workspace's steps as edited here.
#[derive(Default)]
pub(crate) struct FlowStepsState {
    loading: bool,
    loaded: bool,
    error: Option<String>,
    /// `{ id, label, rule }`, in order.
    steps: Vec<Value>,
    /// Where the saved steps come from: `team`, `workspace`, `default`, `builtIn` or `teamError`.
    source: String,
    /// The workspace is connected to a team, so these are the team's steps.
    team: bool,
    /// False for a team member who is not an owner.
    can_edit: bool,
    /// `(kind, description, needsData)`.
    rules: Vec<(String, String, bool)>,
    dirty: bool,
    saving: bool,
    /// The rule chosen for the next added step.
    new_rule: String,
    next_id: usize,
}

impl FlowStepsState {
    fn rule_description(&self, kind: &str) -> String {
        self.rules
            .iter()
            .find(|(known, _, _)| known == kind)
            .map(|(_, description, _)| description.clone())
            .unwrap_or_else(|| kind.to_string())
    }

    fn needs_data(&self, kind: &str) -> bool {
        self.rules
            .iter()
            .any(|(known, _, needs)| known == kind && *needs)
    }

    fn apply(&mut self, result: &Value) {
        self.steps = result
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.source = text(result, "source");
        self.team = result.get("team").and_then(Value::as_bool) == Some(true);
        self.can_edit = result.get("canEdit").and_then(Value::as_bool) != Some(false);
        self.rules = result
            .get("rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|rule| {
                (
                    text(rule, "kind"),
                    text(rule, "description"),
                    rule.get("needsData").and_then(Value::as_bool) == Some(true),
                )
            })
            .collect();
        if self.new_rule.is_empty() {
            self.new_rule = self
                .rules
                .first()
                .map(|(kind, _, _)| kind.clone())
                .unwrap_or_default();
        }
        self.dirty = false;
        self.loaded = true;
        self.error = None;
    }
}

/// The extra value a rule needs: the PR label to look for, or the ticket states that count.
fn rule_param(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "pullRequestLabel" => Some(("PR label", "READY-FOR-QC")),
        "ticketState" => Some(("Ticket states", "Done, Released")),
        _ => None,
    }
}

impl WorkspacesTab {
    fn ensure_flow_steps_loaded(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        let state = self.flow_steps.entry(workspace_id.to_string()).or_default();
        if state.loaded || state.loading || !self.rpc_available(cx) {
            return;
        }
        self.flow_steps_call(workspace_id.to_string(), "/api/readTeamFlow", json!({}), cx);
    }

    /// A read or a write of the workspace's steps; the answer replaces what is shown.
    fn flow_steps_call(
        &mut self,
        workspace_id: String,
        path: &'static str,
        extra: Value,
        cx: &mut Context<Self>,
    ) {
        let mut params = json!({ "workspaceId": workspace_id });
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        let writing = path != "/api/readTeamFlow";
        let state = self.flow_steps.entry(workspace_id.clone()).or_default();
        if writing {
            state.saving = true;
        } else {
            state.loading = true;
        }
        cx.notify();
        let this = cx.weak_entity();
        store_gxserver_rpc(
            &self.store.clone(),
            path,
            params,
            RPC_TIMEOUT,
            move |result, cx| {
                let _ = this.update(cx, |page, cx| {
                    let state = page.flow_steps.entry(workspace_id).or_default();
                    state.loading = false;
                    state.saving = false;
                    match result {
                        Ok(result) => state.apply(&result),
                        Err(error) if writing => {
                            page.toast("error", "Couldn't save the team-flow steps", &error, cx)
                        }
                        Err(error) => state.error = Some(error),
                    }
                    cx.notify();
                });
            },
            cx,
        );
    }

    fn edit_flow_steps(
        &mut self,
        workspace_id: &str,
        edit: impl FnOnce(&mut FlowStepsState),
        cx: &mut Context<Self>,
    ) {
        let state = self.flow_steps.entry(workspace_id.to_string()).or_default();
        edit(state);
        state.dirty = true;
        cx.notify();
    }

    fn move_flow_step(
        &mut self,
        workspace_id: &str,
        index: usize,
        up: bool,
        cx: &mut Context<Self>,
    ) {
        self.edit_flow_steps(
            workspace_id,
            |state| {
                let target = if up {
                    index.checked_sub(1)
                } else {
                    Some(index + 1)
                };
                if let Some(target) = target.filter(|target| *target < state.steps.len()) {
                    state.steps.swap(index, target);
                }
            },
            cx,
        );
    }

    fn add_flow_step(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        let label_id = SharedString::from(format!("flow-step-new-label-{workspace_id}"));
        let param_id = SharedString::from(format!("flow-step-new-param-{workspace_id}"));
        let label = self.draft(&label_id).trim().to_string();
        let param = self.draft(&param_id).trim().to_string();
        let Some(state) = self.flow_steps.get(&workspace_id) else {
            return;
        };
        let kind = state.new_rule.clone();
        let label = if label.is_empty() {
            state.rule_description(&kind)
        } else {
            label
        };
        let mut rule = json!({ "kind": kind });
        match kind.as_str() {
            "pullRequestLabel" => rule["label"] = json!(param),
            "ticketState" => {
                rule["states"] = json!(
                    param
                        .split(',')
                        .map(str::trim)
                        .filter(|state| !state.is_empty())
                        .collect::<Vec<_>>()
                )
            }
            _ => {}
        }
        self.drafts.remove(&label_id);
        self.drafts.remove(&param_id);
        self.edit_flow_steps(
            &workspace_id,
            |state| {
                state.next_id += 1;
                let label: String = label.chars().take(MAX_LABEL_CHARS).collect();
                state.steps.push(json!({
                    "id": format!("added-{}", state.next_id),
                    "label": label,
                    "rule": rule,
                }));
            },
            cx,
        );
    }

    /// The Team-flow steps section of a Work workspace.
    pub(super) fn flow_steps_section(
        &mut self,
        p: &SettingsPalette,
        workspace_id: &str,
        workspace_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.ensure_flow_steps_loaded(workspace_id, cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let (loaded, error, steps, dirty, saving, source, team, can_edit) = {
            let state = self.flow_steps.entry(workspace_id.to_string()).or_default();
            (
                state.loaded,
                state.error.clone(),
                state.steps.clone(),
                state.dirty,
                state.saving,
                state.source.clone(),
                state.team,
                state.can_edit,
            )
        };
        if !loaded {
            rows.push(setting_row(
                p,
                format!("flow-steps-loading-{workspace_id}"),
                RowSpec::new("Team-flow steps")
                    .readout(if error.is_some() {
                        "Couldn't read them"
                    } else {
                        "Reading…"
                    })
                    .description(match error {
                        Some(error) => format!("Couldn't read the steps: {error}"),
                        None => "Reading the steps…".to_string(),
                    }),
                None,
                div().into_any_element(),
                cx,
            ));
        } else if !can_edit {
            rows.push(super::slack_flow::owners_only_row(
                p,
                format!("flow-steps-owners-{workspace_id}"),
                cx,
            ));
            for (index, step) in steps.iter().enumerate() {
                rows.push(self.read_only_flow_step_row(p, workspace_id, index, step, cx));
            }
        } else {
            let count = steps.len();
            for (index, step) in steps.iter().enumerate() {
                rows.push(self.flow_step_row(p, workspace_id, index, count, step, window, cx));
            }
            rows.push(self.add_flow_step_row(p, workspace_id, count, window, cx));
            rows.push(self.flow_steps_footer(p, workspace_id, &source, team, dirty, saving, cx));
        }
        let title = if workspace_name.is_empty() {
            "Team-flow steps".to_string()
        } else {
            format!("{workspace_name} · Team-flow steps")
        };
        settings_section(
            p,
            title,
            Some(SharedString::from(if team {
                "The steps each ticket shows on the Work page, in order, shared by your whole team. A step whose data Ghostex can't see yet shows as unknown, never as done."
            } else {
                "The steps each ticket shows on the Work page, in order. A step whose data Ghostex can't see yet shows as unknown, never as done."
            })),
            None,
            rows,
        )
        .map(IntoElement::into_any_element)
    }

    /// The rule a step uses, with the label or states it needs.
    fn flow_step_detail(&self, workspace_id: &str, rule: &Value) -> String {
        let kind = text(rule, "kind");
        let mut detail = self
            .flow_steps
            .get(workspace_id)
            .map(|state| state.rule_description(&kind))
            .unwrap_or_else(|| kind.clone());
        match kind.as_str() {
            "pullRequestLabel" => detail.push_str(&format!(": {}", text(rule, "label"))),
            "ticketState" => {
                let states: Vec<&str> = rule
                    .get("states")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                detail.push_str(&format!(": {}", states.join(", ")));
            }
            _ => {}
        }
        detail
    }

    /// One of the team's steps as a member sees it: its name and its rule, nothing to change.
    fn read_only_flow_step_row(
        &mut self,
        p: &SettingsPalette,
        workspace_id: &str,
        index: usize,
        step: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rule = step.get("rule").cloned().unwrap_or_default();
        let detail = self.flow_step_detail(workspace_id, &rule);
        setting_row(
            p,
            format!("flow-step-read-{workspace_id}-{}", text(step, "id")),
            RowSpec::new(format!("{}. {}", index + 1, text(step, "label")))
                .readout(detail.clone())
                .description(format!("Done when this rule holds: {detail}.")),
            None,
            div().into_any_element(),
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn flow_step_row(
        &mut self,
        p: &SettingsPalette,
        workspace_id: &str,
        index: usize,
        count: usize,
        step: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let step_id = text(step, "id");
        let label = text(step, "label");
        let rule = step.get("rule").cloned().unwrap_or_default();
        let kind = text(&rule, "kind");
        let needs_data = self
            .flow_steps
            .get(workspace_id)
            .is_some_and(|state| state.needs_data(&kind));
        let detail = self.flow_step_detail(workspace_id, &rule);
        let note = if needs_data {
            "Done when this rule holds. Shows as unknown until that data reaches Ghostex from Slack."
        } else {
            "Done when this rule holds. The field is the step's name on the Work page."
        };
        let input_id = SharedString::from(format!("flow-step-label-{workspace_id}-{step_id}"));
        let rename_workspace = workspace_id.to_string();
        let rename_step = step_id.clone();
        let input =
            FieldStates::text_state(
                self,
                &input_id,
                &label,
                Some("Step name"),
                move |page: &mut Self, text, _window, cx| {
                    let current = page
                        .flow_steps
                        .get(&rename_workspace)
                        .and_then(|state| {
                            state.steps.iter().find(|step| {
                                step.get("id").and_then(Value::as_str) == Some(&rename_step)
                            })
                        })
                        .and_then(|step| step.get("label"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if current.as_deref() == Some(text.as_str()) {
                        return;
                    }
                    let step_id = rename_step.clone();
                    page.edit_flow_steps(
                        &rename_workspace,
                        |state| {
                            if let Some(step) = state.steps.iter_mut().find(|step| {
                                step.get("id").and_then(Value::as_str) == Some(&step_id)
                            }) {
                                step["label"] = json!(text);
                            }
                        },
                        cx,
                    );
                },
                window,
                cx,
            );
        let up_workspace = workspace_id.to_string();
        let down_workspace = workspace_id.to_string();
        let remove_workspace = workspace_id.to_string();
        let control = h_flex()
            .gap(px(4.0))
            .child(div().mr(px(4.0)).child(settings_text_input(
                p,
                &input,
                Some(180.0),
                false,
                window,
                cx,
            )))
            .child(settings_icon_button(
                p,
                SharedString::from(format!("flow-step-up-{workspace_id}-{step_id}")),
                "modals/settings/arrow-up.svg",
                16.0,
                28.0,
                ButtonVariant::Ghost,
                Some("Move up".into()),
                index == 0,
                move |page: &mut Self, _window, cx| {
                    page.move_flow_step(&up_workspace, index, true, cx)
                },
                cx,
            ))
            .child(settings_icon_button(
                p,
                SharedString::from(format!("flow-step-down-{workspace_id}-{step_id}")),
                "modals/settings/arrow-down.svg",
                16.0,
                28.0,
                ButtonVariant::Ghost,
                Some("Move down".into()),
                index + 1 >= count,
                move |page: &mut Self, _window, cx| {
                    page.move_flow_step(&down_workspace, index, false, cx)
                },
                cx,
            ))
            .child(settings_icon_button(
                p,
                SharedString::from(format!("flow-step-remove-{workspace_id}-{step_id}")),
                "modals/settings/trash.svg",
                16.0,
                28.0,
                ButtonVariant::Ghost,
                Some("Remove".into()),
                count <= 1,
                move |page: &mut Self, _window, cx| {
                    page.edit_flow_steps(
                        &remove_workspace,
                        |state| {
                            if index < state.steps.len() {
                                state.steps.remove(index);
                            }
                        },
                        cx,
                    )
                },
                cx,
            ));
        setting_row(
            p,
            input_id,
            RowSpec::new(format!("{}. {detail}", index + 1)).description(note),
            None,
            control.into_any_element(),
            cx,
        )
    }

    fn add_flow_step_row(
        &mut self,
        p: &SettingsPalette,
        workspace_id: &str,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (options, new_rule) = {
            let state = self.flow_steps.entry(workspace_id.to_string()).or_default();
            (
                state
                    .rules
                    .iter()
                    .map(|(kind, description, _)| SettingOption {
                        label: description.clone(),
                        value: kind.clone(),
                    })
                    .collect::<Vec<_>>(),
                state.new_rule.clone(),
            )
        };
        let rule_workspace = workspace_id.to_string();
        let rule_select = settings_select(
            self,
            p,
            format!("flow-step-new-rule-{workspace_id}"),
            &options,
            &new_rule,
            Some(260.0),
            false,
            None,
            move |page: &mut Self, value, _window, cx| {
                page.flow_steps
                    .entry(rule_workspace.clone())
                    .or_default()
                    .new_rule = value;
                cx.notify();
            },
            window,
            cx,
        );
        let label_id = SharedString::from(format!("flow-step-new-label-{workspace_id}"));
        let param_id = SharedString::from(format!("flow-step-new-param-{workspace_id}"));
        let label_input = self.draft_input(&label_id, "", "Step name", window, cx);
        let param = rule_param(&new_rule);
        let param_input =
            param.map(|(_, example)| self.draft_input(&param_id, "", example, window, cx));
        let param_missing = param.is_some() && self.draft(&param_id).trim().is_empty();
        let full = count >= MAX_STEPS;
        let add_workspace = workspace_id.to_string();
        let control = h_flex()
            .w_full()
            .gap(px(8.0))
            .child(rule_select)
            .child(settings_text_input(
                p,
                &label_input,
                Some(150.0),
                false,
                window,
                cx,
            ))
            .when_some(param_input, |row, input| {
                row.child(settings_text_input(p, &input, None, false, window, cx))
            })
            .child(settings_button(
                p,
                SharedString::from(format!("flow-step-add-{workspace_id}")),
                "Add step",
                Some("modals/settings/plus.svg"),
                ButtonVariant::Outline,
                full || param_missing || new_rule.is_empty(),
                full.then(|| {
                    SharedString::from(format!("A team flow has at most {MAX_STEPS} steps."))
                }),
                move |page: &mut Self, _window, cx| page.add_flow_step(add_workspace.clone(), cx),
                cx,
            ));
        setting_row(
            p,
            format!("flow-step-add-row-{workspace_id}"),
            RowSpec::new("Add a step")
                .description(match param {
                    Some((what, _)) => format!(
                        "Pick the rule that says the step is done. This rule needs the {}; separate several with commas.",
                        what.to_lowercase()
                    ),
                    None => "Pick the rule that says the step is done, and name the step.".to_string(),
                })
                .wide(),
            None,
            control.into_any_element(),
            cx,
        )
    }

    fn flow_steps_footer(
        &mut self,
        p: &SettingsPalette,
        workspace_id: &str,
        source: &str,
        team: bool,
        dirty: bool,
        saving: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let readout = if dirty {
            "Unsaved changes"
        } else {
            match source {
                "team" => "Your team's steps",
                "workspace" => "This workspace's own steps",
                "default" => "Default steps",
                _ => "Built-in default",
            }
        };
        let status = if dirty {
            "Not saved yet."
        } else {
            match source {
                "team" => "Your team uses its own steps, shared by every teammate.",
                "workspace" => "This workspace uses its own steps.",
                "default" => "This workspace uses the default steps saved on this computer.",
                _ if team => {
                    "Your team uses the default team flow. Saving makes these steps the whole team's."
                }
                _ => "This workspace uses the default team flow.",
            }
        };
        let save_workspace = workspace_id.to_string();
        let discard_workspace = workspace_id.to_string();
        let reset_workspace = workspace_id.to_string();
        let control = h_flex()
            .gap(px(8.0))
            .when(dirty, |row| {
                row.child(settings_button(
                    p,
                    SharedString::from(format!("flow-steps-discard-{workspace_id}")),
                    "Discard",
                    None,
                    ButtonVariant::Ghost,
                    saving,
                    None,
                    move |page: &mut Self, _window, cx| {
                        page.flow_steps_call(
                            discard_workspace.clone(),
                            "/api/readTeamFlow",
                            json!({}),
                            cx,
                        )
                    },
                    cx,
                ))
            })
            .when(
                (source == "workspace" || source == "team") && !dirty,
                |row| {
                    row.child(settings_button(
                        p,
                        SharedString::from(format!("flow-steps-reset-{workspace_id}")),
                        "Reset to default",
                        None,
                        ButtonVariant::Outline,
                        saving,
                        None,
                        move |page: &mut Self, _window, cx| {
                            page.flow_steps_call(
                                reset_workspace.clone(),
                                "/api/updateTeamFlow",
                                json!({ "reset": true }),
                                cx,
                            )
                        },
                        cx,
                    ))
                },
            )
            .child(settings_button(
                p,
                SharedString::from(format!("flow-steps-save-{workspace_id}")),
                if saving { "Saving…" } else { "Save steps" },
                None,
                ButtonVariant::Outline,
                saving || !dirty,
                None,
                move |page: &mut Self, _window, cx| {
                    let steps = page
                        .flow_steps
                        .get(&save_workspace)
                        .map(|state| Value::Array(state.steps.clone()))
                        .unwrap_or_default();
                    page.flow_steps_call(
                        save_workspace.clone(),
                        "/api/updateTeamFlow",
                        json!({ "steps": steps }),
                        cx,
                    );
                },
                cx,
            ));
        setting_row(
            p,
            format!("flow-steps-save-row-{workspace_id}"),
            RowSpec::new("Save").readout(readout).description(status),
            None,
            control.into_any_element(),
            cx,
        )
    }
}
