use super::*;

const SESSION_CHAT_QUEUE_TICK_SECONDS: u64 = 1;

/// How long a session must look stopped before the head row is released.
/// Tracked exactly like `delayed_sends`' `nonWorkingSinceAt`: it restarts the
/// instant the session looks busy again, so a blip between two tool calls can
/// never be mistaken for the end of a turn.
pub const SESSION_CHAT_QUEUE_STABILITY_MS: i64 = 2_000;

/// Tail window for the lifecycle probe. Big enough that a turn ending in a run
/// of tool_use / tool_result rows still exposes the boundary record that named
/// the turn, small enough that the read is a couple of reverse chunks.
const SESSION_CHAT_QUEUE_LIFECYCLE_TAIL_LIMIT: usize = 8;

/*
The session's currently resolved terminal notice (screen classification merged
with the send watchdog), which is state `server.rs` owns. Injected the same way
the sender and publisher are, so this module never learns about AppState.
*/
pub type SessionChatQueueNoticeReader =
    Arc<dyn Fn(&str, &str) -> Option<SessionChatTerminalNotice> + Send + Sync>;

/*
CDXC:SessionChat 2026-08-26:
The session's last known composer verdict, injected the same way. Read from the
cache only — a tick must never spawn a capture — so a session nobody has probed
reads `Unknown` and the queue behaves exactly as it did before this feature.
*/
pub type SessionChatQueueComposerReader =
    Arc<dyn Fn(&str, &str) -> SessionChatComposerReadiness + Send + Sync>;

/*
Refresh screen evidence for a queued session waiting for startup or compaction.
This keeps queued delivery progressing even after the chat client disconnects.
*/
pub type SessionChatQueueCompactingRefresher = Arc<dyn Fn(&str, &str, Option<&str>) + Send + Sync>;

#[derive(Default)]
struct SessionQueueGate {
    /// First moment this session looked stopped without looking busy since.
    /// `None` means the window has not started (or was just reset).
    stopped_since: Option<DateTime<Utc>>,
    transcript: SessionChatTranscriptGate,
}

/// CDXC:DelayedSend 2026-09-16 WHY:
/// Hooks and titles can look stopped before a turn finishes. Conditional delayed sends and queued prompts must both check the transcript before starting their stability window.
#[derive(Default)]
pub(crate) struct SessionChatTranscriptGate {
    /// Cache resolution by agent and conversation identity; resolving can scan agent homes.
    identity: String,
    path: Option<PathBuf>,
}

impl SessionChatTranscriptGate {
    pub(crate) fn is_working(&mut self, session: &Value) -> bool {
        self.lifecycle(session)
            .is_some_and(|lifecycle| lifecycle.state == SessionChatTurnLifecycleState::Working)
    }

    /// The newest turn's lifecycle in the session's transcript, `None` without one.
    pub(crate) fn lifecycle(
        &mut self,
        session: &Value,
    ) -> Option<crate::session_chat::SessionChatTurnLifecycle> {
        let agent = session_text(session, "agentId").or_else(|| runtime_text(session, "agentName"));
        let agent_icon = session
            .get("launchSettings")
            .and_then(Value::as_object)
            .and_then(|settings| settings.get("icon"))
            .and_then(Value::as_str);
        let resolved_agent = agent
            .as_deref()
            .filter(|value| resolve_session_chat_transcript_agent(Some(value)).is_some())
            .or(agent_icon);
        let transcript_agent = resolve_session_chat_transcript_agent(resolved_agent)?;
        let agent_session_id = runtime_text(session, "agentSessionId");
        let agent_session_path = runtime_text(session, "agentSessionPath");
        let identity = format!(
            "{}|{}|{}",
            resolved_agent.unwrap_or_default(),
            agent_session_id.clone().unwrap_or_default(),
            agent_session_path.clone().unwrap_or_default(),
        );
        let cached = (self.identity == identity)
            .then(|| self.path.clone())
            .flatten()
            .filter(|path| path.is_file());
        let path = match cached {
            Some(path) => {
                // The Empryo chat is a mirror only a resolve or an open chat keeps current.
                if transcript_agent == crate::session_chat::SessionChatTranscriptAgent::Empryo {
                    crate::session_chat_empryo_mirror::sync_empryo_transcript_mirror_for_path(
                        &path,
                    );
                }
                Some(path)
            }
            None => resolve_session_chat_transcript_path(
                transcript_agent,
                agent_session_id.as_deref(),
                agent_session_path.as_deref(),
            ),
        };
        self.identity = identity;
        self.path = path.clone();
        // No transcript on disk yet: agent hooks are the only signal, and they already said idle.
        match read_session_chat_tail_page(
            transcript_agent,
            &path?,
            SESSION_CHAT_QUEUE_LIFECYCLE_TAIL_LIMIT,
            None,
        ) {
            Ok(SessionChatTailPage::Page { lifecycle, .. }) => lifecycle,
            _ => None,
        }
    }
}

