use super::*;

#[test]
fn query_building_keeps_the_provider_host_fixed() {
    let url = world_bank_url(&query()).unwrap();
    assert_eq!(url.host_str(), Some("api.worldbank.org"));
    assert_eq!(url.path(), "/v2/country/USA/indicator/NY.GDP.MKTP.CD");
    assert!(url.query().unwrap().contains("date=2020%3A2021"));
}

#[test]
fn query_validation_rejects_extra_fields_and_path_injection() {
    assert!(validate_macro_query(json!({
        "provider": WORLD_BANK_SOURCE_ID,
        "economy": "../../all",
        "indicator": "NY.GDP.MKTP.CD",
        "start_year": 2020,
        "end_year": 2021
    }))
    .is_err());
    assert!(validate_macro_query(json!({
        "provider": WORLD_BANK_SOURCE_ID,
        "economy": "USA",
        "indicator": "NY.GDP.MKTP.CD",
        "start_year": 2020,
        "end_year": 2021,
        "url": "https://example.com"
    }))
    .is_err());
}

#[test]
fn source_inventory_projects_catalog_metadata_and_runtime_overlays() {
    let service = FinanceDataService::new().unwrap();
    let sources = service.sources();
    let projected = sources["sources"].as_array().unwrap();
    let catalog = crate::marketplace::bundled_catalog().unwrap();
    assert_eq!(projected.len(), FINANCE_SOURCE_IDS.len());
    assert_eq!(
        projected
            .iter()
            .map(|source| source["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        FINANCE_SOURCE_IDS
    );
    for source in projected {
        let id = source["id"].as_str().unwrap();
        let entry = catalog.entries.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(source["name"], entry.name);
        assert_eq!(source["data_authority"], entry.data_authority);
        assert_eq!(source["status"], json!(entry.release_stage));
        assert_eq!(source["trust"], json!(entry.trust));
        assert_eq!(source["capabilities"], json!(entry.capabilities));
        assert_eq!(
            source["network_hosts"],
            json!(entry.permissions.network_hosts)
        );
        assert_eq!(source["terms_url"], json!(entry.terms_url));
        assert!(source["query_contract"].is_object());
    }
    assert!(projected
        .iter()
        .all(|source| source["official_source"] == true));
    assert_eq!(projected[0]["id"], WORLD_BANK_SOURCE_ID);
    assert_eq!(projected[1]["id"], OPENDART_SOURCE_ID);
    assert_eq!(projected[1]["access"], "api_key");
    assert_eq!(projected[2]["id"], KRX_SOURCE_ID);
    assert_eq!(projected[2]["access"], "api_key");
    assert_eq!(projected[3]["id"], KIS_SOURCE_ID);
    assert_eq!(projected[3]["access"], "app_credentials");
    assert_eq!(projected[3]["query_contract"]["public_read_operations"], 88);
    assert_eq!(
        projected[3]["query_contract"]["profile_gated_market_operations"],
        3
    );
    assert_eq!(
        projected[3]["query_contract"]["account_read_operations"],
        55
    );
    assert_eq!(
        projected[3]["query_contract"]["account_reads_available_v1"],
        true
    );
    assert_eq!(
        projected[3]["query_contract"]["account_profile_required"],
        true
    );
    assert_eq!(projected[3]["query_contract"]["orders_available"], false);
}

#[tokio::test]
async fn credential_verification_rejects_unknown_or_malformed_inputs_before_network() {
    let service = FinanceDataService::new().unwrap();
    let malformed = BTreeMap::from([("api_key".to_owned(), "not-a-secret".to_owned())]);
    assert!(matches!(
        service
            .verify_credential("unknown.provider", &malformed, None)
            .await,
        Err(FinanceDataError::InvalidQuery(
            "credential provider is not supported"
        ))
    ));
    let short = BTreeMap::from([("api_key".to_owned(), "short".to_owned())]);
    assert!(matches!(
        service
            .verify_credential(OPENDART_SOURCE_ID, &short, None)
            .await,
        Err(FinanceDataError::InvalidQuery(
            "provider API key has an invalid shape"
        ))
    ));
    let incomplete_kis =
        BTreeMap::from([("app_key".to_owned(), "test-kis-application-key".to_owned())]);
    assert!(matches!(
        service
            .verify_credential(KIS_SOURCE_ID, &incomplete_kis, None)
            .await,
        Err(FinanceDataError::InvalidQuery(
            "KIS requires one app key and one app secret"
        ))
    ));
    let unexpected_kis = BTreeMap::from([
        ("app_key".to_owned(), "test-kis-application-key".to_owned()),
        (
            "app_secret".to_owned(),
            "test-kis-application-secret".to_owned(),
        ),
        ("api_key".to_owned(), "unexpected-secret".to_owned()),
    ]);
    assert!(matches!(
        service
            .verify_credential(KIS_SOURCE_ID, &unexpected_kis, None)
            .await,
        Err(FinanceDataError::InvalidQuery(
            "KIS requires one app key and one app secret"
        ))
    ));
}
