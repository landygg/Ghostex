//! A web page asking for something outside the page: opening another app through its link scheme
//! (`com-okta-authenticator:`, `zoommtg:`, `mailto:`), or connecting to apps on this computer and
//! devices on the local network. Alloy-style CEF refuses both without a word, so every CEF page
//! hands them to the app, which asks the user (`app/browser_site_requests.rs`).
use super::*;

/// CDXC:Browser 2026-10-03 WHY:
/// Every Ghostex CEF browser is Alloy style (a native child view always is), and Alloy drops an
/// unknown-scheme navigation unless `OnProtocolExecution` allows it and answers a permission prompt
/// it has no handler for with IGNORE. Okta FastPass (Okta Verify on the desktop) needs both: it
/// probes Okta Verify's server on 127.0.0.1, which Chromium's Local Network Access gates behind a
/// `loopback-network` prompt, and falls back to loading `com-okta-authenticator:/deviceChallenge…`
/// in a hidden iframe, which is also what its "Open Okta Verify" button does. So the Linear
/// extension's Okta SSO never reached Okta Verify (Discord report, 10.8.1). Chrome asks before
/// either; so does Ghostex. CEF never launches the app itself (`allow_os_execution` stays 0): the
/// app checks that the OS has an app for the link, asks, and opens it, the same way for Browser
/// panes, website and custom views, extension views, modals and panels.
/// SEE-ALSO: app/browser_site_requests.rs (the prompts), GhostexGpuiPermissionHandler (the
/// Local Network Access prompt), request_handling.rs and browser_handlers.rs (the popup paths).
pub enum BrowserSiteRequest {
    OpenExternalApp(BrowserExternalAppRequest),
    LocalNetworkAccess(BrowserLocalNetworkAccessRequest),
}

pub struct BrowserExternalAppRequest {
    pub url: String,
    /// Lowercase, without the colon.
    pub scheme: String,
    /// The asking page's `scheme://host[:port]`, empty when it is not an http(s) page.
    pub origin: String,
    /// Closes the Browser tab that was opened only for this link; runs once the request is done
    /// with (answered, skipped or not shown), so it never outlives the prompt.
    close_link_only_tab: Option<BrowserPageMetadataHandler>,
}

impl Drop for BrowserExternalAppRequest {
    fn drop(&mut self) {
        if let Some(handler) = self.close_link_only_tab.take() {
            handler(BrowserPageMetadataEvent::CloseRequested);
        }
    }
}

/// A pending Local Network Access prompt. It stays open until `allow` runs or it is dropped, and
/// dropping it answers "not now" (DISMISS) so the page's request never hangs and the site is not
/// blocked for good: Ghostex has no site-settings page to undo a block.
pub struct BrowserLocalNetworkAccessRequest {
    pub(crate) origin: String,
    pub(crate) local_network: bool,
    pub(crate) callback: Option<PermissionPromptCallback>,
}

impl BrowserLocalNetworkAccessRequest {
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// True when the page asked for devices on the local network, not only apps on this computer.
    pub fn includes_local_network(&self) -> bool {
        self.local_network
    }

    /// Chromium stores the grant for the origin in the browser profile, so it is asked once.
    pub fn allow(mut self) {
        if let Some(callback) = self.callback.take() {
            callback.cont(PermissionRequestResult::ACCEPT);
        }
    }
}

impl Drop for BrowserLocalNetworkAccessRequest {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback.cont(PermissionRequestResult::DISMISS);
        }
    }
}

pub type BrowserSiteRequestHandler = StdRc<dyn Fn(BrowserSiteRequest)>;

thread_local! {
    static BROWSER_SITE_REQUEST_HANDLER: RefCell<Option<BrowserSiteRequestHandler>> =
        const { RefCell::new(None) };
}

