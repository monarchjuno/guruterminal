use super::*;

fn manifest() -> KisManifest {
    KisManifest::parse(KIS_MANIFEST_JSON).unwrap()
}

fn complete_profile() -> KisAccountProfile {
    KisAccountProfile::from_values(BTreeMap::from([
        ("account_number".to_owned(), "12345678".to_owned()),
        ("account_product_code".to_owned(), "01".to_owned()),
        ("account_password".to_owned(), "native-password".to_owned()),
        (
            "customer_identity_number".to_owned(),
            "identity-number".to_owned(),
        ),
        ("home_net_id".to_owned(), "home-net-id".to_owned()),
        ("hts_id".to_owned(), "hts-user".to_owned()),
    ]))
    .unwrap()
}

fn account_only_profile() -> KisAccountProfile {
    KisAccountProfile::from_values(BTreeMap::from([
        ("account_number".to_owned(), "12345678".to_owned()),
        ("account_product_code".to_owned(), "01".to_owned()),
    ]))
    .unwrap()
}

fn tool_query_for(operation: &KisOperation, environment: KisEnvironment) -> KisToolQuery {
    let selected_rule = operation
        .tr_id_rules
        .iter()
        .find(|rule| rule.when.get("environment").map(String::as_str) == Some(environment.as_str()))
        .unwrap();
    let mut params = BTreeMap::new();
    for parameter in operation
        .parameters
        .iter()
        .filter(|parameter| parameter.source == KisParameterSource::Tool)
    {
        if let Some(value) = selected_rule.when.get(&parameter.id) {
            params.insert(parameter.id.clone(), value.clone());
        } else if parameter.required {
            params.insert(parameter.id.clone(), "X".to_owned());
        }
    }
    KisToolQuery {
        operation_id: operation.id.clone(),
        params,
    }
}

#[test]
fn manifest_pins_the_reviewed_read_inventory_and_excludes_writes() {
    let manifest = manifest();
    assert_eq!(manifest.operations.len(), 146);
    assert_eq!(manifest.market_operation_count(), 91);
    assert_eq!(manifest.public_operation_count(), 88);
    assert_eq!(manifest.profile_gated_market_operation_count(), 3);
    assert_eq!(manifest.account_operation_count(), 55);
    assert_eq!(manifest.excluded_write_operation_ids.len(), 18);
    assert!(manifest
        .operations
        .iter()
        .all(|operation| operation.http_method == "GET"));
    assert!(manifest
        .excluded_write_operation_ids
        .iter()
        .all(|excluded| {
            manifest
                .operations
                .iter()
                .all(|operation| operation.id != *excluded)
        }));
    assert!(manifest
        .excluded_write_operation_ids
        .contains(&"domestic_stock.order_cash".to_owned()));
    assert!(manifest
        .excluded_write_operation_ids
        .contains(&"overseas_stock.order_rvsecncl".to_owned()));

    let excluded_order = KisToolQuery {
        operation_id: "domestic_stock.order_cash".to_owned(),
        params: BTreeMap::new(),
    };
    assert!(matches!(
        prepare_kis_request(
            &manifest,
            &excluded_order,
            KisEnvironment::Real,
            None,
            KisRequestAuthority::AgentRead,
        ),
        Err(FinanceDataError::InvalidQuery(
            "KIS operation ID is not in the reviewed manifest"
        ))
    ));
}

