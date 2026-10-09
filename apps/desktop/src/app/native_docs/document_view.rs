//! The document side of Docs: its header and the open file, as an editor, formatted, or a picture.

use std::time::{Duration, Instant};

use gpui::StyledImage as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px, rems,
};
use gpui_component::input::Editor;
use gpui_component::scroll::ScrollbarMode;

use super::files_list::{ROW_STRIP_HEIGHT, header_icon, header_tile};
use super::palette::DocsPalette;
use super::sidebar::DocsSidebarLayout;
use super::state::{DocsDocumentLoad, DocsFileKind, DocsMarkdownMode};
use crate::GhostexGpuiApp;
use crate::app::consts::WORKAREA_HEADER_EDGE_PADDING;
use crate::app::helpers::{titlebar_svg_icon, titlebar_tooltip};

thread_local! {
    /// The document header's bounds, so the selection toolbar flips below the text instead of
    /// covering the header.
    pub(crate) static HEADER_BOUNDS: std::cell::Cell<gpui::Bounds<gpui::Pixels>> =
        std::cell::Cell::new(gpui::Bounds::default());
}

/// How long "Saved" and "Copied!" stay up.
const FLASH: Duration = Duration::from_millis(1600);

/// `formatFileSize`.
fn format_file_size(size: u64) -> String {
    if size < 1024 {
        return format!("{size} B");
    }
    let units = ["KB", "MB", "GB"];
    let mut value = size as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 {
        format!("{value:.0} {}", units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// `languageLabelForPath`.
fn language_label(path: &str) -> String {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return "Text".to_string();
    };
    let extension = extension.to_lowercase();
    match extension.as_str() {
        "css" => "CSS",
        "excalidraw" => "Excalidraw",
        "go" => "Go",
        "h" => "C/C++",
        "html" => "HTML",
        "js" | "mjs" => "JavaScript",
        "json" => "JSON",
        "jsx" | "tsx" => "React",
        "md" => "Markdown",
        "py" => "Python",
        "rs" => "Rust",
        "sh" => "Shell",
        "swift" => "Swift",
        "ts" => "TypeScript",
        "txt" => "Text",
        "yaml" | "yml" => "YAML",
        "zig" => "Zig",
        _ => return extension.to_uppercase(),
    }
    .to_string()
}

impl GhostexGpuiApp {
    pub(crate) fn render_native_docs_document(
        &mut self,
        p: &DocsPalette,
        layout: DocsSidebarLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = self.render_native_docs_header(p, layout, cx);
        let body = self.render_native_docs_body(p, window, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(header)
            // The body fills exactly the space under the header. A percentage height inside the
            // flexed row resolved against the whole column instead, which pushed the body's last
            // 37px (the formatting bar and the document's last lines) below the view.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(div().absolute().inset_0().child(body)),
            )
            .into_any_element()
    }

    /// The document header: the title (which copies itself), the meta strip and the actions.
    ///
    /// CDXC:Docs 2026-09-21 DECISION:
    /// User: make the buttons on the top right of the Docs view match the look, gap, and right-edge alignment of the native view tab strip buttons above them: 32px by 27px tiles, 7px radius, 2px gap, 18px stroke-2 icons, and a 9px right inset.
    fn render_native_docs_header(
        &mut self,
        p: &DocsPalette,
        layout: DocsSidebarLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(document) = self.native_docs.active_document() else {
            return div()
                .flex_none()
                .h(px(ROW_STRIP_HEIGHT))
                .border_b_1()
                .border_color(p.border)
                .bg(p.chrome)
                .into_any_element();
        };
        let now = Instant::now();
        let title: SharedString = document.display_path.clone().into();
        let dirty = document.dirty;
        let copied = document.title_copied_until.is_some_and(|until| until > now);
        let saved = document.saved_flash_until.is_some_and(|until| until > now);
        let kind = document.kind;
        let path = document.path.clone();
        let review = document.review_session_title.clone();
        let language = if review.is_some() {
            "Agent reply".to_string()
        } else {
            language_label(&document.path)
        };
        let external_change = document.external_change;
        let size = document.size.map(format_file_size);
        let html_annotate = document.html_annotate;
        let svg = kind == DocsFileKind::Image && DocsFileKind::is_svg(&path);
        let svg_source = document.svg_source;
        let media = matches!(
            kind,
            DocsFileKind::Image | DocsFileKind::Video | DocsFileKind::Audio
        );
        // CDXC:Docs 2026-10-01 WHY: the Show files button is the header row's last control, not a corner overlay drawn over the row's right end; a guessed reserve for the overlay let it cover the actions (the Open With arrow and Reload) whenever the row and the overlay disagreed.
        // CDXC:Docs 2026-10-09 DECISION: User: "when i hover the button to show files list pls keep the bar behind to not change". The button stays in the row while the list floats over it (a peek or a drawer) and goes only once the list is docked, so opening the floating list never re-lays out the bar: the title, the meta strip and the other buttons stay exactly where they were.
        let restore = (!layout.docked).then(|| self.render_native_docs_restore_button(p, cx));
        // CDXC:Docs 2026-10-08 DECISION:
        // User: "when i click here please lets copy the whole path not the file name". Clicking the top bar's file name copies the file's full path on its computer (the project folder joined with a project-relative path), supersedes the 2026-09-07 rule of copying the displayed name.
        let copy_title: SharedString = {
            let file = std::path::Path::new(&path);
            if file.is_absolute() {
                path.clone()
            } else {
                self.native_docs
                    .project
                    .as_ref()
                    .map(|project| {
                        project
                            .project_path
                            .join(file)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .unwrap_or_else(|| path.clone())
            }
        }
        .into();
        let tooltip: SharedString = if copied {
            "Copied!".into()
        } else if dirty {
            if cfg!(target_os = "macos") {
                "Unsaved changes. Press ⌘S to save. Click to copy the full path"
            } else {
                "Unsaved changes. Press Ctrl+S to save. Click to copy the full path"
            }
            .into()
        } else {
            "Copy full path".into()
        };
        let reload_path = path.clone();
        let note_actions = if kind == DocsFileKind::Markdown {
            let compact = super::render::view_width() < 560.0;
            self.render_native_docs_note_actions(p, compact, cx)
        } else {
            Vec::new()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(9.0))
            .h(px(ROW_STRIP_HEIGHT))
            .pl(px(13.0))
            .pr(px(WORKAREA_HEADER_EDGE_PADDING))
            .border_b_1()
            .border_color(p.border)
            .bg(p.chrome)
            .child(
                gpui::canvas(
                    |bounds, _, _| HEADER_BOUNDS.with(|cell| cell.set(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                // CDXC:Docs 2026-09-07 DECISION:
                // User: clicking the file name in the Docs top bar copies it and shows a tooltip. Copy the displayed name or path, including the readable name of mounted folders.
                //
                // CDXC:Docs 2026-09-15 DECISION:
                // User: unsaved changes are shown by the file icon in the top bar, which becomes a filled dot until the file is saved. Long titles truncate from the start so the buttons on the right stay on screen.
                div()
                    .id("native-docs-title")
                    .flex()
                    .flex_shrink(1.0)
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .child(if dirty {
                        div()
                            .flex_none()
                            .size(px(15.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(9.0)).rounded_full().bg(p.text))
                            .into_any_element()
                    } else {
                        titlebar_svg_icon(
                            if kind == DocsFileKind::Excalidraw {
                                "files-view/t-edit-175.svg"
                            } else if media {
                                super::files_list::file_icon(&path)
                            } else {
                                "files-view/t-file-text-175.svg"
                            },
                            15.0,
                            p.muted,
                        )
                        .into_any_element()
                    })
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_start()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(title),
                    )
                    .tooltip(move |window, cx| titlebar_tooltip(tooltip.clone(), window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_title.to_string()));
                        crate::app::helpers::gpui_copy_feedback(cx);
                        if let Some(active) = this.native_docs.active.clone()
                            && let Some(document) = this.native_docs.document_mut(&active)
                        {
                            document.title_copied_until = Some(Instant::now() + FLASH);
                        }
                        this.native_docs_notify_after(FLASH, cx);
                        this.native_docs_notify(cx);
                    })),
            )
            .child(
                // The meta strip gives way before the actions do, so a narrow view never pushes
                // a button out of the row.
                div()
                    .flex()
                    .flex_shrink(1.0)
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .items_center()
                    .gap(px(9.0))
                    .text_size(px(10.5))
                    .text_color(p.subtle)
                    .child(language)
                    .children(review.clone().filter(|title| !title.is_empty()))
                    .children(size)
                    .when(dirty, |this| this.child("Edited"))
                    .when(!dirty && saved, |this| this.child("Saved")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.0))
                    .children(note_actions)
                    .when(kind.opens_externally() && review.is_none(), |this| {
                        let (open_path, menu_path) = (path.clone(), path.clone());
                        this.child(
                            header_tile(
                                "native-docs-open-externally",
                                header_icon("titlebar/external-link.svg", false, p),
                                false,
                                false,
                                p,
                            )
                            .tooltip(|window, cx| titlebar_tooltip("Open in browser", window, cx))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.native_docs_open_externally(&open_path, None, cx);
                                },
                            )),
                        )
                        .child(
                            header_tile(
                                "native-docs-open-with",
                                titlebar_svg_icon(
                                    "titlebar/chevron-down.svg",
                                    12.0,
                                    p.toolbar_icon,
                                ),
                                false,
                                false,
                                p,
                            )
                            .w(px(16.0))
                            .child(
                                gpui::canvas(
                                    |bounds, _, _| {
                                        super::open_externally::OPEN_WITH_MENU_ANCHOR
                                            .with(|cell| cell.set(bounds))
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .tooltip(|window, cx| titlebar_tooltip("Open with", window, cx))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.show_native_docs_open_with_menu(&menu_path, window, cx);
                                },
                            )),
                        )
                    })
                    .when(kind == DocsFileKind::Html, |this| {
                        let annotate = html_annotate;
                        let toggle_path = path.clone();
                        this.child(
                            header_tile(
                                "native-docs-html-annotate",
                                header_icon("files-view/t-message-plus-2.svg", false, p),
                                annotate,
                                false,
                                p,
                            )
                            .tooltip(move |window, cx| {
                                titlebar_tooltip(
                                    if annotate {
                                        "Disable annotations"
                                    } else {
                                        "Enable annotations"
                                    },
                                    window,
                                    cx,
                                )
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(document) =
                                        this.native_docs.document_mut(&toggle_path)
                                    {
                                        document.html_annotate = !document.html_annotate;
                                    }
                                    this.native_docs_notify(cx);
                                },
                            )),
                        )
                    })
                    .when(svg, |this| {
                        let toggle_path = path.clone();
                        this.child(
                            header_tile(
                                "native-docs-svg-source",
                                header_icon(
                                    if svg_source {
                                        "titlebar/photo.svg"
                                    } else {
                                        "titlebar/code.svg"
                                    },
                                    false,
                                    p,
                                ),
                                false,
                                false,
                                p,
                            )
                            .tooltip(move |window, cx| {
                                titlebar_tooltip(
                                    if svg_source {
                                        "Show picture"
                                    } else {
                                        "Edit source"
                                    },
                                    window,
                                    cx,
                                )
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.native_docs_toggle_svg_source(&toggle_path, cx);
                                },
                            )),
                        )
                    })
                    .when(media && review.is_none(), |this| {
                        let open_path = path.clone();
                        this.child(
                            header_tile(
                                "native-docs-open-system-app",
                                header_icon("titlebar/external-link.svg", false, p),
                                false,
                                false,
                                p,
                            )
                            .tooltip(|window, cx| {
                                titlebar_tooltip("Open in system app", window, cx)
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.native_docs_open_with_system_app(&open_path, cx);
                                },
                            )),
                        )
                    })
                    .when(review.is_some(), |this| {
                        this.child(
                            header_tile(
                                "native-docs-close-review",
                                header_icon("titlebar/x.svg", false, p),
                                false,
                                false,
                                p,
                            )
                            .tooltip(|window, cx| {
                                titlebar_tooltip("Close reply review", window, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.native_docs_drop_reviews(cx)),
                            ),
                        )
                    })
                    .when(
                        review.is_none() && (kind != DocsFileKind::Image || svg_source),
                        |this| {
                            let amber = p.amber;
                            this.child(
                                header_tile(
                                    "native-docs-reload",
                                    header_icon("files-view/t-refresh-2.svg", false, p),
                                    false,
                                    false,
                                    p,
                                )
                                .when(external_change, |this| {
                                    this.child(
                                        div()
                                            .absolute()
                                            .top(px(6.0))
                                            .right(px(6.0))
                                            .size(px(7.0))
                                            .rounded_full()
                                            .bg(amber),
                                    )
                                })
                                .tooltip(move |window, cx| {
                                    titlebar_tooltip(
                                        if external_change {
                                            "Reload to show new changes"
                                        } else {
                                            "Reload file"
                                        },
                                        window,
                                        cx,
                                    )
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.native_docs_reload(&reload_path, cx);
                                    },
                                )),
                            )
                        },
                    )
                    .children(restore),
            )
            .into_any_element()
    }

    fn render_native_docs_body(
        &mut self,
        p: &DocsPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let notice = |icon: &'static str, text: String| {
            div()
                .size_full()
                .min_h(px(140.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.muted)
                .child(titlebar_svg_icon(icon, 16.0, p.muted))
                .child(text)
                .into_any_element()
        };
        let format_bar = self.render_native_docs_format_bar(p, cx);
        let Some(document) = self.native_docs.active_document() else {
            return notice("files-view/t-file-175.svg", "Select a file".to_string());
        };
        let action_button = |id: &'static str, icon: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(28.0))
                .px(px(12.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(p.border_strong)
                .cursor_pointer()
                .text_color(p.text)
                .hover(|style| style.bg(p.control_hover))
                .child(titlebar_svg_icon(icon, 14.0, p.text))
                .child(label)
        };
        let open_externally = |path: String, reason: String, cx: &mut Context<Self>| {
            div()
                .size_full()
                .min_h(px(140.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.muted)
                .child(reason)
                .child(
                    action_button(
                        "native-docs-open-system-app-button",
                        "titlebar/external-link.svg",
                        "Open in system app",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.native_docs_open_with_system_app(&path, cx);
                    })),
                )
                .into_any_element()
        };
        match &document.load {
            DocsDocumentLoad::Loading => {
                return notice("files-view/t-refresh-2.svg", "Loading file".to_string());
            }
            DocsDocumentLoad::Error(error) => {
                return notice("titlebar/alert-triangle.svg", error.clone());
            }
            DocsDocumentLoad::Unsupported(reason) if document.kind == DocsFileKind::Markdown => {
                // Only Markdown has a size limit (CDXC:Docs 2026-09-28 in
                // helpers/os_cli/process_and_constants.rs); past it the file opens in the Code view.
                let path = document.path.clone();
                return div()
                    .size_full()
                    .min_h(px(140.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.muted)
                    .child(reason.clone())
                    .child(
                        action_button(
                            "native-docs-open-code-view-button",
                            crate::app::consts::TITLEBAR_ICON_CODE,
                            "Open in Code view",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.native_docs_open_in_code_view(&path, cx);
                        })),
                    )
                    .into_any_element();
            }
            DocsDocumentLoad::Unsupported(reason) => {
                let (path, reason) = (document.path.clone(), reason.clone());
                return open_externally(path, reason, cx);
            }
            DocsDocumentLoad::Ready => {}
        }
        match document.kind {
            DocsFileKind::SystemApp => {
                let path = document.path.clone();
                return open_externally(path, "This file opens in its own app.".to_string(), cx);
            }
            DocsFileKind::Image if document.svg_source => {}
            DocsFileKind::Image => {
                let Some(image) = document.image.clone() else {
                    return notice("files-view/t-refresh-2.svg", "Loading file".to_string());
                };
                return div()
                    .id("native-docs-image")
                    .size_full()
                    .overflow_scroll()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.0))
                    .child(
                        gpui::img(image)
                            .max_w_full()
                            .max_h_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .into_any_element();
            }
            DocsFileKind::Html
            | DocsFileKind::Excalidraw
            | DocsFileKind::Video
            | DocsFileKind::Audio => {
                // The web runtime prompt (app/render/web_runtime_prompt.rs) with this file's Open
                // in system app beside Install.
                if let Some(prompt) =
                    crate::app::helpers::web_runtime::web_runtime_install_prompt("This file")
                {
                    let path = document.path.clone();
                    return self.render_web_runtime_prompt_card(
                        "native-docs-web-runtime-prompt",
                        prompt,
                        Some(path),
                        cx,
                    );
                }
                if self.native_docs_browser_area_covered() {
                    return div().size_full().into_any_element();
                }
                return self.render_native_docs_browser_area(cx).unwrap_or_else(|| {
                    notice("files-view/t-refresh-2.svg", "Loading file".to_string())
                });
            }
            DocsFileKind::Markdown => {
                let Some(live) = document.live.clone() else {
                    return notice("files-view/t-refresh-2.svg", "Loading file".to_string());
                };
                let table_actions = {
                    let app = cx.weak_entity();
                    super::table_tools::TableActionHost {
                        copy: std::rc::Rc::new(|text, cx| {
                            crate::app::helpers::gpui_copy_to_clipboard(
                                gpui::ClipboardItem::new_string(text),
                                cx,
                            );
                        }),
                        open: Some(std::rc::Rc::new(move |source, cx| {
                            let message = serde_json::json!({ "source": source });
                            let _ = app.update(cx, |app, cx| {
                                app.open_gpui_markdown_table_modal(&message, cx);
                            });
                        })),
                    }
                };
                let body = super::markdown_body::render_markdown_body(
                    super::markdown_body::DocsMarkdownBody {
                        id: SharedString::from(format!("native-docs-scroll-{}", document.path)),
                        live: &live,
                        scroll: &document.scroll,
                        source: document.mode == DocsMarkdownMode::Source,
                        line_numbers: self.native_docs.line_numbers,
                        constrain: self.native_docs.constrain_width,
                        changes: self
                            .native_docs
                            .git_changes
                            .then_some(document.changes.as_ref())
                            .flatten(),
                        table_actions: &table_actions,
                        sliding: crate::terminal_element::grid_resize_held(),
                    },
                    p,
                    window,
                    cx,
                )
                .capture_key_down(cx.listener(Self::native_docs_selection_key));
                let ruler = super::markdown_body::render_overview_ruler(
                    &live,
                    &document.scroll,
                    self.native_docs
                        .git_changes
                        .then_some(document.changes.as_ref())
                        .flatten(),
                    p,
                    cx,
                );
                return div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(body)
                    .children(ruler)
                    .child(super::markdown_body::render_body_scrollbar(
                        &document.scroll,
                    ))
                    .children(format_bar)
                    .into_any_element();
            }
            DocsFileKind::Text => {}
        }
        let Some(editor) = document.editor.clone() else {
            return notice("files-view/t-refresh-2.svg", "Loading file".to_string());
        };
        let path_row: SharedString = document.display_path.clone().into();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(18.0))
                    .py(px(8.0))
                    .text_size(px(11.0))
                    .font_family(p.mono_font.clone())
                    .text_color(p.subtle)
                    .child(path_row),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pt(px(16.0))
                    .px(px(18.0))
                    .pb(px(28.0))
                    .font_family(p.mono_font.clone())
                    .text_size(px(12.0))
                    .child(
                        // `Editor` brings the theme's code font, size and row height; the text
                        // view keeps the Docs mono font at the input text size and rows.
                        Editor::new(&editor)
                            .scrollbar_show(ScrollbarMode::Always)
                            .appearance(false)
                            .h_full()
                            .font_family(p.mono_font.clone())
                            .text_sm()
                            .line_height(rems(1.25)),
                    ),
            )
            .into_any_element()
    }

    /// Redraws once `after` has passed, to take down "Saved" and "Copied!".
    pub(crate) fn native_docs_notify_after(&mut self, after: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            let _ = this.update(cx, |this, cx| this.native_docs_notify(cx));
        })
        .detach();
    }
}
