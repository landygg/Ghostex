//! The work-mode Link to picker: `Link "<session>" to a Linear issue`, a search line, gxserver's
//! suggestions, and Enter to link. Drawn in Quick Access's language (search line, rows, footer)
//! because it is the same kind of pick-one-from-a-list window.
//!
//! CDXC:WorkMode 2026-10-09 DECISION:
//! User: the Link to picker has a search box with focus, a list of suggestions, Enter links and Escape closes; Linear issues are multi-select (a session can link several issues shipped in one PR), the other kinds pick one. The footer says what Enter does.
//! SEE-ALSO: apps/desktop/src/app/work_link_picker_modal_lifecycle.rs (open, link, close),
//! server/src/work_mode/candidates.rs (`/api/listWorkLinkCandidates`, the suggestions),
//! packages/gx-core/src/sidebar_menu/link_menu.rs (the submenu that opens it).
use super::quick_access::chrome::{
    quick_access_keycap, quick_access_search_bar, quick_access_tooltip,
};
use super::quick_access::palette::{
    QUICK_ACCESS_FOOTER_HEIGHT, QUICK_ACCESS_GROUP_HEADING_HEIGHT, QUICK_ACCESS_ITEM_FONT_SIZE,
    QUICK_ACCESS_LIST_PADDING, QUICK_ACCESS_META_FONT_SIZE, QUICK_ACCESS_ROW_FONT_SIZE,
    QUICK_ACCESS_ROW_HEIGHT, QUICK_ACCESS_ROW_PADDING_X, QUICK_ACCESS_ROW_RADIUS,
    QuickAccessPalette, hsla,
};
use crate::app::gx_store::gx_rpc;
use crate::app::window::native_modal_kit::{MODAL_MONO_FONT, MODAL_UI_FONT, ModalCornerClose};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{h_flex, v_flex};
use serde_json::{Value, json};
use std::rc::Rc;
use std::time::Duration;

pub(crate) const WORK_LINK_PICKER_MODAL_WIDTH: f32 = 600.0;
pub(crate) const WORK_LINK_PICKER_MODAL_INITIAL_HEIGHT: f32 = 460.0;
/// Typing settles before gxserver is asked again (it runs `gh` or calls Linear).
const QUERY_DEBOUNCE: Duration = Duration::from_millis(250);
const HEADER_HEIGHT: f32 = 40.0;

/// What the session is being linked to, from the submenu row that opened the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkLinkKind {
    PullRequest,
    LinearIssue,
    LinearProject,
    GithubIssue,
    GithubProject,
}

impl WorkLinkKind {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "pullRequest" => Some(Self::PullRequest),
            "linearIssue" => Some(Self::LinearIssue),
            "linearProject" => Some(Self::LinearProject),
            "githubIssue" => Some(Self::GithubIssue),
            "githubProject" => Some(Self::GithubProject),
            _ => None,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::PullRequest => "pullRequest",
            Self::LinearIssue => "linearIssue",
            Self::LinearProject => "linearProject",
            Self::GithubIssue => "githubIssue",
            Self::GithubProject => "githubProject",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::PullRequest => "a pull request",
            Self::LinearIssue => "a Linear issue",
            Self::LinearProject => "a Linear project",
            Self::GithubIssue => "a GitHub issue",
            Self::GithubProject => "a GitHub project",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::PullRequest => "Search open pull requests…",
            Self::LinearIssue => "Search Linear issues, or type an ID like SPX-1245…",
            Self::LinearProject => "Search Linear projects…",
            Self::GithubIssue => "Search open GitHub issues…",
            Self::GithubProject => "Search GitHub projects…",
        }
    }

    fn multi_select(self) -> bool {
        self == Self::LinearIssue
    }

    /// The `/api/setSessionWorkLinks` fields that link `values`.
    pub(crate) fn links(self, values: &[String]) -> Value {
        match self {
            Self::PullRequest => json!({ "pullRequest": values.first() }),
            Self::LinearIssue => json!({ "linearIssues": values }),
            Self::LinearProject => json!({ "linearProject": values.first() }),
            Self::GithubIssue => {
                json!({ "githubIssues": values.first().into_iter().collect::<Vec<_>>() })
            }
            Self::GithubProject => json!({ "githubProject": values.first() }),
        }
    }
}