#[test]
fn public_request_uses_only_the_fixed_host_path_transaction_and_wire_mapping() {
    let manifest = manifest();
    let query = KisToolQuery {
        operation_id: "domestic_stock.inquire_price".to_owned(),
        params: BTreeMap::from([
            ("fid_cond_mrkt_div_code".to_owned(), "J".to_owned()),
            ("fid_input_iscd".to_owned(), "005930".to_owned()),
        ]),
    };
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        None,
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    assert_eq!(prepared.url.host_str(), Some("openapi.koreainvestment.com"));
    assert_eq!(prepared.url.port(), Some(9443));
    assert_eq!(
        prepared.url.path(),
        "/uapi/domestic-stock/v1/quotations/inquire-price"
    );
    assert_eq!(prepared.tr_id, "FHKST01010100");
    assert_eq!(
        prepared.url.query_pairs().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            ("FID_COND_MRKT_DIV_CODE".into(), "J".into()),
            ("FID_INPUT_ISCD".into(), "005930".into()),
        ])
    );
    let encoded = prepared.url.as_str();
    assert!(!encoded.contains("appkey"));
    assert!(!encoded.contains("appsecret"));
    assert!(!encoded.contains("token"));
}

#[test]
fn all_146_agent_reads_prepare_for_both_fixed_environments_and_profile_reads_fail_closed() {
    let manifest = manifest();
    let profile = complete_profile();
    for operation in &manifest.operations {
        for (environment, host) in [
            (KisEnvironment::Real, "openapi.koreainvestment.com"),
            (KisEnvironment::Demo, "openapivts.koreainvestment.com"),
        ] {
            let query = tool_query_for(operation, environment);
            let prepared = prepare_kis_request(
                &manifest,
                &query,
                environment,
                Some(&profile),
                KisRequestAuthority::AgentRead,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{} did not prepare for {}: {error}",
                    operation.id,
                    environment.as_str()
                )
            });
            assert_eq!(prepared.operation.id, operation.id);
            assert_eq!(prepared.url.host_str(), Some(host));
        }
        if operation.requires_account_profile() {
            let query = tool_query_for(operation, KisEnvironment::Real);
            assert!(matches!(
                prepare_kis_request(
                    &manifest,
                    &query,
                    KisEnvironment::Real,
                    None,
                    KisRequestAuthority::AgentRead,
                ),
                Err(FinanceDataError::InvalidQuery(
                    "KIS operation requires a configured account profile"
                ))
            ));
        }
    }
}

#[test]
fn account_profile_values_cannot_be_supplied_as_tool_parameters() {
    let manifest = manifest();
    for (operation_id, parameter, value) in [
        ("domestic_stock.inquire_account_balance", "cano", "87654321"),
        (
            "domestic_stock.inquire_account_balance",
            "acnt_prdt_cd",
            "01",
        ),
        ("domestic_stock.psearch_result", "user_id", "model-user"),
    ] {
        let operation = manifest.operation(operation_id).unwrap();
        let mut query = tool_query_for(operation, KisEnvironment::Real);
        query.params.insert(parameter.to_owned(), value.to_owned());
        assert!(matches!(
            prepare_kis_request(
                &manifest,
                &query,
                KisEnvironment::Real,
                Some(&complete_profile()),
                KisRequestAuthority::AgentRead,
            ),
            Err(FinanceDataError::InvalidQuery(
                "KIS parameters do not match the reviewed operation"
            ))
        ));
    }
}

#[test]
fn conditional_transaction_id_rules_are_selected_from_reviewed_values() {
    let manifest = manifest();
    let operation = manifest
        .operation("domestic_stock.inquire_daily_ccld")
        .unwrap();
    let mut query = tool_query_for(operation, KisEnvironment::Demo);
    query.params.insert("pd_dv".to_owned(), "inner".to_owned());
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Demo,
        Some(&complete_profile()),
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    assert_eq!(prepared.tr_id, "VTTC0081R");
    assert_eq!(
        prepared.url.host_str(),
        Some("openapivts.koreainvestment.com")
    );
    assert_eq!(prepared.url.port(), Some(29443));
}

