// Launch-time CEF deferral: the runtime starts only when a web view is about to be shown, and
// only when it is installed (app/helpers/web_runtime.rs).

use futures::StreamExt as _;

use crate::app::helpers::web_runtime::*;
use crate::app::model::*;
use crate::*;

/// An app-modal open that arrived before CEF was ready.
pub(crate) struct GpuiAppModalOpenDeferredForCef {
    modal: GpuiAppModalKind,
    open_message: serde_json::Value,
}

impl GhostexGpuiApp {
    /// CDXC:CefRuntime 2026-09-28 DECISION:
    /// User: "I want the user to be able to actually use almost all of the app without even installing or running the CEF browser". Launch starts no web runtime and no longer warms it once launch has settled (supersedes the 2026-09-19 decision to warm CEF after launch so Settings and Quick Access opened instantly; Quick Access is native and Settings is being ported). CEF starts on the first browser-creation signal, which only a web view about to be shown sends (a Browser tab, a web workarea view, an extension popup or modal, Files' browser area), including one restored at launch.
    pub(crate) fn begin_deferred_cef_startup(&mut self, cx: &mut gpui::Context<Self>) {
        self.gx_store_load_remote_recent_projects(cx);
        if let Some(mut demand) = cef::take_runtime_demand_receiver() {
            // Every open window answers the signal, and the listener outlives the window that
            // took it (app/workspace_windows/).
            let app: &gpui::App = cx;
            app.spawn(async move |cx| {
                while demand.next().await.is_some() {
                    cx.update(crate::app::workspace_windows::request_cef_runtime_in_every_window);
                }
            })
            .detach();
        }
    }

    /// A web view wants CEF. Starts it when it is installed and not started yet; while it is not
    /// installed, being installed or failed, the web views show the prompt instead and nothing
    /// is downloaded.
    pub(crate) fn request_cef_runtime(&mut self, cx: &mut gpui::Context<Self>) {
        if self.cef_runtime_requested || web_runtime_state() != WebRuntimeState::Installed {
            return;
        }
        self.cef_runtime_requested = true;
        self.start_web_runtime(cx);
    }

    /// The modal host creates its page once and does not retry, so an open that lands before CEF is ready is held and replayed instead of showing an empty window. The newest request wins, matching the one-app-modal rule.
    /// While the runtime is not installed the open is still held, so the modal opens once the user installs it from the prompt (`open_web_runtime_modal_prompt`).
    pub(crate) fn defer_gpui_app_modal_open_for_cef(
        &mut self,
        modal: GpuiAppModalKind,
        open_message: serde_json::Value,
        cx: &mut gpui::Context<Self>,
    ) {
        self.app_modal_open_deferred_for_cef = Some(GpuiAppModalOpenDeferredForCef {
            modal,
            open_message,
        });
        if web_runtime_available() {
            self.request_cef_runtime(cx);
        } else {
            self.open_web_runtime_modal_prompt(modal, cx);
        }
    }

    /// The modal waiting for the web runtime, if any.
    pub(crate) fn gpui_app_modal_deferred_for_cef_kind(&self) -> Option<GpuiAppModalKind> {
        self.app_modal_open_deferred_for_cef
            .as_ref()
            .map(|deferred| deferred.modal)
    }

    pub(crate) fn forget_gpui_app_modal_deferred_for_cef(&mut self) {
        self.app_modal_open_deferred_for_cef = None;
    }

    pub(crate) fn open_gpui_app_modal_deferred_for_cef(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(deferred) = self.app_modal_open_deferred_for_cef.take() else {
            return;
        };
        // A floating chat page is a native window with its own CEF page, not the extension host.
        if deferred.modal == GpuiAppModalKind::VisualPage {
            self.open_gpui_visual_page_modal(&deferred.open_message, cx);
            return;
        }
        self.open_gpui_app_modal_window_inner(deferred.modal, deferred.open_message, None, cx);
    }
}