pub(crate) struct WorkLinkPickerConfig {
    /// The gxserver project and session ids (not the sidebar row id).
    pub(crate) project_id: String,
    pub(crate) session_id: String,
    pub(crate) session_title: String,
    pub(crate) kind: WorkLinkKind,
}

pub(crate) enum WorkLinkPickerCommand {
    Link { links: Value },
    Close,
}

pub(crate) type WorkLinkPickerHost = Rc<dyn Fn(WorkLinkPickerCommand, &mut App)>;

#[derive(Clone, Debug)]
struct Candidate {
    value: String,
    label: String,
    title: String,
    detail: Option<String>,
    url: Option<String>,
    own_repo: bool,
    linked: bool,
}

impl Candidate {
    fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            value: text("value")?,
            label: text("label").unwrap_or_default(),
            title: text("title").unwrap_or_default(),
            detail: text("detail"),
            url: text("url"),
            own_repo: value.get("ownRepo").and_then(Value::as_bool) == Some(true),
            linked: value.get("linked").and_then(Value::as_bool) == Some(true),
        })
    }
}

pub(crate) struct GpuiWorkLinkPickerModalWindow {
    host: WorkLinkPickerHost,
    config: WorkLinkPickerConfig,
    search: Entity<InputState>,
    query: String,
    rows: Vec<Candidate>,
    /// Multi-select: the values that will be linked. Starts as what the session links now.
    ticked: Vec<String>,
    /// The first answer seeded `ticked`; later answers keep the user's ticks.
    seeded: bool,
    notice: Option<String>,
    loading: bool,
    error: Option<String>,
    selected: usize,
    request_generation: u64,
    scroll: ScrollHandle,
    pending_scroll: Option<usize>,
    glass: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GpuiWorkLinkPickerModalWindow {
    pub(crate) fn new(
        config: WorkLinkPickerConfig,
        host: WorkLinkPickerHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = config.kind.placeholder();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let change = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if value != this.query {
                        this.query = value;
                        this.request(true, cx);
                    }
                }
            },
        );
        search.update(cx, |input, cx| input.focus(window, cx));
        let mut this = Self {
            host,
            config,
            search,
            query: String::new(),
            rows: Vec::new(),
            ticked: Vec::new(),
            seeded: false,
            notice: None,
            loading: true,
            error: None,
            selected: 0,
            request_generation: 0,
            scroll: ScrollHandle::new(),
            pending_scroll: None,
            glass: crate::app::helpers::window_glass_active(),
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![change],
        };
        this.request(false, cx);
        this
    }

    fn close_window_and_send(
        &mut self,
        command: WorkLinkPickerCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.remove_window();
        (self.host)(command, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_window_and_send(WorkLinkPickerCommand::Close, window, cx);
    }

    /// The rows the list shows: gxserver's answer, narrowed at once by what is typed so the list
    /// follows the keyboard while the next answer is on its way.
    fn visible_rows(&self) -> Vec<&Candidate> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.rows
            .iter()
            .filter(|row| {
                if words.is_empty() || row.linked {
                    return true;
                }
                let haystack = format!(
                    "{} {} {}",
                    row.label,
                    row.title,
                    row.detail.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                words.iter().all(|word| haystack.contains(word.as_str()))
            })
            .collect()
    }

    fn request(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.request_generation += 1;
        let generation = self.request_generation;
        self.loading = true;
        self.error = None;
        self.selected = 0;
        self.pending_scroll = Some(0);
        cx.notify();
        let params = json!({
            "projectId": self.config.project_id,
            "sessionId": self.config.session_id,
            "kind": self.config.kind.wire(),
            "query": self.query.trim().chars().take(200).collect::<String>(),
        });
        cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(QUERY_DEBOUNCE).await;
                let current = this
                    .read_with(cx, |this, _| this.request_generation == generation)
                    .unwrap_or(false);
                if !current {
                    return;
                }
            }
            let result = gx_rpc(None, "/api/listWorkLinkCandidates", params).await;
            let _ = this.update(cx, |this, cx| {
                if this.request_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(answer) => this.apply_answer(&answer),
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_answer(&mut self, answer: &Value) {
        self.rows = answer
            .get("candidates")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Candidate::from_json).collect())
            .unwrap_or_default();
        self.notice = answer
            .get("notice")
            .and_then(Value::as_str)
            .map(str::to_string);
        if !self.seeded {
            self.seeded = true;
            self.ticked = answer
                .get("linked")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
        }
        self.selected = 0;
        self.pending_scroll = Some(0);
    }

    fn toggle(&mut self, value: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.ticked.iter().position(|ticked| ticked == value) {
            self.ticked.remove(index);
        } else {
            self.ticked.push(value.to_string());
        }
        cx.notify();
    }

    /// A single pick links the row; a multi-select links what is ticked plus the highlighted row,
    /// so "type, Enter" works the same in both.
    fn link(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let picked =
            index.and_then(|index| self.visible_rows().get(index).map(|row| row.value.clone()));
        let values: Vec<String> = if self.config.kind.multi_select() {
            let mut values = self.ticked.clone();
            if let Some(picked) = picked {
                if !values.contains(&picked) {
                    values.push(picked);
                }
            }
            values
        } else {
            match picked {
                Some(picked) => vec![picked],
                None => return,
            }
        };
        let links = self.config.kind.links(&values);
        self.close_window_and_send(WorkLinkPickerCommand::Link { links }, window, cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.visible_rows().len();
        if count == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        if next != self.selected {
            self.selected = next;
            self.pending_scroll = Some(next);
            cx.notify();
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.close(window, cx);
            }
            "down" => {
                cx.stop_propagation();
                self.move_selection(1, cx);
            }
            "up" => {
                cx.stop_propagation();
                self.move_selection(-1, cx);
            }
            "enter" if !event.is_held && !modifiers.alt && !modifiers.shift => {
                cx.stop_propagation();
                // A multi-select with nothing highlighted still links what is ticked.
                let index = (self.selected < self.visible_rows().len()).then_some(self.selected);
                self.link(index, window, cx);
            }
            _ => {
                let search = self.search.clone();
                if !search.read(cx).focus_handle(cx).is_focused(window) {
                    search.update(cx, |input, cx| input.focus(window, cx));
                }
            }
        }
    }
}

impl Render for GpuiWorkLinkPickerModalWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = QuickAccessPalette::current(self.glass);
        if let Some(index) = self.pending_scroll.take() {
            self.scroll.scroll_to_item(index);
        }
        let title = format!(
            "Link \u{201c}{}\u{201d} to {}",
            self.config.session_title,
            self.config.kind.noun()
        );
        div()
            .id("work-link-picker-window")
            .size_full()
            .overflow_hidden()
            .bg(hsla(p.window))
            .font_family(MODAL_UI_FONT)
            .text_size(px(QUICK_ACCESS_ITEM_FONT_SIZE))
            .line_height(px(18.0))
            .text_color(hsla(p.item))
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .child(
                v_flex()
                    .size_full()
                    .min_h_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .w_full()
                            .h(px(HEADER_HEIGHT))
                            .px(px(16.0))
                            .pt(px(14.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(p.foreground))
                            .child(SharedString::from(title)),
                    )
                    .child(quick_access_search_bar(
                        &p,
                        &self.search,
                        !self.query.is_empty(),
                        Vec::new(),
                        |this: &mut Self, window, cx| {
                            this.search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                            if !this.query.is_empty() {
                                this.query.clear();
                                this.request(false, cx);
                            }
                        },
                        cx,
                    ))
                    .child(self.render_list(&p, cx))
                    .child(self.render_footer(&p, cx)),
            )
    }
}