/// The app registers this once; it runs on the CEF UI thread, which is the app's main thread.
pub fn set_browser_site_request_handler(handler: BrowserSiteRequestHandler) {
    BROWSER_SITE_REQUEST_HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

pub(crate) fn dispatch_browser_site_request(request: BrowserSiteRequest) {
    let handler = BROWSER_SITE_REQUEST_HANDLER.with(|slot| slot.borrow().clone());
    if let Some(handler) = handler {
        handler(request);
    }
}

/// Schemes no page may hand to the OS: the ones Chromium loads itself, Ghostex's own (a page must
/// not drive this app through the OS), and Chrome's never-launch list plus two Windows exploit
/// vectors (`ms-msdt`, `search-ms`).
const NEVER_OPENED_IN_ANOTHER_APP_SCHEMES: &[&str] = &[
    "about",
    "afp",
    "blob",
    "cef",
    "chrome",
    "chrome-devtools",
    "chrome-error",
    "chrome-extension",
    "chrome-search",
    "chrome-untrusted",
    "data",
    "devtools",
    "disk",
    "disks",
    "file",
    "filesystem",
    "ftp",
    "ghostex",
    "hcp",
    "http",
    "https",
    "ie.http",
    "javascript",
    "ms-help",
    "ms-msdt",
    "nntp",
    "res",
    "search-ms",
    "shell",
    "vbscript",
    "view-source",
    "vnd.ms.radio",
    "ws",
    "wss",
];

/// The scheme of a link that only another app can open, or None when the page must not hand it to
/// the OS at all.
pub(crate) fn external_app_link_scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.trim().split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    // A one-letter "scheme" is a Windows drive (`C:\…`), not a link.
    if !valid || scheme.len() < 2 {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    (!NEVER_OPENED_IN_ANOTHER_APP_SCHEMES.contains(&scheme.as_str())).then_some(scheme)
}

fn web_page_origin(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default()
}

/// The page the request came from: the browser's main frame (the Okta link loads in a hidden
/// iframe, whose own URL says nothing).
fn browser_page_origin(browser: Option<&mut cef::Browser>) -> String {
    browser
        .and_then(|browser| browser.main_frame())
        .map(|frame| web_page_origin(&CefString::from(&frame.url()).to_string()))
        .unwrap_or_default()
}

/// For the popup paths (`window.open`, target=_blank, Cmd/Ctrl-click), which run on the UI thread:
/// an app link is handed to the app instead of becoming a blank Browser tab. True when it was one.
pub(crate) fn dispatch_external_app_popup(
    browser: Option<&mut cef::Browser>,
    target_url: Option<&CefString>,
) -> bool {
    let url = target_url.map(CefString::to_string).unwrap_or_default();
    let Some(scheme) = external_app_link_scheme(&url) else {
        return false;
    };
    dispatch_browser_site_request(BrowserSiteRequest::OpenExternalApp(
        BrowserExternalAppRequest {
            url,
            scheme,
            origin: browser_page_origin(browser),
            close_link_only_tab: None,
        },
    ));
    true
}

/// CDXC:Browser 2026-10-10 WHY:
/// Handing an app link to the OS from `on_protocol_execution` still commits Chromium's
/// ERR_UNKNOWN_URL_SCHEME page in the frame (linear.app redirecting a Browser tab to `linear://`
/// when Linear's "Open in desktop app" is on), so every request handler's `on_before_browse` asks
/// here first: the navigation is cancelled, the frame keeps what it showed, and the app prompts as
/// before. A Browser tab that had loaded no page yet was opened only for that link, so it closes
/// once the prompt is done with, as Chrome does (`page_metadata_handler` is Browser tabs only).
/// True when the navigation was such a link and must be cancelled.
pub(crate) fn cancel_external_app_navigation(
    browser: Option<&mut cef::Browser>,
    frame: Option<&mut Frame>,
    request: Option<&mut Request>,
    page_metadata_handler: Option<&BrowserPageMetadataHandler>,
) -> bool {
    let url = request
        .map(|request| CefString::from(&request.url()).to_string())
        .unwrap_or_default();
    let Some(scheme) = external_app_link_scheme(&url) else {
        return false;
    };
    let is_main_frame = frame.is_none_or(|frame| frame.is_main() != 0);
    let has_document = browser
        .as_deref()
        .is_some_and(|browser| browser.has_document() != 0);
    let close_link_only_tab = page_metadata_handler
        .filter(|_| is_main_frame && !has_document)
        .cloned();
    dispatch_browser_site_request(BrowserSiteRequest::OpenExternalApp(
        BrowserExternalAppRequest {
            url,
            scheme,
            origin: browser_page_origin(browser),
            close_link_only_tab,
        },
    ));
    true
}

wrap_task! {
    pub(crate) struct GhostexDispatchExternalAppRequest {
        url: String,
        scheme: String,
        origin: String,
    }

    impl Task {
        fn execute(&self) {
            dispatch_browser_site_request(BrowserSiteRequest::OpenExternalApp(
                BrowserExternalAppRequest {
                    url: self.url.clone(),
                    scheme: self.scheme.clone(),
                    origin: self.origin.clone(),
                    close_link_only_tab: None,
                },
            ));
        }
    }
}

wrap_resource_request_handler! {
    pub(crate) struct GhostexExternalAppResourceRequestHandler;

    impl ResourceRequestHandler {
        // The cef-rs default answers RV_CANCEL (see GhostexManageDocsResourceRequestHandler).
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut cef::Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            ReturnValue::CONTINUE
        }

        fn on_protocol_execution(
            &self,
            browser: Option<&mut cef::Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            allow_os_execution: Option<&mut c_int>,
        ) {
            if let Some(allow_os_execution) = allow_os_execution {
                *allow_os_execution = 0;
            }
            let Some(url) = request.map(|request| CefString::from(&request.url()).to_string())
            else {
                return;
            };
            let Some(scheme) = external_app_link_scheme(&url) else {
                return;
            };
            // This runs on CEF's IO thread; the app's handler lives on the UI thread.
            let mut task =
                GhostexDispatchExternalAppRequest::new(url, scheme, browser_page_origin(browser));
            post_task(ThreadId::UI, Some(&mut task));
        }
    }
}

/// The resource handler for a request only another app can open, so CEF calls
/// `on_protocol_execution` for it; every other request keeps CEF's default handling.
pub(crate) fn external_app_resource_request_handler(
    request: Option<&mut Request>,
) -> Option<ResourceRequestHandler> {
    let url = CefString::from(&request?.url()).to_string();
    external_app_link_scheme(&url).map(|_| GhostexExternalAppResourceRequestHandler::new())
}

// For pages with no request handler of their own (extension views, modals, panels).
wrap_request_handler! {
    pub(crate) struct GhostexGpuiExternalAppRequestHandler;

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut cef::Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            cancel_external_app_navigation(browser, frame, request, None) as c_int
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut cef::Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _is_navigation: c_int,
            _is_download: c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut c_int>,
        ) -> Option<ResourceRequestHandler> {
            external_app_resource_request_handler(request)
        }

        fn on_open_urlfrom_tab(
            &self,
            browser: Option<&mut cef::Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
        ) -> c_int {
            dispatch_external_app_popup(browser, target_url) as c_int
        }
    }
}
