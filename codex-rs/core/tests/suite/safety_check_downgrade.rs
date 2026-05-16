use anyhow::Result;
use codex_protocol::models::ContentItem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ModelVerification;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_model_verification_metadata;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_once;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_completed;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use wiremock::ResponseTemplate;

const SERVER_MODEL: &str = "gpt-5.2";
const REQUESTED_MODEL: &str = "gpt-5.3-codex";
const DIRECT_CYBER_POLICY_FALLBACK_MODEL: &str = "gpt-5.2";
const TRUSTED_ACCESS_FOR_CYBER_VERIFICATION: &str = "trusted_access_for_cyber";

const CYBER_POLICY_MESSAGE: &str =
    "This request has been flagged for potentially high-risk cyber activity.";

fn disabled_text_turn(test: &TestCodex, text: &str) -> Op {
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.cwd_path());
    Op::UserTurn {
        environments: None,
        items: vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        cwd: test.cwd_path().to_path_buf(),
        approval_policy: AskForApproval::Never,
        approvals_reviewer: None,
        sandbox_policy,
        permission_profile,
        model: REQUESTED_MODEL.to_string(),
        effort: test.config.model_reasoning_effort,
        summary: None,
        service_tier: None,
        collaboration_mode: None,
        personality: None,
    }
}

