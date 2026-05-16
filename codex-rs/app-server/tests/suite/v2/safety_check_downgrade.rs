use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::ModelVerification;
use codex_app_server_protocol::ModelVerificationNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::ResponseTemplate;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const REQUESTED_MODEL: &str = "gpt-5.4";
const SERVER_MODEL: &str = "gpt-5.3-codex";
const TRUSTED_ACCESS_FOR_CYBER_VERIFICATION: &str = "trusted_access_for_cyber";
const CYBER_POLICY_MESSAGE: &str =
    "This request has been flagged for potentially high-risk cyber activity.";

#[tokio::test]
async fn openai_model_header_mismatch_retries_on_server_model_without_warning_v2() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let first_body = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ]);
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_response_sequence(
        &server,
        vec![
            responses::sse_response(first_body).insert_header("OpenAI-Model", SERVER_MODEL),
            responses::sse_response(second_body).insert_header("OpenAI-Model", SERVER_MODEL),
        ],
    )
    .await;

    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some(REQUESTED_MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse {
        thread: _thread, ..
    } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "trigger safeguard".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let _turn_start: TurnStartResponse = to_response(turn_resp)?;

    collect_turn_notifications_without_error_or_warning(&mut mcp).await?;
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body_json()["model"].as_str(),
        Some(REQUESTED_MODEL)
    );
    assert_eq!(
        requests[1].body_json()["model"].as_str(),
        Some(SERVER_MODEL)
    );

    Ok(())
}

#[tokio::test]
async fn cyber_policy_response_retries_with_default_fallback_without_error_v2() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let first_response = ResponseTemplate::new(400).set_body_json(serde_json::json!({
        "error": {
            "message": CYBER_POLICY_MESSAGE,
            "type": "invalid_request",
            "param": null,
            "code": "cyber_policy"
        }
    }));
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_response_sequence(
        &server,
        vec![first_response, responses::sse_response(second_body)],
    )
    .await;

    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some(REQUESTED_MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse {
        thread: _thread, ..
    } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "trigger cyber policy error".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let _turn_start: TurnStartResponse = to_response(turn_resp)?;

    collect_turn_notifications_without_error_or_warning(&mut mcp).await?;
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body_json()["model"].as_str(),
        Some(REQUESTED_MODEL)
    );
    assert_eq!(requests[1].body_json()["model"].as_str(), Some("gpt-5.2"));

    Ok(())
}

#[tokio::test]
async fn response_model_field_mismatch_retries_without_warning_v2_when_header_matches_requested()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let first_body = responses::sse(vec![
        serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp-1",
                "headers": {
                    "OpenAI-Model": SERVER_MODEL
                }
            }
        }),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ]);
    let second_body = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_response_sequence(
        &server,
        vec![
            responses::sse_response(first_body).insert_header("OpenAI-Model", REQUESTED_MODEL),
            responses::sse_response(second_body).insert_header("OpenAI-Model", SERVER_MODEL),
        ],
    )
    .await;

    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some(REQUESTED_MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "trigger response model check".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let _turn_start: TurnStartResponse = to_response(turn_resp)?;

    collect_turn_notifications_without_error_or_warning(&mut mcp).await?;
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body_json()["model"].as_str(),
        Some(REQUESTED_MODEL)
    );
    assert_eq!(
        requests[1].body_json()["model"].as_str(),
        Some(SERVER_MODEL)
    );

    Ok(())
}

#[tokio::test]
async fn model_verification_emits_typed_notification_and_warning_v2() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let body = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_model_verification_metadata(
            "resp-1",
            vec![TRUSTED_ACCESS_FOR_CYBER_VERIFICATION],
        ),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ]);
    let response = responses::sse_response(body);
    let _response_mock = responses::mount_response_once(&server, response).await;

    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some(REQUESTED_MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "trigger model verification".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let turn_start: TurnStartResponse = to_response(turn_resp)?;

    let verification =
        collect_model_verification_notifications_and_validate_no_warning_item(&mut mcp).await?;
    assert_eq!(
        verification,
        ModelVerificationNotification {
            thread_id: thread.id,
            turn_id: turn_start.turn.id,
            verifications: vec![ModelVerification::TrustedAccessForCyber],
        }
    );

    Ok(())
}

async fn collect_turn_notifications_without_error_or_warning(mcp: &mut McpProcess) -> Result<()> {
    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        match notification.method.as_str() {
            "item/started" => {
                let params = notification.params.ok_or_else(|| {
                    anyhow::anyhow!("item/started notifications must include params")
                })?;
                let payload: ItemStartedNotification = serde_json::from_value(params)?;
                assert!(!is_warning_user_message_item(&payload.item));
            }
            "item/completed" => {
                let params = notification.params.ok_or_else(|| {
                    anyhow::anyhow!("item/completed notifications must include params")
                })?;
                let payload: ItemCompletedNotification = serde_json::from_value(params)?;
                assert!(!is_warning_user_message_item(&payload.item));
            }
            "warning" => {
                anyhow::bail!("cyber safety response must not emit warning");
            }
            "model/rerouted" => {
                anyhow::bail!("cyber safety response must not emit model/rerouted");
            }
            "error" => {
                anyhow::bail!("cyber safety response must not emit error");
            }
            "turn/completed" => {
                return Ok(());
            }
            _ => {}
        }
    }
}

async fn collect_model_verification_notifications_and_validate_no_warning_item(
    mcp: &mut McpProcess,
) -> Result<ModelVerificationNotification> {
    let mut verification = None;

    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        match notification.method.as_str() {
            "model/verification" => {
                let params = notification.params.ok_or_else(|| {
                    anyhow::anyhow!("model/verification notifications must include params")
                })?;
                let payload: ModelVerificationNotification = serde_json::from_value(params)?;
                verification = Some(payload);
            }
            "warning" => {
                anyhow::bail!("verification-only response must not emit warning");
            }
            "model/rerouted" => {
                anyhow::bail!("verification-only response must not emit model/rerouted");
            }
            "item/started" => {
                let params = notification.params.ok_or_else(|| {
                    anyhow::anyhow!("item/started notifications must include params")
                })?;
                let payload: ItemStartedNotification = serde_json::from_value(params)?;
                assert!(!is_warning_user_message_item(&payload.item));
            }
            "item/completed" => {
                let params = notification.params.ok_or_else(|| {
                    anyhow::anyhow!("item/completed notifications must include params")
                })?;
                let payload: ItemCompletedNotification = serde_json::from_value(params)?;
                assert!(!is_warning_user_message_item(&payload.item));
            }
            "turn/completed" => {
                let verification = verification.ok_or_else(|| {
                    anyhow::anyhow!(
                        "expected model/verification notification before turn/completed"
                    )
                })?;
                return Ok(verification);
            }
            _ => {}
        }
    }
}

fn warning_text_from_item(item: &ThreadItem) -> Option<&str> {
    let ThreadItem::UserMessage { content, .. } = item else {
        return None;
    };

    content.iter().find_map(|input| match input {
        UserInput::Text { text, .. } if text.starts_with("Warning: ") => Some(text.as_str()),
        _ => None,
    })
}

fn is_warning_user_message_item(item: &ThreadItem) -> bool {
    warning_text_from_item(item).is_some()
}

fn create_config_toml(codex_home: &std::path::Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "{REQUESTED_MODEL}"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[features]
remote_models = false
personality = true

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
