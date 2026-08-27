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

fn batch_item(id: &str, operation: &str, arguments: Value) -> Value {
    json!({
        "operations": [{
            "id": id,
            "operation": operation,
            "arguments": arguments,
        }]
    })
}

fn batch_item_error(output: &Value) -> (&str, &str) {
    let item = &output["results"][0];
    assert_eq!(item["ok"], false);
    (
        item["error"]["code"].as_str().unwrap(),
        item["error"]["message"].as_str().unwrap(),
    )
}

#[tokio::test]
async fn percentage_change_with_unit_is_rejected_before_the_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let output = executor
        .finance_calculate(
            &finance_policy(),
            batch_item(
                "change",
                "percentage_change",
                json!({ "start": "80", "end": "100", "unit": "percent" }),
            ),
            "delivery-1",
        )
        .await
        .unwrap();
    let (code, message) = batch_item_error(&output);
    assert_eq!(code, "invalid_arguments");
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
    let output = executor
        .finance_calculate(
            &finance_policy(),
            batch_item(
                "change",
                "percentage_change",
                json!({ "start": "69,154", "end": "75210" }),
            ),
            "delivery-1",
        )
        .await
        .unwrap();
    let (code, message) = batch_item_error(&output);
    assert_eq!(code, "invalid_arguments");
    assert!(
        message.contains("arguments.start") && message.contains("grouping commas"),
        "agent-facing message must name the grouped decimal field: {message}"
    );
}

#[tokio::test]
async fn extra_top_level_fields_name_the_rejected_key() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let output = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operations": [{
                    "id": "change",
                    "operation": "percentage_change",
                    "arguments": { "start": "80", "end": "100" },
                    "unit": "percent"
                }]
            }),
            "delivery-1",
        )
        .await
        .unwrap();
    let (code, message) = batch_item_error(&output);
    assert_eq!(code, "invalid_request");
    assert!(
        message.contains("unit"),
        "agent-facing message must name the extra top-level field: {message}"
    );
}

#[tokio::test]
async fn non_object_arguments_are_named_execution_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let output = executor
        .finance_calculate(
            &finance_policy(),
            batch_item("ratio", "ratio", json!(["1", "2"])),
            "delivery-1",
        )
        .await
        .unwrap();
    let (code, message) = batch_item_error(&output);
    assert_eq!(code, "invalid_arguments");
    assert!(
        message.contains("arguments") && message.contains("object"),
        "agent-facing message must say arguments must be an object: {message}"
    );
}

#[tokio::test]
async fn unknown_operation_is_an_item_scoped_denial() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let output = executor
        .finance_calculate(
            &finance_policy(),
            batch_item("npv", "npv", json!({ "start": "1", "end": "2" })),
            "delivery-1",
        )
        .await
        .unwrap();
    let (code, message) = batch_item_error(&output);
    assert_eq!(code, "operation_denied");
    assert!(message.contains("operations[0].operation"));
}

#[tokio::test]
async fn batch_size_is_bounded_before_the_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    for operations in [Vec::new(), vec![Value::Null; 65]] {
        let error = executor
            .finance_calculate(
                &finance_policy(),
                json!({ "operations": operations }),
                "delivery-1",
            )
            .await
            .unwrap_err();
        assert!(execution_message(error).contains("1-64"));
    }
}

#[tokio::test]
async fn invalid_batch_items_keep_input_order_and_do_not_start_a_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = finance_executor(&temporary);
    let output = executor
        .finance_calculate(
            &finance_policy(),
            json!({
                "operations": [
                    { "id": "first", "operation": "percentage_change", "arguments": { "start": "1", "end": "2", "unit": "percent" } },
                    { "id": "second", "operation": "npv", "arguments": {} }
                ]
            }),
            "delivery-1",
        )
        .await
        .unwrap();
    assert_eq!(output["results"][0]["id"], "first");
    assert_eq!(output["results"][0]["index"], 0);
    assert_eq!(output["results"][1]["id"], "second");
    assert_eq!(output["results"][1]["index"], 1);
}