#[test]
fn operation_search_exposes_only_reads_authorized_by_the_native_profile() {
    let service = FinanceDataService::new().unwrap();
    let public = service
        .kis_operation_search(json!({"query": "", "limit": "20"}), None)
        .unwrap();
    assert_eq!(
        public["schema_version"],
        "guruterminal-kis-operation-search/1"
    );
    assert_eq!(public["policy"]["public_reads_available_v1"], 88);
    assert_eq!(public["policy"]["agent_reads_available"], 88);
    assert_eq!(public["policy"]["account_reads_available_v1"], 0);
    let operations = public["operations"].as_array().unwrap();
    assert!(!operations.is_empty());
    assert!(operations.iter().all(|operation| {
        operation["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|parameter| {
                !matches!(
                    parameter["id"].as_str(),
                    Some("cano" | "acnt_prdt_cd" | "user_id" | "acnt_pwd")
                )
            })
    }));

    let account_only = service
        .kis_operation_search(
            json!({"query": "inquire_balance", "limit": "20"}),
            Some(&account_only_profile()),
        )
        .unwrap();
    assert_eq!(account_only["policy"]["agent_reads_available"], 143);
    assert_eq!(account_only["policy"]["account_reads_available_v1"], 55);

    let profiled = service
        .kis_operation_search(
            json!({"query": "inquire_balance", "limit": "20"}),
            Some(&complete_profile()),
        )
        .unwrap();
    assert_eq!(profiled["policy"]["agent_reads_available"], 146);
    assert_eq!(profiled["policy"]["account_reads_available_v1"], 55);
    assert!(profiled["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation["scope"] == "account"));
}

#[test]
fn public_normalization_records_no_raw_persistence() {
    let manifest = manifest();
    let query = KisToolQuery {
        operation_id: "domestic_stock.inquire_price".to_owned(),
        params: BTreeMap::from([
            ("fid_cond_mrkt_div_code".to_owned(), "J".to_owned()),
            ("fid_input_iscd".to_owned(), "005930".to_owned()),
        ]),
    };
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        None,
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    let bytes = br#"{"rt_cd":"0","msg_cd":"MCA00000","msg1":"ok","output":{"stck_shrn_iscd":"005930","stck_prpr":"70000"}}"#;
    let raw = RawProviderResponse {
        bytes: bytes.to_vec(),
        source_url: prepared.url.to_string(),
        retrieved_at: "2026-08-13T00:00:00.000Z".to_owned(),
        continuation: None,
    };
    let result = normalize_kis_market_response(&prepared, &raw, None).unwrap();
    assert_eq!(result["tool"], "finance_market_data");
    assert_eq!(result["operation"], "market.operation");
    assert_eq!(result["source_id"], KIS_SOURCE_ID);
    assert_eq!(result["quality"]["status"], "warn");
    assert!(result["quality"].get("use_class").is_none());
    assert_eq!(result["quality"]["checks"][0]["status"], "pass");
    assert_eq!(result["quality"]["checks"][1]["status"], "pass");
    assert_eq!(result["quality"]["checks"][2]["status"], "warn");
    assert_eq!(result["schema_version"], "guruterminal-kis-result/1");
    assert_eq!(result["provenance"]["raw_persisted"], false);
    assert!(result["data"]["response"].get("rt_cd").is_none());
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("appsecret"));
    assert!(!encoded.contains("authorization"));
}

#[test]
fn public_normalization_rejects_sensitive_response_fields() {
    let manifest = manifest();
    let query = KisToolQuery {
        operation_id: "domestic_stock.inquire_price".to_owned(),
        params: BTreeMap::from([
            ("fid_cond_mrkt_div_code".to_owned(), "J".to_owned()),
            ("fid_input_iscd".to_owned(), "005930".to_owned()),
        ]),
    };
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        None,
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    let raw = RawProviderResponse {
        bytes: br#"{"rt_cd":"0","msg_cd":"MCA00000","msg1":"ok","output":{"stck_shrn_iscd":"005930","CANO":"12345678","tot_evlu_amt":"1000000"}}"#.to_vec(),
        source_url: prepared.url.to_string(),
        retrieved_at: "2026-08-13T00:00:00.000Z".to_owned(),
        continuation: None,
    };

    let error = normalize_kis_market_response(&prepared, &raw, None).unwrap_err();
    assert!(matches!(
        error,
        FinanceDataError::KisResponseContract { .. }
    ));
    assert_eq!(
        error.to_string(),
        "KIS response contract mismatch for domestic_stock.inquire_price: container output included unexpected sensitive field(s): CANO"
    );
}