async fn assert_no_error_reroute_or_warning_until_turn_complete(test: &TestCodex) {
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::Error(error) => panic!("unexpected error event: {error:?}"),
            EventMsg::ModelReroute(reroute) => {
                panic!("unexpected model reroute event: {reroute:?}")
            }
            EventMsg::Warning(warning) => panic!("unexpected warning event: {warning:?}"),
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_model_header_mismatch_retries_on_server_model_without_warning() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let first_response =
        sse_response(sse_completed("resp-1")).insert_header("OpenAI-Model", SERVER_MODEL);
    let second_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        core_test_support::responses::ev_completed("resp-2"),
    ]))
    .insert_header("OpenAI-Model", SERVER_MODEL);
    let mock = mount_response_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger safety check"))
        .await?;

    assert_no_error_reroute_or_warning_until_turn_complete(&test).await;
    let requests = mock.requests();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cyber_policy_response_retries_with_default_fallback_without_error() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let first_response = ResponseTemplate::new(400).set_body_json(serde_json::json!({
        "error": {
            "message": CYBER_POLICY_MESSAGE,
            "type": "invalid_request",
            "param": null,
            "code": "cyber_policy"
        }
    }));
    let second_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        core_test_support::responses::ev_completed("resp-2"),
    ]));
    let mock = mount_response_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger cyber policy error"))
        .await?;

    assert_no_error_reroute_or_warning_until_turn_complete(&test).await;
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body_json()["model"].as_str(),
        Some(REQUESTED_MODEL)
    );
    assert_eq!(
        requests[1].body_json()["model"].as_str(),
        Some(DIRECT_CYBER_POLICY_FALLBACK_MODEL)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_model_field_mismatch_retries_without_warning_when_header_matches_requested()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let first_response = sse_response(sse(vec![
        serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp-1",
                "headers": {
                    "OpenAI-Model": SERVER_MODEL
                }
            }
        }),
        core_test_support::responses::ev_completed("resp-1"),
    ]))
    .insert_header("OpenAI-Model", REQUESTED_MODEL);
    let second_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        core_test_support::responses::ev_completed("resp-2"),
    ]))
    .insert_header("OpenAI-Model", SERVER_MODEL);
    let mock = mount_response_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger response model check"))
        .await?;

    assert_no_error_reroute_or_warning_until_turn_complete(&test).await;
    let requests = mock.requests();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_model_header_mismatch_retry_does_not_record_failed_response_items() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let tool_args = serde_json::json!({
        "command": "echo hello",
        "timeout_ms": 1_000
    });

    let first_response = sse_response(sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(
            "call-1",
            "shell_command",
            &serde_json::to_string(&tool_args)?,
        ),
        core_test_support::responses::ev_completed("resp-1"),
    ]))
    .insert_header("OpenAI-Model", SERVER_MODEL);
    let second_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        core_test_support::responses::ev_completed("resp-2"),
    ]))
    .insert_header("OpenAI-Model", SERVER_MODEL);
    let mock = mount_response_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger follow-up turn"))
        .await?;

    assert_no_error_reroute_or_warning_until_turn_complete(&test).await;
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert!(mock.function_call_output_text("call-1").is_none());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cyber_policy_response_emits_typed_error_after_retry_budget_exhausted() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let cyber_response = || {
        ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {
                "message": CYBER_POLICY_MESSAGE,
                "type": "invalid_request",
                "param": null,
                "code": "cyber_policy"
            }
        }))
    };
    let mock = mount_response_sequence(&server, vec![cyber_response(), cyber_response()]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(
            &test,
            "trigger repeated cyber policy error",
        ))
        .await?;

    let mut reroute_count = 0;
    let mut warning_count = 0;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::ModelReroute(_) => reroute_count += 1,
            EventMsg::Warning(_) => warning_count += 1,
            EventMsg::Error(error) => {
                assert_eq!(error.message, CYBER_POLICY_MESSAGE);
                assert_eq!(error.codex_error_info, Some(CodexErrorInfo::CyberPolicy));
                break;
            }
            _ => {}
        }
    }

    assert_eq!(reroute_count, 0);
    assert_eq!(warning_count, 0);
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body_json()["model"].as_str(),
        Some(REQUESTED_MODEL)
    );
    assert_eq!(
        requests[1].body_json()["model"].as_str(),
        Some(DIRECT_CYBER_POLICY_FALLBACK_MODEL)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_model_header_casing_only_mismatch_does_not_warn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let requested_header = REQUESTED_MODEL.to_ascii_uppercase();
    let response = sse_response(sse_completed("resp-1"))
        .insert_header("OpenAI-Model", requested_header.as_str());
    let _mock = mount_response_once(&server, response).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger casing check"))
        .await?;

    let mut reroute_count = 0;
    let mut warning_count = 0;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::ModelReroute(_) => reroute_count += 1,
            EventMsg::Warning(warning)
                if warning
                    .message
                    .contains("flagged for potentially high-risk cyber activity") =>
            {
                warning_count += 1;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    assert_eq!(reroute_count, 0);
    assert_eq!(warning_count, 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_verification_emits_structured_event_without_reroute_or_warning() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = sse_response(sse(vec![
        ev_response_created("resp-1"),
        ev_model_verification_metadata("resp-1", vec![TRUSTED_ACCESS_FOR_CYBER_VERIFICATION]),
        core_test_support::responses::ev_completed("resp-1"),
    ]));
    let _mock = mount_response_once(&server, response).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(&test, "trigger model verification"))
        .await?;

    let mut verification_count = 0;
    let mut reroute_count = 0;
    let mut warning_count = 0;
    let mut warning_item_count = 0;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::ModelVerification(event) => {
                assert_eq!(
                    event.verifications,
                    vec![ModelVerification::TrustedAccessForCyber]
                );
                verification_count += 1;
            }
            EventMsg::Warning(_) => warning_count += 1,
            EventMsg::ModelReroute(_) => reroute_count += 1,
            EventMsg::RawResponseItem(raw)
                if matches!(
                    &raw.item,
                    ResponseItem::Message { content, .. }
                        if content.iter().any(|item| matches!(
                            item,
                            ContentItem::InputText { text } if text.starts_with("Warning: ")
                        ))
                ) =>
            {
                warning_item_count += 1;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    assert_eq!(verification_count, 1);
    assert_eq!(reroute_count, 0);
    assert_eq!(warning_count, 0);
    assert_eq!(warning_item_count, 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_verification_only_emits_once_per_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let tool_args = serde_json::json!({
        "command": "echo hello",
        "timeout_ms": 1_000
    });

    let first_response = sse_response(sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(
            "call-1",
            "shell_command",
            &serde_json::to_string(&tool_args)?,
        ),
        ev_model_verification_metadata("resp-1", vec![TRUSTED_ACCESS_FOR_CYBER_VERIFICATION]),
        core_test_support::responses::ev_completed("resp-1"),
    ]));
    let second_response = sse_response(sse(vec![
        ev_response_created("resp-2"),
        ev_model_verification_metadata("resp-2", vec![TRUSTED_ACCESS_FOR_CYBER_VERIFICATION]),
        ev_assistant_message("msg-1", "done"),
        core_test_support::responses::ev_completed("resp-2"),
    ]));
    let _mock = mount_response_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_codex().with_model(REQUESTED_MODEL);
    let test = builder.build(&server).await?;

    test.codex
        .submit(disabled_text_turn(
            &test,
            "trigger follow-up model verification",
        ))
        .await?;

    let mut verification_count = 0;
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        match event {
            EventMsg::ModelVerification(_) => verification_count += 1,
            EventMsg::Warning(warning) if warning.message.contains("high-risk cyber activity") => {
                panic!("model verification should not emit a warning event");
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    assert_eq!(verification_count, 1);

    Ok(())
}
