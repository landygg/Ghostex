//! Native GPUI New Coordinator dialog: name the coordinator, pick its agent, and optionally give it
//! a goal and a first request.
//!
//! CDXC:Coordinators 2026-09-30 WHY:
//! Claude's New project dialog asks for a name and an optional goal and then opens the conversation; Ghostex does the same inside a project, plus the agent (a coordinator runs on Claude, Codex, ZCode or Empryo, the agents Ghostex can hand its role) and an optional first request, so the coordinator can start planning the moment it opens instead of waiting for a second step.
//! SEE-ALSO: apps/desktop/src/app/new_coordinator_modal_lifecycle.rs (open, create), apps/desktop/src/app/gx_store/create/coordinator.rs (the gxserver calls), server/src/coordinators/ (what a coordinator is).
use super::native_modal_kit::*;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, px,
};
use gpui_component::input::{Enter, Escape, InputEvent, InputState, TextareaState};
use gpui_component::{h_flex, v_flex};
use std::rc::Rc;

pub(crate) const NEW_COORDINATOR_MODAL_WIDTH: f32 = 540.0;
pub(crate) const NEW_COORDINATOR_MODAL_INITIAL_HEIGHT: f32 = 640.0;

const TITLE: &str = "New Orchestrator";
const FIELD_NAME: &str = "Name";
const NAME_PLACEHOLDER: &str = "e.g. Checkout redesign";
const NAME_HINT: &str =
    "Shown in the sidebar so you can find this orchestrator later. It keeps this name.";
const FIELD_AGENT: &str = "Agent";
const FIELD_MODEL: &str = "Model";
const FIELD_EFFORT: &str = "Effort";
/// CDXC:Coordinators 2026-09-30 DECISION:
/// User (question 2, answer 2B): a coordinator runs at medium effort by default, and the user picks its model (and can change the effort) when creating it. Routing work does not need the deepest thinking, and a faster coordinator answers and reacts to reports sooner.
const DEFAULT_COORDINATOR_EFFORT: &str = "medium";
/// CDXC:Coordinators 2026-10-01 DECISION:
/// User: a coordinator on a Claude launcher starts on Opus 5.5 at medium effort ("my preferences should be the preferences for this feature for customers using the same setup, since that's what I tested"). `opus[1m]` is Opus 5.5's row in the Claude lineup; a Codex lineup has no such row and keeps its own default model.
/// SEE-ALSO: DEFAULT_CLAUDE_COORDINATOR_MODEL in server/src/ghostex_cli/coordinator/command.rs (`ghostex coordinator create` keeps the same default).
const DEFAULT_CLAUDE_COORDINATOR_MODEL: &str = "opus[1m]";
const FIELD_GOAL: &str = "Goal (optional)";
const GOAL_PLACEHOLDER: &str = "One line it works toward, e.g. Ship the new checkout by Friday";
const FIELD_REQUEST: &str = "First request (optional)";
const REQUEST_PLACEHOLDER: &str =
    "Describe the work. It plans it, starts a thread for each task, and reports back.";
const CANCEL: &str = "Cancel";
const CREATE: &str = "Create";

/// An agent a coordinator can run on (a Claude, Codex, ZCode or Empryo launcher), with its model lineup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewCoordinatorAgent {
    pub(crate) agent_id: String,
    pub(crate) name: String,
    pub(crate) models: Vec<NewCoordinatorModel>,
}

/// One model of the lineup, with the efforts it accepts as `(value, label)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewCoordinatorModel {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) efforts: Vec<(String, String)>,
    pub(crate) default: bool,
}

pub(crate) enum NewCoordinatorModalCommand {
    Create {
        agent_id: String,
        name: String,
        goal: String,
        first_request: String,
        model: Option<String>,
        effort: Option<String>,
    },
    Cancel,
}

pub(crate) type NewCoordinatorModalHost = Rc<dyn Fn(NewCoordinatorModalCommand, &mut App)>;

pub(crate) struct NewCoordinatorModalConfig {
    pub(crate) project_name: String,
    pub(crate) agents: Vec<NewCoordinatorAgent>,
    pub(crate) selected_agent: usize,
    pub(crate) palette: ModalPalette,
}