struct ReadyDelivery {
    project_id: String,
    session_id: String,
    prompt_id: String,
    model_selection: Option<crate::session_chat_model_selection::PendingModelSelection>,
}

#[derive(Clone)]
pub struct SessionChatQueueRuntime {
    paths: GxserverPaths,
    server_id: String,
    sender_factory: SessionChatQueueSenderFactory,
    model_selector: crate::session_chat_model_selection::ModelSelectionSender,
    publisher_factory: SessionChatQueuePublisherFactory,
    notice_reader: SessionChatQueueNoticeReader,
    composer_reader: SessionChatQueueComposerReader,
    compacting_refresher: SessionChatQueueCompactingRefresher,
    gates: Arc<Mutex<HashMap<String, SessionQueueGate>>>,
    composer_holds: crate::session_chat_queue_undelivered::SessionChatQueueComposerHolds,
    /// Sessions with a delivery in flight. The claim in
    /// `deliver_session_chat_queued_prompt` is the real guard; this only keeps
    /// the scheduler from stacking tasks for the same session every second
    /// while a slow send (draft handshake, resume picker) is still running.
    delivering: Arc<Mutex<HashSet<String>>>,
}

impl SessionChatQueueRuntime {
    pub fn new(
        paths: GxserverPaths,
        server_id: impl Into<String>,
        sender_factory: SessionChatQueueSenderFactory,
        model_selector: crate::session_chat_model_selection::ModelSelectionSender,
        publisher_factory: SessionChatQueuePublisherFactory,
        notice_reader: SessionChatQueueNoticeReader,
        composer_reader: SessionChatQueueComposerReader,
        compacting_refresher: SessionChatQueueCompactingRefresher,
    ) -> Self {
        Self {
            paths,
            server_id: server_id.into(),
            sender_factory,
            model_selector,
            publisher_factory,
            notice_reader,
            composer_reader,
            compacting_refresher,
            gates: Arc::new(Mutex::new(HashMap::new())),
            composer_holds: Default::default(),
            delivering: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn start(&self, mut shutdown_rx: broadcast::Receiver<()>) {
        /*
        Restart recovery is NOT done here: `recover_session_chat_queue_after_restart`
        already runs once at server start and is idempotent. Rows left in
        `sending` become `failed` there and are never re-sent, because the bytes
        may already have reached the agent.
        */
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(SESSION_CHAT_QUEUE_TICK_SECONDS));
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => break,
                    _ = interval.tick() => runtime.run_tick().await,
                    _ = crate::session_chat_model_selection::selection_requested() => runtime.run_tick().await,
                }
            }
        });
    }

    async fn run_tick(&self) {
        // The readiness pass reads SQLite and transcripts (an Empryo transcript is rebuilt from
        // its raw log), so it runs on a blocking thread, never on an async worker.
        let runtime = self.clone();
        let Ok(deliveries) =
            tokio::task::spawn_blocking(move || runtime.collect_ready_deliveries()).await
        else {
            return;
        };
        for ready in deliveries {
            let key = session_queue_key(&ready.project_id, &ready.session_id);
            if !self.begin_delivery(&key) {
                continue;
            }
            let runtime = self.clone();
            tokio::spawn(async move {
                runtime.deliver(key, ready).await;
            });
        }
    }

    /*
    The whole readiness pass, synchronous so the SQLite connection is opened,
    used and dropped without ever crossing an await point.
    */
    fn collect_ready_deliveries(&self) -> Vec<ReadyDelivery> {
        let Ok(targets) = list_sessions_with_pending_queue(&self.paths) else {
            return Vec::new();
        };
        self.retain_gates(&targets);
        if targets.is_empty() {
            return Vec::new();
        }
        let Ok(db) = open_gxserver_database(&self.paths) else {
            return Vec::new();
        };
        let repository = DomainRepository::new(&db, self.server_id.as_str());
        let now = Utc::now();
        let generated_at = now.to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut ready: Vec<ReadyDelivery> = Vec::new();
        let mut blocked: Vec<(String, String, String, String)> = Vec::new();

        for (project_id, session_id) in targets {
            let key = session_queue_key(&project_id, &session_id);
            if self.is_delivering(&key) {
                // A send in flight is the busiest a session ever is.
                self.reset_gate(&key);
                continue;
            }
            let Ok(Some(session)) = repository.get_session(&project_id, &session_id) else {
                self.reset_gate(&key);
                continue;
            };
            /*
            A sleeping or stopped session keeps its queue rather than failing it:
            the rows drain once it is awake again. Writing into a dead provider
            would lose the text with nothing to show for it.
            */
            if session_text(&session, "lifecycleState").as_deref() != Some("running") {
                self.reset_gate(&key);
                continue;
            }
            // CDXC:SessionChat 2026-09-05 DECISION:
            // User: model changes run during a turn whenever the CLI accepts them; only actual delivery failure keeps them pending.
            // Prompt activity, transcript and stability gates below do not apply to model selection. The serialized driver checks a fresh terminal screen.
            let snapshot = read_session_chat_queue_snapshot_with(&db, &project_id, &session_id);
            if let Some(selection) = snapshot.pending_model_selection.as_ref() {
                // A failed selection is never redelivered, and it keeps holding the prompts behind
                // it: they were written for the model it asked for (2026-09-27 decision in
                // session_chat_model_selection_alert.rs). Picking a model again releases them.
                if selection.state == "failed" {
                    self.reset_gate(&key);
                    continue;
                }
                if selection.retry_at > now.timestamp_millis() {
                    continue;
                }
                ready.push(ReadyDelivery {
                    project_id,
                    session_id,
                    prompt_id: String::new(),
                    model_selection: Some(selection.clone()),
                });
                continue;
            }
            // CDXC:SessionChat 2026-09-09 SEE-ALSO:
            // sendSessionChatMessage accepts new-chat sends before the provider exists. Probe startup independently of activity and require positive input-box evidence before the first delivery.
            if snapshot.deliverable_head().is_none() {
                self.reset_gate(&key);
                continue;
            }
            // CDXC:SessionChat 2026-09-25 WHY:
            // A send held for a session that was asleep or still starting (session_chat_send_wake.rs) waits exactly like a new chat's first message: nobody may be viewing the session, so the scheduler refreshes the screen itself and delivers the moment the input box appears. Waiting for the cached reading made a woken session take 40 seconds.
            let awaiting_startup = crate::agents::session_is_draft(&session)
                || snapshot
                    .deliverable_head()
                    .is_some_and(|head| head.startup_send);
            let composer_agent =
                crate::session_chat_composer::session_chat_composer_agent_id(&session);
            let composer = (self.composer_reader)(&project_id, &session_id);
            // A draft whose Run on row picked a box has no input box to wait for: delivering its
            // first queued prompt starts the box (agents/draft_run_location.rs).
            let pending_box = crate::agentbox::pending_session_agentbox(&session).is_some();
            let awaiting_startup = awaiting_startup && !pending_box;
            if awaiting_startup
                && crate::session_chat_composer::has_session_chat_composer_signature(
                    composer_agent.as_deref(),
                )
                && composer.state != crate::session_chat_composer::SessionChatComposerState::Ready
            {
                self.reset_gate(&key);
                (self.compacting_refresher)(&project_id, &session_id, composer_agent.as_deref());
                continue;
            }
            /*
            A compacting marker is written by the same whole zmx screen capture
            that feeds chat's progress card. Refresh only this marked state so
            a queued prompt can leave as soon as compaction disappears even if
            every client has disconnected. Failed/capped captures do not clear
            the marker, so uncertainty always holds the prompt safely.
            */
            if crate::session_chat_compacting::session_chat_compacting_detected_at(&session)
                .is_some()
            {
                let agent = session_chat_agent_for_session(&session);
                (self.compacting_refresher)(&project_id, &session_id, agent.as_deref());
                self.reset_gate(&key);
                continue;
            }
            /*
            "working" is obvious. "attention" is the load-bearing one: the agent
            is sitting on a permission/approval prompt, and a prompt delivered
            now becomes the ANSWER to it.

            CDXC:SessionChat 2026-09-04 DECISION:
            User: Claude's Stop now rings attention like Codex's, and queued
            prompts must keep draining unattended. The one attention the queue
            may deliver into is the finished turn a hook's Stop entered
            (`attentionSource: turnComplete`) with no question or approval card
            standing; that is exactly the "next stop" this clock waits for.
            Every other attention still holds the row.
            */
            if matches!(
                session
                    .pointer("/runtimeSettings/accountRecovery/status")
                    .and_then(Value::as_str),
                Some("waiting" | "retrying" | "needsAttention")
            ) {
                self.reset_gate(&key);
                continue;
            }
            if !session_chat_queue_activity_is_deliverable(&session, &generated_at) {
                self.reset_gate(&key);
                continue;
            }
            let startup_composer_ready = awaiting_startup
                && composer.state == crate::session_chat_composer::SessionChatComposerState::Ready;
            // CDXC:SessionChat 2026-09-26 WHY:
            // An agent that died mid-turn leaves its transcript ending in a tool result with no reply, which reads as Working forever. A freshly started CLI showing an empty input box cannot be mid-turn, so a startup send trusts the screen over that dead turn; checking the transcript here left a woken session's "continue" queued indefinitely.
            if !startup_composer_ready && self.transcript_lifecycle_is_working(&key, &session) {
                self.reset_gate(&key);
                continue;
            }
            if !startup_composer_ready && !self.stability_window_elapsed(&key, now) {
                continue;
            }
            let Some(head) = snapshot.deliverable_head() else {
                continue;
            };
            if self.composer_holds.waiting(&key, &head.id, now) {
                continue;
            }
            /*
            A trust dialog, a first-run setup screen, an update modal, a usage
            limit waiting on a keypress, an expired login, the agent process
            gone, a delivery the watchdog could not prove: in every one of
            those the terminal does not pass a prompt to the model, and several
            of them consume it as the answer to what is on screen. Hold the row
            with the notice title as its reason so the stall is VISIBLE and
            retryable — a queue that silently waits forever is the failure mode
            of this rule, not its goal.

            Gating on severity was the original bug: the catalog rates a trust
            prompt `Warning` and onboarding `Info` precisely because the user
            is one keypress from continuing, which says nothing about whether a
            prompt sent meanwhile survives.
            */
            if let Some(notice) = (self.notice_reader)(&project_id, &session_id) {
                if crate::accounts::recovery::holds_queue(&repository, &session, &notice) {
                    self.reset_gate(&key);
                    continue;
                }
                // CDXC:SessionChat 2026-10-09 WHY: an exited agent is restarted by the send itself and the row waits for it (the 2026-10-09 DECISION in send_heal.rs), so the row is delivered instead of failed with "no longer running".
                if notice.kind == crate::session_chat_notice::SESSION_CHAT_NOTICE_AGENT_EXITED {
                    ready.push(ReadyDelivery {
                        project_id,
                        session_id,
                        prompt_id: head.id.clone(),
                        model_selection: None,
                    });
                    continue;
                }
                if notice.blocks_queued_delivery()
                    && !escape_closes_claude_panel(&composer, &notice)
                {
                    blocked.push((
                        project_id,
                        session_id,
                        head.id.clone(),
                        notice.title.clone(),
                    ));
                    continue;
                }
            }
            /*
            CDXC:SessionChat 2026-08-26:
            No input box on screen ⇒ HOLD, and deliberately not the `blocked`
            treatment above. A blocking notice is a state only the user can
            leave — a trust dialog waits forever for a keypress — so failing the
            row makes the stall visible and retryable. A missing composer is the
            opposite: it is what a CLI looks like while it BOOTS, and it clears
            on its own within seconds. Burning the head row for that would fail
            every queued prompt on a session that was merely restarted, which is
            precisely the moment a queue exists to cover.

            Grok requires positive readiness; unmeasured agents retain their
            existing Unknown behavior.
            */
            if !pending_box
                && composer.blocks_message_for(
                    crate::session_chat_composer::session_chat_composer_agent_id(&session)
                        .as_deref(),
                )
                && !composer.should_dismiss()
            {
                self.reset_gate(&key);
                continue;
            }
            ready.push(ReadyDelivery {
                project_id,
                session_id,
                prompt_id: head.id.clone(),
                model_selection: None,
            });
        }
        drop(repository);
        drop(db);

        for (project_id, session_id, prompt_id, reason) in blocked {
            if let Ok(snapshot) = fail_session_chat_queued_prompt(
                &self.paths,
                &project_id,
                &session_id,
                &prompt_id,
                &reason,
            ) {
                (self.publisher_factory)(&project_id, &session_id)();
                if let Some(prompt) = snapshot.queue.iter().find(|prompt| prompt.id == prompt_id) {
                    self.notify_sender(&project_id, &session_id, &prompt.text, &reason);
                }
            }
            self.reset_gate(&session_queue_key(&project_id, &session_id));
        }
        ready
    }

    async fn deliver(&self, key: String, ready: ReadyDelivery) {
        if let Some(selection) = ready.model_selection {
            (self.model_selector)(ready.project_id, ready.session_id, selection).await;
            self.reset_gate(&key);
            self.finish_delivery(&key);
            return;
        }
        let sender = (self.sender_factory)(&ready.project_id, &ready.session_id);
        /*
        The shared claim → send → settle path. It deletes the row on success and
        marks it `failed` with the reason on error, so "Send now" and the
        scheduler can never both deliver the same row.
        */
        let may_hold = self
            .composer_holds
            .may_hold(&key, &ready.prompt_id, Utc::now());
        let delivered = deliver_session_chat_queued_prompt(
            &self.paths,
            &self.server_id,
            &ready.project_id,
            &ready.session_id,
            &ready.prompt_id,
            &sender,
            may_hold,
        )
        .await;
        if let Ok(delivery) = &delivered {
            (self.publisher_factory)(&ready.project_id, &ready.session_id)();
            if delivery.held {
                self.composer_holds
                    .record(&key, &ready.prompt_id, Utc::now());
            } else {
                self.composer_holds.clear(&key);
            }
            if let Some(reason) = delivery.error_message.as_deref() {
                if let Some(prompt) = delivery
                    .snapshot
                    .queue
                    .iter()
                    .find(|prompt| prompt.id == ready.prompt_id)
                {
                    self.notify_sender(&ready.project_id, &ready.session_id, &prompt.text, reason);
                }
            }
        }
        /*
        ONE prompt per idle window, whatever happened: a fresh stability window
        has to elapse before the next row is even considered.
        */
        self.reset_gate(&key);
        self.finish_delivery(&key);
    }

    /// The transcript read happens outside the `gates` lock, which deliveries also take.
    fn transcript_lifecycle_is_working(&self, key: &str, session: &Value) -> bool {
        let mut transcript = {
            let Ok(mut gates) = self.gates.lock() else {
                return true;
            };
            std::mem::take(&mut gates.entry(key.to_string()).or_default().transcript)
        };
        let working = transcript.is_working(session);
        if let Ok(mut gates) = self.gates.lock() {
            gates.entry(key.to_string()).or_default().transcript = transcript;
        }
        working
    }

    fn stability_window_elapsed(&self, key: &str, now: DateTime<Utc>) -> bool {
        let Ok(mut gates) = self.gates.lock() else {
            return false;
        };
        let gate = gates.entry(key.to_string()).or_default();
        match gate.stopped_since {
            Some(since) => {
                now.signed_duration_since(since).num_milliseconds()
                    >= SESSION_CHAT_QUEUE_STABILITY_MS
            }
            None => {
                gate.stopped_since = Some(now);
                false
            }
        }
    }

    fn reset_gate(&self, key: &str) {
        if let Ok(mut gates) = self.gates.lock() {
            if let Some(gate) = gates.get_mut(key) {
                gate.stopped_since = None;
            }
        }
    }

    /// Drops the in-memory window for sessions whose queue has gone empty, so a
    /// long-lived daemon does not accumulate a gate per session it ever queued
    /// a prompt for.
    fn retain_gates(&self, targets: &[(String, String)]) {
        let Ok(mut gates) = self.gates.lock() else {
            return;
        };
        if gates.is_empty() {
            return;
        }
        let live: HashSet<String> = targets
            .iter()
            .map(|(project_id, session_id)| session_queue_key(project_id, session_id))
            .collect();
        gates.retain(|key, _| live.contains(key));
        self.composer_holds.retain(|key| live.contains(key));
    }

    fn notify_sender(&self, project_id: &str, session_id: &str, text: &str, reason: &str) {
        if let Some((sender_project, sender_session)) =
            crate::session_chat_queue_undelivered::notify_sender_of_undelivered_agent_message(
                &self.paths,
                &self.server_id,
                project_id,
                session_id,
                text,
                reason,
            )
        {
            (self.publisher_factory)(&sender_project, &sender_session)();
        }
    }

    fn is_delivering(&self, key: &str) -> bool {
        self.delivering
            .lock()
            .map(|delivering| delivering.contains(key))
            .unwrap_or(true)
    }

    fn begin_delivery(&self, key: &str) -> bool {
        self.delivering
            .lock()
            .map(|mut delivering| delivering.insert(key.to_string()))
            .unwrap_or(false)
    }

    fn finish_delivery(&self, key: &str) {
        if let Ok(mut delivering) = self.delivering.lock() {
            delivering.remove(key);
        }
    }
}