impl GpuiWorkLinkPickerModalWindow {
    fn render_list(&self, p: &QuickAccessPalette, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let rows = self.visible_rows();
        let has_own = rows.iter().any(|row| row.own_repo && !row.linked);
        let has_other = rows.iter().any(|row| !row.own_repo && !row.linked);
        let mut children: Vec<AnyElement> = Vec::new();
        let mut section: Option<&'static str> = None;
        for (index, row) in rows.iter().enumerate() {
            // Headings only help when the list mixes the session's repo with everything else.
            let heading = if row.linked {
                "Linked now"
            } else if has_own && has_other {
                // Every PR listed is from this repo; the ones gxserver ranks first are the
                // session branch's own.
                match (self.config.kind, row.own_repo) {
                    (WorkLinkKind::PullRequest, true) => "This session's branch",
                    (WorkLinkKind::PullRequest, false) => "Other open pull requests",
                    (WorkLinkKind::GithubProject, true) => "The repo owner's projects",
                    (WorkLinkKind::GithubProject, false) => "Your projects",
                    (_, true) => "From this repo",
                    (_, false) => "More",
                }
            } else {
                "Suggested"
            };
            if section != Some(heading) {
                children.push(section_heading(&p, heading, section.is_some()));
                section = Some(heading);
            }
            children.push(self.render_row(&p, index, row, cx));
        }
        let status = if let Some(error) = self.error.as_ref() {
            Some((error.clone(), p.destructive))
        } else if rows.is_empty() && self.loading {
            Some(("Loading suggestions…".to_string(), p.muted))
        } else if rows.is_empty() {
            Some((
                self.notice.clone().unwrap_or_else(|| {
                    if self.query.trim().is_empty() {
                        "Nothing to suggest yet. Type to search.".to_string()
                    } else {
                        "Nothing matches.".to_string()
                    }
                }),
                p.muted,
            ))
        } else {
            None
        };
        if let Some((text, color)) = status {
            children.push(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .py(px(18.0))
                    .px(px(12.0))
                    .text_center()
                    .text_size(px(QUICK_ACCESS_ITEM_FONT_SIZE))
                    .text_color(hsla(color))
                    .child(SharedString::from(text))
                    .into_any_element(),
            );
        }
        v_flex()
            .id("work-link-picker-list")
            .flex_1()
            .min_h_0()
            .w_full()
            .p(px(QUICK_ACCESS_LIST_PADDING))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(children)
            .into_any_element()
    }