pub(crate) struct GpuiNewCoordinatorModalWindow {
    host: NewCoordinatorModalHost,
    palette: ModalPalette,
    project_name: String,
    agents: Vec<NewCoordinatorAgent>,
    selected_agent: usize,
    model_index: usize,
    effort: Option<String>,
    model_select: ModalSelect,
    effort_select: ModalSelect,
    name_input: Entity<InputState>,
    goal_input: Entity<InputState>,
    request_input: Entity<TextareaState>,
    name: String,
    goal: String,
    request: String,
    fit: ModalFit,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GpuiNewCoordinatorModalWindow {
    pub(crate) fn new(
        config: NewCoordinatorModalConfig,
        host: NewCoordinatorModalHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder(NAME_PLACEHOLDER));
        let goal_input = cx.new(|cx| InputState::new(window, cx).placeholder(GOAL_PLACEHOLDER));
        let request_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder(REQUEST_PLACEHOLDER));
        let subscriptions = vec![
            cx.subscribe_in(
                &name_input,
                window,
                |this: &mut Self, input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.name = input.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &goal_input,
                window,
                |this: &mut Self, input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.goal = input.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &request_input,
                window,
                |this: &mut Self, input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.request = input.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
        ];
        name_input.update(cx, |input, cx| input.focus(window, cx));
        let selected_agent = config
            .selected_agent
            .min(config.agents.len().saturating_sub(1));
        let mut this = Self {
            host,
            palette: config.palette,
            project_name: config.project_name,
            agents: config.agents,
            selected_agent,
            model_index: 0,
            effort: None,
            model_select: ModalSelect::new(),
            effort_select: ModalSelect::new(),
            name_input,
            goal_input,
            request_input,
            name: String::new(),
            goal: String::new(),
            request: String::new(),
            fit: ModalFit::fixed(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reset_model();
        this
    }

    fn agent(&self) -> Option<&NewCoordinatorAgent> {
        self.agents.get(self.selected_agent)
    }

    fn model(&self) -> Option<&NewCoordinatorModel> {
        self.agent()?.models.get(self.model_index)
    }

    fn efforts(&self) -> Vec<(String, String)> {
        self.model()
            .map(|model| model.efforts.clone())
            .unwrap_or_default()
    }

    /// Opus 5.5 on Claude, otherwise the agent's default model, and medium effort when that model
    /// takes it.
    fn reset_model(&mut self) {
        self.model_index = self
            .agent()
            .and_then(|agent| {
                agent
                    .models
                    .iter()
                    .position(|model| model.value == DEFAULT_CLAUDE_COORDINATOR_MODEL)
                    .or_else(|| agent.models.iter().position(|model| model.default))
            })
            .unwrap_or(0);
        self.reset_effort();
    }

    fn reset_effort(&mut self) {
        let efforts = self.efforts();
        let keep = self
            .effort
            .as_ref()
            .filter(|effort| efforts.iter().any(|(value, _)| value == *effort))
            .cloned();
        self.effort = keep.or_else(|| {
            efforts
                .iter()
                .find(|(value, _)| value == DEFAULT_COORDINATOR_EFFORT)
                .or_else(|| efforts.first())
                .map(|(value, _)| value.clone())
        });
    }

    fn toggle_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent().is_none_or(|agent| agent.models.is_empty()) {
            return;
        }
        self.effort_select.close();
        self.focus_handle.focus(window, cx);
        self.model_select.toggle(Some(self.model_index));
        cx.notify();
    }

    fn toggle_effort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let efforts = self.efforts();
        if efforts.is_empty() {
            return;
        }
        self.model_select.close();
        self.focus_handle.focus(window, cx);
        let selected = efforts
            .iter()
            .position(|(value, _)| Some(value) == self.effort.as_ref());
        self.effort_select.toggle(selected);
        cx.notify();
    }

    fn choose_model(&mut self, index: usize, cx: &mut Context<Self>) {
        self.model_index = index;
        self.model_select.close();
        self.reset_effort();
        cx.notify();
    }

