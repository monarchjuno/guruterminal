use super::*;

fn finance_executor(temporary: &tempfile::TempDir) -> AppToolExecutor {
    let mut executor = bare_executor(temporary);
    executor
        .capability_ids
        .insert("guruterminal.finance-core".into());
    executor
}

fn finance_policy() -> ToolPolicy {
    ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    }
}

fn execution_message(error: BrokerError) -> String {
    match error {
        BrokerError::Execution(message) => message,
        other => panic!("expected Execution, got {other:?}"),
    }
}

#[tokio::test]
async fn percentage_change_with_unit_is_rejected_before_the_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let error = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operation": "percentage_change",
                "arguments": {
                    "start": "80",
                    "end": "100",
                    "unit": "percent"
                }
            }),
            "delivery-1",
        )
        .await
        .unwrap_err();
    let message = execution_message(error);
    assert!(
        message.contains("percentage_change") && message.contains("unit"),
        "agent-facing message must name the operation and unsupported key: {message}"
    );
    assert!(
        !message.contains("malformed_request"),
        "finance_calculate must not hide the field in Malformed: {message}"
    );
}

#[tokio::test]
async fn percentage_change_with_grouped_decimals_names_the_field() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let error = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operation": "percentage_change",
                "arguments": {
                    "start": "69,154",
                    "end": "75210"
                }
            }),
            "delivery-1",
        )
        .await
        .unwrap_err();
    let message = execution_message(error);
    assert!(
        message.contains("arguments.start") && message.contains("grouping commas"),
        "agent-facing message must name the grouped decimal field: {message}"
    );
}

#[tokio::test]
async fn extra_top_level_fields_name_the_rejected_key() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let error = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operation": "percentage_change",
                "arguments": { "start": "80", "end": "100" },
                "unit": "percent"
            }),
            "delivery-1",
        )
        .await
        .unwrap_err();
    let message = execution_message(error);
    assert!(
        message.contains("unit"),
        "agent-facing message must name the extra top-level field: {message}"
    );
}

#[tokio::test]
async fn non_object_arguments_are_named_execution_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let error = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operation": "ratio",
                "arguments": ["1", "2"]
            }),
            "delivery-1",
        )
        .await
        .unwrap_err();
    let message = execution_message(error);
    assert!(
        message.contains("arguments") && message.contains("object"),
        "agent-facing message must say arguments must be an object: {message}"
    );
}

#[tokio::test]
async fn unknown_operation_stays_method_denied() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let error = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operation": "npv",
                "arguments": { "start": "1", "end": "2" }
            }),
            "delivery-1",
        )
        .await
        .unwrap_err();
    assert!(matches!(error, BrokerError::MethodDenied));
}