    /// One suggestion: the tick (multi-select), its ID, its title, and who or what state on the
    /// right; the URL and every detail in the tooltip.
    fn render_row(
        &self,
        p: &QuickAccessPalette,
        index: usize,
        row: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let multi = self.config.kind.multi_select();
        let ticked = multi && self.ticked.contains(&row.value);
        let selected = index == self.selected;
        let tooltip = [
            Some(row.label.as_str()),
            Some(row.title.as_str()),
            row.detail.as_deref(),
            row.url.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        let value = row.value.clone();
        let label = if row.label.is_empty() {
            row.value.clone()
        } else {
            row.label.clone()
        };
        h_flex()
            .id(("work-link-picker-row", index))
            .w_full()
            .flex_shrink_0()
            .h(px(QUICK_ACCESS_ROW_HEIGHT))
            .px(px(QUICK_ACCESS_ROW_PADDING_X))
            .gap(px(10.0))
            .items_center()
            .rounded(px(QUICK_ACCESS_ROW_RADIUS))
            .cursor_default()
            .when(selected, |this| this.bg(hsla(p.row_selected)))
            .on_mouse_move(cx.listener(move |this, _, _window, cx| {
                if this.selected != index {
                    this.selected = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if multi {
                    this.toggle(&value, cx);
                } else {
                    this.link(Some(index), window, cx);
                }
            }))
            .tooltip(move |window, cx| quick_access_tooltip(tooltip.clone(), window, cx))
            .when(multi, |this| this.child(tick_box(&p, ticked)))
            .child(
                div()
                    .flex_shrink_0()
                    .min_w(px(72.0))
                    .font_family(MODAL_MONO_FONT)
                    .text_size(px(QUICK_ACCESS_META_FONT_SIZE))
                    .text_color(hsla(p.muted))
                    .whitespace_nowrap()
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(QUICK_ACCESS_ROW_FONT_SIZE))
                    .text_color(hsla(p.foreground))
                    .child(SharedString::from(row.title.clone())),
            )
            .children(row.detail.clone().map(|detail| {
                div()
                    .flex_shrink_0()
                    .max_w(px(180.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(QUICK_ACCESS_META_FONT_SIZE))
                    .text_color(hsla(p.muted))
                    .child(SharedString::from(detail))
            }))
            .when(row.linked && !multi, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.5))
                        .text_color(hsla(p.accent))
                        .child("Linked"),
                )
            })
            .into_any_element()
    }

    fn render_footer(&self, p: &QuickAccessPalette, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let multi = self.config.kind.multi_select();
        let hint = if multi {
            "Click to tick several. Enter links the ticked issues and the highlighted one."
        } else {
            "Enter links it. The card shows the chip right away."
        };
        let action = if multi {
            let count = self.ticked.len();
            match count {
                0 => "Link".to_string(),
                1 => "Link 1 issue".to_string(),
                count => format!("Link {count} issues"),
            }
        } else {
            "Link".to_string()
        };
        let can_link = multi || self.selected < self.visible_rows().len();
        h_flex()
            .id("work-link-picker-footer")
            .flex_shrink_0()
            .w_full()
            .h(px(QUICK_ACCESS_FOOTER_HEIGHT))
            .px(px(14.0))
            .gap(px(8.0))
            .items_center()
            .border_t_1()
            .border_color(hsla(p.hairline))
            .bg(hsla(p.footer))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.5))
                    .text_color(hsla(p.muted))
                    .child(hint),
            )
            .children(can_link.then(|| {
                h_flex()
                    .id("work-link-picker-link")
                    .flex_shrink_0()
                    .h(px(28.0))
                    .pl(px(9.0))
                    .pr(px(6.0))
                    .gap(px(7.0))
                    .items_center()
                    .rounded(px(7.0))
                    .text_size(px(12.5))
                    .text_color(hsla(p.foreground))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hsla(p.raised)))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        let index = (!this.config.kind.multi_select()
                            && this.selected < this.visible_rows().len())
                        .then_some(this.selected);
                        this.link(index, window, cx);
                    }))
                    .child(SharedString::from(action))
                    .child(quick_access_keycap(&p, "↵"))
            }))
            .into_any_element()
    }
}

fn section_heading(p: &QuickAccessPalette, label: &'static str, after_rows: bool) -> AnyElement {
    div()
        .w_full()
        .flex_shrink_0()
        .min_h(px(QUICK_ACCESS_GROUP_HEADING_HEIGHT))
        .px(px(QUICK_ACCESS_ROW_PADDING_X))
        .pt(px(if after_rows { 10.0 } else { 5.0 }))
        .pb(px(5.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(16.0))
        .text_color(hsla(p.muted))
        .child(label)
        .into_any_element()
}

fn tick_box(p: &QuickAccessPalette, ticked: bool) -> AnyElement {
    div()
        .flex_shrink_0()
        .size(px(15.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(hsla(if ticked { p.accent } else { p.hairline }))
        .when(ticked, |this| this.bg(hsla(p.accent)))
        .text_size(px(11.0))
        .line_height(px(11.0))
        .text_color(hsla(p.solid_window))
        .when(ticked, |this| this.child("✓"))
        .into_any_element()
}

impl Focusable for GpuiWorkLinkPickerModalWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalCornerClose for GpuiWorkLinkPickerModalWindow {
    fn close_from_corner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }
}