    fn choose_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((value, _)) = self.efforts().get(index) {
            self.effort = Some(value.clone());
        }
        self.effort_select.close();
        cx.notify();
    }

    fn can_create(&self) -> bool {
        !self.agents.is_empty()
    }

    fn close_window_and_send(
        &mut self,
        command: NewCoordinatorModalCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.remove_window();
        (self.host)(command, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_window_and_send(NewCoordinatorModalCommand::Cancel, window, cx);
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agents.get(self.selected_agent).cloned() else {
            return;
        };
        let name = self.name.split_whitespace().collect::<Vec<_>>().join(" ");
        let command = NewCoordinatorModalCommand::Create {
            agent_id: agent.agent_id,
            name,
            goal: self.goal.trim().to_string(),
            first_request: self.request.trim().to_string(),
            model: self.model().map(|model| model.value.clone()),
            effort: self.effort.clone().filter(|_| !self.efforts().is_empty()),
        };
        self.close_window_and_send(command, window, cx);
    }

    /// Enter creates from the one-line fields; in the request it is a newline, and Cmd/Ctrl+Enter
    /// creates from anywhere.
    fn on_enter_action(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        let in_request = self
            .request_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        if in_request && !action.secondary {
            cx.propagate();
            return;
        }
        cx.stop_propagation();
        if self.can_create() {
            self.create(window, cx);
        }
    }

    fn on_escape_action(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.model_select.open || self.effort_select.open {
            self.model_select.close();
            self.effort_select.close();
            cx.notify();
            return;
        }
        self.cancel(window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let models = self.agent().map_or(0, |agent| agent.models.len());
        match self.model_select.handle_key(key, models) {
            ModalSelectKey::Consumed => {
                cx.notify();
                cx.stop_propagation();
                return;
            }
            ModalSelectKey::Choose(index) => {
                self.choose_model(index, cx);
                cx.stop_propagation();
                return;
            }
            ModalSelectKey::Ignored => {}
        }
        match self.effort_select.handle_key(key, self.efforts().len()) {
            ModalSelectKey::Consumed => {
                cx.notify();
                cx.stop_propagation();
                return;
            }
            ModalSelectKey::Choose(index) => {
                self.choose_effort(index, cx);
                cx.stop_propagation();
                return;
            }
            ModalSelectKey::Ignored => {}
        }
        match key {
            "escape" => self.cancel(window, cx),
            "enter" if !event.is_held && self.can_create() => self.create(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn field(&self, label: &'static str, control: AnyElement) -> AnyElement {
        v_flex()
            .w_full()
            .gap(px(8.0))
            .child(modal_section_title(&self.palette, label))
            .child(control)
            .into_any_element()
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let mut body = v_flex().w_full().flex_1().min_h_0().gap(px(16.0));
        body = body.child(
            v_flex()
                .w_full()
                .gap(px(8.0))
                .child(modal_section_title(&p, FIELD_NAME))
                .child(modal_text_input(&p, &self.name_input, false, window, cx))
                .child(modal_hint(&p, NAME_HINT)),
        );
        if self.agents.is_empty() {
            body = body.child(modal_error(
                &p,
                "An orchestrator runs on Claude, Codex, ZCode or Empryo. Add one of them in Settings > Agents first.",
            ));
        } else if self.agents.len() > 1 {
            let items = self
                .agents
                .iter()
                .map(|agent| ModalRailItem {
                    label: SharedString::from(agent.name.clone()),
                    trailing: None,
                })
                .collect::<Vec<_>>();
            let control = modal_bordered_segmented_control(
                p.foreground,
                p.hairline,
                p.muted,
                "new-coordinator-agent",
                &items,
                self.selected_agent,
                |this: &mut Self, index, _window, cx| {
                    this.selected_agent = index;
                    this.reset_model();
                    cx.notify();
                },
                cx,
            )
            .w_full()
            .into_any_element();
            body = body.child(self.field(FIELD_AGENT, control));
        }
        if let Some(row) = self.render_model_row(cx) {
            body = body.child(row);
        }
        body = body.child(self.field(
            FIELD_GOAL,
            modal_text_input(&p, &self.goal_input, false, window, cx),
        ));
        body.child(
            v_flex()
                .w_full()
                .flex_1()
                .min_h_0()
                .gap(px(8.0))
                .child(modal_section_title(&p, FIELD_REQUEST))
                .child(modal_text_area(
                    &p,
                    &self.request_input,
                    None,
                    false,
                    window,
                    cx,
                ))
                .child(modal_hint(
                    &p,
                    format!(
                        "Threads show up under the orchestrator in the sidebar. Press {} to create.",
                        crate::hotkey_label::terminal_overlay_hotkey_chord_label("cmd+enter")
                    ),
                )),
        )
        .into_any_element()
    }

    /// Model and effort side by side; the effort half is left out for a model without efforts.
    fn render_model_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let agent = self.agent()?;
        if agent.models.is_empty() {
            return None;
        }
        let p = self.palette;
        let model = modal_select_trigger(
            &p,
            &self.model_select,
            "new-coordinator-model-select",
            self.model().map(|model| model.label.clone()),
            "Choose a model",
            false,
            |this, window, cx| this.toggle_model_menu(window, cx),
            cx,
        );
        let efforts = self.efforts();
        let mut row = h_flex().w_full().gap(px(12.0)).child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.0))
                .child(modal_section_title(&p, FIELD_MODEL))
                .child(
                    h_flex()
                        .w_full()
                        .on_children_prepainted(capture_child_bounds(
                            self.model_select.trigger_bounds.clone(),
                            0,
                        ))
                        .child(model),
                ),
        );
        if !efforts.is_empty() {
            let label = efforts
                .iter()
                .find(|(value, _)| Some(value) == self.effort.as_ref())
                .map(|(_, label)| label.clone());
            let effort = modal_select_trigger(
                &p,
                &self.effort_select,
                "new-coordinator-effort-select",
                label,
                "Effort",
                false,
                |this, window, cx| this.toggle_effort_menu(window, cx),
                cx,
            );
            row = row.child(
                v_flex()
                    .w(px(170.0))
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .child(modal_section_title(&p, FIELD_EFFORT))
                    .child(
                        h_flex()
                            .w_full()
                            .on_children_prepainted(capture_child_bounds(
                                self.effort_select.trigger_bounds.clone(),
                                0,
                            ))
                            .child(effort),
                    ),
            );
        }
        Some(row.into_any_element())
    }

    /// The one open select's list, floated above the dialog.
    fn render_open_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette;
        if self.model_select.open {
            let labels = self
                .agent()
                .map(|agent| {
                    agent
                        .models
                        .iter()
                        .map(|model| model.label.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return modal_select_menu(
                &p,
                &self.model_select,
                "new-coordinator-model-menu",
                &labels,
                Some(self.model_index),
                |this: &mut Self, index, _window, cx| this.choose_model(index, cx),
                |this: &mut Self, _window, cx| {
                    this.model_select.close();
                    cx.notify();
                },
                window,
                cx,
            );
        }
        let efforts = self.efforts();
        let labels = efforts
            .iter()
            .map(|(_, label)| label.clone())
            .collect::<Vec<_>>();
        modal_select_menu(
            &p,
            &self.effort_select,
            "new-coordinator-effort-menu",
            &labels,
            efforts
                .iter()
                .position(|(value, _)| Some(value) == self.effort.as_ref()),
            |this: &mut Self, index, _window, cx| this.choose_effort(index, cx),
            |this: &mut Self, _window, cx| {
                this.effort_select.close();
                cx.notify();
            },
            window,
            cx,
        )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let cancel = modal_action_button(
            &p,
            "new-coordinator-cancel",
            CANCEL,
            None,
            ModalButtonTone::Neutral,
            false,
            |this, window, cx| this.cancel(window, cx),
            cx,
        );
        let create = modal_action_button(
            &p,
            "new-coordinator-create",
            CREATE,
            None,
            ModalButtonTone::Primary,
            !self.can_create(),
            |this, window, cx| {
                if this.can_create() {
                    this.create(window, cx)
                }
            },
            cx,
        );
        modal_footer(vec![cancel, create])
    }
}

impl Render for GpuiNewCoordinatorModalWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let description = format!(
            "One agent you talk to about {}. It plans the work, hands each task to a thread (its own agent session, optionally in its own worktree), and reports back when threads finish or need you.",
            self.project_name
        );
        let content = vec![
            modal_header(&p, TITLE, Some(description)),
            self.render_body(window, cx),
        ];
        let footer = self.render_footer(cx);
        let menu = self.render_open_menu(window, cx);
        modal_shell(
            &p,
            "ghostex-gpui-new-coordinator-modal",
            &self.focus_handle,
            &self.fit,
            Self::on_key_down,
            content,
            footer,
            menu,
            cx,
        )
        .capture_action(cx.listener(Self::on_enter_action))
        .capture_action(cx.listener(Self::on_escape_action))
    }
}