#[test]
fn public_normalization_drops_additive_provider_fields() {
    let manifest = manifest();
    let query = KisToolQuery {
        operation_id: "domestic_stock.inquire_price".to_owned(),
        params: BTreeMap::from([
            ("fid_cond_mrkt_div_code".to_owned(), "J".to_owned()),
            ("fid_input_iscd".to_owned(), "005930".to_owned()),
        ]),
    };
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        None,
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    let raw = RawProviderResponse {
        bytes: br#"{"rt_cd":"0","msg_cd":"MCA00000","msg1":"ok","provider_meta":"ignored","output":{"stck_shrn_iscd":"005930","":"placeholder","future_field":"ignored"}}"#.to_vec(),
        source_url: prepared.url.to_string(),
        retrieved_at: "2026-08-13T00:00:00.000Z".to_owned(),
        continuation: None,
    };

    let result = normalize_kis_market_response(&prepared, &raw, None).unwrap();
    let response = &result["data"]["response"];
    assert_eq!(response["output"]["stck_shrn_iscd"], "005930");
    assert!(response.get("provider_meta").is_none());
    assert!(response["output"].get("").is_none());
    assert!(response["output"].get("future_field").is_none());
    assert!(result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| {
            warning
                .as_str()
                .unwrap()
                .contains("outside the reviewed response schema")
        }));
}

#[test]
fn account_normalization_redacts_identity_and_does_not_persist_raw_data() {
    let manifest = manifest();
    let operation = manifest
        .operation("domestic_futureoption.inquire_balance")
        .unwrap();
    let query = tool_query_for(operation, KisEnvironment::Real);
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        Some(&complete_profile()),
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    let bytes = br#"{"rt_cd":"0","msg_cd":"MCA00000","msg1":"ok","ctx_area_fk200":"","ctx_area_nk200":"","output1":[{"cano":"12345678","evlu_amt":"1000000"}],"output2":[]}"#;
    let raw = RawProviderResponse {
        bytes: bytes.to_vec(),
        source_url: prepared.url.to_string(),
        retrieved_at: "2026-08-13T00:00:00.000Z".to_owned(),
        continuation: None,
    };
    let result =
        normalize_kis_account_response(&prepared, &raw, Some(&complete_profile())).unwrap();
    assert_eq!(
        result["schema_version"],
        "guruterminal-kis-account-result/1"
    );
    assert_eq!(result["provenance"]["raw_persisted"], false);
    assert_eq!(
        result["data"]["response"]["output1"][0]["evlu_amt"],
        "1000000"
    );
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("12345678"));
    assert!(!result["provenance"]["source_origin"]
        .as_str()
        .unwrap()
        .contains('?'));
}

#[test]
fn account_normalization_rejects_profile_values_hidden_under_allowed_generic_fields() {
    let manifest = manifest();
    let operation = manifest
        .operation("domestic_futureoption.inquire_balance")
        .unwrap();
    let profile = complete_profile();
    let query = tool_query_for(operation, KisEnvironment::Real);
    let prepared = prepare_kis_request(
        &manifest,
        &query,
        KisEnvironment::Real,
        Some(&profile),
        KisRequestAuthority::AgentRead,
    )
    .unwrap();
    let raw = RawProviderResponse {
        bytes: br#"{"rt_cd":"0","msg_cd":"MCA00000","msg1":"ok","ctx_area_fk200":"","ctx_area_nk200":"","output1":[{"evlu_amt":"prefix-hts-user-suffix"}],"output2":[]}"#.to_vec(),
        source_url: prepared.url.to_string(),
        retrieved_at: "2026-08-13T00:00:00.000Z".to_owned(),
        continuation: None,
    };
    assert!(matches!(
        normalize_kis_account_response(&prepared, &raw, Some(&profile)),
        Err(FinanceDataError::InvalidResponse)
    ));
}

