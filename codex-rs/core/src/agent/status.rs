use codex_protocol::error::redact_cyber_policy_message;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::EventMsg;

/// Derive the next agent status from a single emitted event.
/// Returns `None` when the event does not affect status tracking.
pub(crate) fn agent_status_from_event(msg: &EventMsg) -> Option<AgentStatus> {
    match msg {
        EventMsg::TurnStarted(_) => Some(AgentStatus::Running),
        EventMsg::TurnComplete(ev) => Some(match &ev.error {
            Some(error) => AgentStatus::Errored(sanitize_message(&error.message)),
            None => AgentStatus::Completed(ev.last_agent_message.as_deref().map(sanitize_message)),
        }),
        EventMsg::TurnAborted(ev) => match ev.reason {
            codex_protocol::protocol::TurnAbortReason::Interrupted
            | codex_protocol::protocol::TurnAbortReason::BudgetLimited => {
                Some(AgentStatus::Interrupted)
            }
            _ => Some(AgentStatus::Errored(format!("{:?}", ev.reason))),
        },
        // Auto-continue interruptions do not affect turn status; the agent is still running.
        EventMsg::Error(ev) if ev.affects_turn_status() => {
            Some(AgentStatus::Errored(sanitize_message(&ev.message)))
        }
        EventMsg::Error(_) => None,
        EventMsg::ShutdownComplete => Some(AgentStatus::Shutdown),
        _ => None,
    }
}

fn sanitize_message(message: &str) -> String {
    redact_cyber_policy_message(message).unwrap_or_else(|| message.to_string())
}

pub(crate) fn is_final(status: &AgentStatus) -> bool {
    !matches!(
        status,
        AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Interrupted
    )
}