impl ModalCornerClose for GpuiNewCoordinatorModalWindow {
    fn close_from_corner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }
}

pub(crate) const MAKE_COORDINATOR_MODAL_INITIAL_HEIGHT: f32 = 330.0;

pub(crate) enum MakeCoordinatorModalCommand {
    Make { goal: String },
    Cancel,
}

pub(crate) type MakeCoordinatorModalHost = Rc<dyn Fn(MakeCoordinatorModalCommand, &mut App)>;

/// The Make Coordinator dialog: the session's Advanced > Make Coordinator. One optional field
/// (the goal) and a confirm, in the New Coordinator dialog's look.
pub(crate) struct GpuiMakeCoordinatorModalWindow {
    host: MakeCoordinatorModalHost,
    palette: ModalPalette,
    session_title: String,
    goal_input: Entity<InputState>,
    goal: String,
    fit: ModalFit,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GpuiMakeCoordinatorModalWindow {
    pub(crate) fn new(
        session_title: String,
        palette: ModalPalette,
        host: MakeCoordinatorModalHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let goal_input = cx.new(|cx| InputState::new(window, cx).placeholder(GOAL_PLACEHOLDER));
        let subscriptions = vec![cx.subscribe_in(
            &goal_input,
            window,
            |this: &mut Self, input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.goal = input.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )];
        goal_input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            host,
            palette,
            session_title,
            goal_input,
            goal: String::new(),
            fit: ModalFit::fixed(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn close_window_and_send(
        &mut self,
        command: MakeCoordinatorModalCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.remove_window();
        (self.host)(command, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_window_and_send(MakeCoordinatorModalCommand::Cancel, window, cx);
    }

    fn make(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let goal = self.goal.trim().to_string();
        self.close_window_and_send(MakeCoordinatorModalCommand::Make { goal }, window, cx);
    }

    fn on_enter_action(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.make(window, cx);
    }

    fn on_escape_action(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.cancel(window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.cancel(window, cx),
            "enter" if !event.is_held => self.make(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for GpuiMakeCoordinatorModalWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let description = format!(
            "\"{}\" keeps its conversation and keeps running: nothing restarts or interrupts it. It gets the crown now, and its orchestrator playbook arrives once its current turn is over.",
            self.session_title
        );
        let body = v_flex()
            .w_full()
            .gap(px(8.0))
            .child(modal_section_title(&p, FIELD_GOAL))
            .child(modal_text_input(&p, &self.goal_input, false, window, cx))
            .child(modal_hint(
                &p,
                "Sessions it started before are not its threads yet; ask it to adopt them (ghostex orchestrator link).",
            ))
            .into_any_element();
        let cancel = modal_action_button(
            &p,
            "make-coordinator-cancel",
            CANCEL,
            None,
            ModalButtonTone::Neutral,
            false,
            |this, window, cx| this.cancel(window, cx),
            cx,
        );
        let make = modal_action_button(
            &p,
            "make-coordinator-confirm",
            "Make Orchestrator",
            None,
            ModalButtonTone::Primary,
            false,
            |this, window, cx| this.make(window, cx),
            cx,
        );
        modal_shell(
            &p,
            "ghostex-gpui-make-coordinator-modal",
            &self.focus_handle,
            &self.fit,
            Self::on_key_down,
            vec![
                modal_header(&p, "Make Orchestrator", Some(description)),
                body,
            ],
            modal_footer(vec![cancel, make]),
            None,
            cx,
        )
        .capture_action(cx.listener(Self::on_enter_action))
        .capture_action(cx.listener(Self::on_escape_action))
    }
}

impl ModalCornerClose for GpuiMakeCoordinatorModalWindow {
    fn close_from_corner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }
}