#[test]
fn credential_echo_is_rejected_before_a_provider_result_is_admitted() {
    let app_key = "test-kis-application-key";
    let app_secret = "test-kis-application-secret";
    let access_token = "test-kis-access-token-value";

    for body in [
        format!(r#"{{"rt_cd":"0","output":{{"app_key":"{app_key}","price":"1"}}}}"#),
        format!(r#"{{"rt_cd":"0","output":{{"provider_value":"prefix-{app_secret}-suffix"}}}}"#),
        format!(r#"{{"rt_cd":"0","output":{{"Authorization":"Bearer {access_token}"}}}}"#),
    ] {
        assert!(matches!(
            reject_kis_credential_echo(body.as_bytes(), app_key, app_secret, access_token,),
            Err(FinanceDataError::InvalidResponse)
        ));
    }
    assert!(reject_kis_credential_echo(
        br#"{"rt_cd":"0","output":{"stck_prpr":"70000"}}"#,
        app_key,
        app_secret,
        access_token,
    )
    .is_ok());
}

#[tokio::test]
async fn deleting_kis_credentials_can_clear_every_ephemeral_access_token() {
    let service = FinanceDataService::new().unwrap();
    service.kis_tokens.lock().await.insert(
        "test-fingerprint".to_owned(),
        KisAccessToken {
            value: "test-kis-access-token-value".to_owned(),
            environment: KisEnvironment::Real,
            expires_at: Utc::now() + ChronoDuration::hours(1),
        },
    );
    service.clear_kis_token_cache().await;
    assert!(service.kis_tokens.lock().await.is_empty());
}

#[test]
fn token_response_is_bounded_and_error_bodies_are_redacted_to_a_typed_failure() {
    assert_ne!(
        kis_credential_fingerprint(KisEnvironment::Real, "test-app-key", "test-app-secret"),
        kis_credential_fingerprint(KisEnvironment::Demo, "test-app-key", "test-app-secret")
    );
    let token = parse_kis_token_response(
        br#"{"access_token":"a-valid-bounded-access-token","token_type":"Bearer","expires_in":86400}"#,
        KisEnvironment::Real,
        "test-app-key",
        "test-app-secret",
    )
    .unwrap();
    assert!(token.environment == KisEnvironment::Real);
    let rejected = match parse_kis_token_response(
        br#"{"error_code":"EGW00123","error_description":"The app key and secret do not match."}"#,
        KisEnvironment::Real,
        "test-app-key",
        "test-app-secret",
    ) {
        Err(error) => error,
        Ok(_) => panic!("KIS error response unexpectedly produced a token"),
    };
    assert!(matches!(
        &rejected,
        FinanceDataError::KisCredentialRejected(diagnostic)
            if diagnostic.summary() == "EGW00123: The app key and secret do not match."
    ));

    let limited = classify_kis_token_error(
        StatusCode::FORBIDDEN,
        r#"{"error_code":"EGW00133","error_description":"1분당 1회만 발급 가능합니다."}"#
            .as_bytes(),
        "test-app-key",
        "test-app-secret",
    );
    assert!(matches!(limited, FinanceDataError::KisRateLimited(_)));

    let redacted = classify_kis_token_error(
        StatusCode::BAD_REQUEST,
        br#"{"error_code":"EGW00123","error_description":"test-app-secret was rejected"}"#,
        "test-app-key",
        "test-app-secret",
    );
    assert!(matches!(
        redacted,
        FinanceDataError::KisCredentialRejected(diagnostic)
            if diagnostic.summary() == "EGW00123"
    ));
}
