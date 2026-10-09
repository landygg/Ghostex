//! Open and close plumbing for the floating window a chat page card opens.
//! SEE-ALSO: apps/desktop/src/app/window/visual_page_modal.rs (the window entity), apps/desktop/src/app/native_app_modal_lifecycle.rs (the shared window path), apps/desktop/src/app/cef_deferred_startup.rs (an open that waits for the web runtime).
use crate::app::window::*;
use crate::*;

impl GhostexGpuiApp {
    /// Opens the floating window for the `visualPage` open message the chat's page card sends
    /// (`url`, `title`, and the published file's name as `file`). The page needs the web runtime:
    /// until it runs, the open is held (and the install prompt shown when it is not installed).
    pub(crate) fn open_gpui_visual_page_modal(
        &mut self,
        message: &serde_json::Value,
        cx: &mut gpui::Context<Self>,
    ) {
        let text = |key: &str| {
            message
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let Some(url) = text("url").filter(|url| {
            gpui::http_client::Url::parse(url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        }) else {
            return;
        };
        if !cef::context_initialized() {
            self.defer_gpui_app_modal_open_for_cef(
                GpuiAppModalKind::VisualPage,
                message.clone(),
                cx,
            );
            return;
        }
        let title = text("title").unwrap_or_else(|| "Page".to_string());
        let detail = match text("file") {
            Some(file) => format!("Interactive HTML page · {file}"),
            None => "Interactive HTML page".to_string(),
        };
        let palette = self.gpui_native_modal_palette();
        let host = self.native_app_modal_host(cx, |app, command, cx| {
            app.handle_gpui_visual_page_modal_command(command, cx);
        });
        let kind = GpuiAppModalKind::VisualPage;
        let (width, height) = self.gpui_native_modal_size_on_screen(kind.window_size(), cx);
        self.open_native_app_modal(
            kind,
            width,
            height,
            move |window, cx| {
                cx.new(|cx| {
                    GpuiVisualPageModalWindow::new(url, title, detail, palette, host, window, cx)
                })
            },
            cx,
        );
    }

    fn handle_gpui_visual_page_modal_command(
        &mut self,
        command: VisualPageModalCommand,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.native_app_modal_kind() != Some(GpuiAppModalKind::VisualPage) {
            return;
        }
        self.close_native_app_modal_from_bridge(cx);
        if let VisualPageModalCommand::OpenInBrowser { url } = command {
            let _ = gpui_open_external_http_url(&url);
        }
    }
}