fn session_queue_key(project_id: &str, session_id: &str) -> String {
    format!("{project_id}\u{1f}{session_id}")
}

/// Idle, or a finished turn's attention with no interactive card pending.
/// Working and every other attention hold the queue.
fn session_chat_queue_activity_is_deliverable(session: &Value, generated_at: &str) -> bool {
    match presentation_activity(session, generated_at).as_str() {
        "idle" => true,
        "attention" => {
            let activity = session
                .get("runtimeSettings")
                .and_then(Value::as_object)
                .and_then(|settings| settings.get("agentActivity"));
            is_turn_complete_attention(activity)
                && crate::agents::session_chat_prompt_setting(session).is_none()
        }
        _ => false,
    }
}

fn session_text(session: &Value, key: &str) -> Option<String> {
    session
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn runtime_text(session: &Value, key: &str) -> Option<String> {
    session
        .get("runtimeSettings")
        .and_then(Value::as_object)
        .and_then(|settings| settings.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// An Escape-safe Claude panel (its Settings screen, or an offer Claude opened by itself) that the
/// notice classifier also reads as an input-blocking dialog. The send closes it with Escape instead
/// of handing it to the user as a question or failing a queued row on it.
pub(super) fn escape_closes_claude_panel(
    composer: &crate::session_chat_composer::SessionChatComposerReadiness,
    notice: &crate::session_chat_notice::SessionChatTerminalNotice,
) -> bool {
    composer.should_dismiss()
        && notice.kind == crate::session_chat_notice::SESSION_CHAT_NOTICE_CLAUDE_INPUT_BLOCKED
}
