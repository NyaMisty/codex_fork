use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::CodexThread;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;

pub(super) const DIRECT_INPUT_TO_MULTI_AGENT_V2_SUBAGENT_ERROR: &str =
    "direct app-server input is not allowed for multi-agent v2 sub-agents";

/// Mirrors the ownership policy in thread capability responses and resume validation.
pub(super) fn can_accept_direct_input(
    multi_agent_version: Option<MultiAgentVersion>,
    session_source: &SessionSource,
) -> bool {
    multi_agent_version != Some(MultiAgentVersion::V2)
        || !matches!(
            session_source,
            SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. })
        )
}

pub(super) async fn ensure_direct_input_allowed(
    _thread: &CodexThread,
) -> Result<(), JSONRPCErrorError> {
    // Loaded sub-agent threads intentionally accept direct user input so the TUI can talk
    // to an agent while viewing it. Ownership remains enforced for resume and queue lifecycle.
    Ok(())
}
