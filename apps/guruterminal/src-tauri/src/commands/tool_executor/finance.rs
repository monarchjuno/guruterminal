use super::*;

const FINANCE_CALCULATE_OPERATIONS: &[&str] = &[
    "compound_annual_growth_rate",
    "currency_convert",
    "dcf_sensitivity",
    "discounted_cash_flow",
    "enterprise_value_bridge",
    "internal_rate_of_return",
    "percentage_change",
    "period_aggregate",
    "point_in_time_filter",
    "ratio",
    "risk_metrics",
    "series_statistics",
    "weighted_average_cost_of_capital",
];

const FINANCE_HOST_ARGUMENT_KEYS: &[&str] = &["as_of"];

const FINANCE_DECIMAL_SCALAR_KEYS: &[&str] = &[
    "amount",
    "cost_of_debt",
    "cost_of_equity",
    "debt_weight",
    "denominator",
    "discount_rate",
    "end",
    "enterprise_value",
    "equity_value",
    "equity_weight",
    "fx_rate",
    "lease_liabilities",
    "minority_interest",
    "multiplier",
    "net_debt",
    "non_operating_assets",
    "numerator",
    "shares_outstanding",
    "start",
    "tax_rate",
    "terminal_growth_rate",
    "terminal_value",
];

const FINANCE_DECIMAL_ARRAY_KEYS: &[&str] = &[
    "cash_flows",
    "discount_rate_shocks",
    "growth_rate_shocks",
    "market_values",
    "values",
];

fn finance_execution(message: impl Into<String>) -> BrokerError {
    BrokerError::Execution(message.into())
}

fn finance_calculate_worker_keys(operation: &str) -> &'static [&'static str] {
    match operation {
        "percentage_change" => &["end", "precision", "start"],
        "ratio" => &[
            "denominator",
            "multiplier",
            "numerator",
            "precision",
            "unit",
        ],
        "compound_annual_growth_rate" => &["end", "periods", "precision", "start"],
        "discounted_cash_flow" => &[
            "cash_flows",
            "currency",
            "discount_rate",
            "net_debt",
            "precision",
            "shares_outstanding",
            "terminal_growth_rate",
            "terminal_value",
        ],
        "dcf_sensitivity" => &[
            "cash_flows",
            "currency",
            "discount_rate",
            "discount_rate_shocks",
            "growth_rate_shocks",
            "net_debt",
            "precision",
            "shares_outstanding",
            "terminal_growth_rate",
            "terminal_value",
        ],
        "enterprise_value_bridge" => &[
            "currency",
            "enterprise_value",
            "equity_value",
            "lease_liabilities",
            "minority_interest",
            "net_debt",
            "non_operating_assets",
            "precision",
            "shares_outstanding",
        ],
        "internal_rate_of_return" => &["cash_flow_dates", "cash_flows", "precision"],
        "period_aggregate" => &["dates", "periods", "precision", "values"],
        "point_in_time_filter" => &["rows"],
        "risk_metrics" => &["dates", "market_values", "precision", "values"],
        "series_statistics" => &["dates", "periods_per_year", "precision", "values"],
        "currency_convert" => &[
            "amount",
            "currency",
            "fx_as_of",
            "fx_rate",
            "precision",
            "quote_currency",
        ],
        "weighted_average_cost_of_capital" => &[
            "cost_of_debt",
            "cost_of_equity",
            "debt_weight",
            "equity_weight",
            "precision",
            "tax_rate",
        ],
        _ => &[],
    }
}

fn parse_finance_as_of(value: Option<&str>) -> Result<Option<chrono::NaiveDate>, BrokerError> {
    match parse_as_of_date(value) {
        Ok(date) => Ok(date),
        Err(BrokerError::Malformed) => Err(finance_execution(
            "finance_calculate arguments.as_of must be an ISO date (YYYY-MM-DD)",
        )),
        Err(error) => Err(error),
    }
}

fn reject_grouped_decimal(path: &str, value: &Value) -> Result<(), BrokerError> {
    match value {
        Value::String(text) if text.contains(',') => Err(finance_execution(format!(
            "finance_calculate {path} must be a decimal without grouping commas"
        ))),
        _ => Ok(()),
    }
}

fn reject_grouped_decimals(arguments: &serde_json::Map<String, Value>) -> Result<(), BrokerError> {
    for key in FINANCE_DECIMAL_SCALAR_KEYS {
        if let Some(value) = arguments.get(*key) {
            reject_grouped_decimal(&format!("arguments.{key}"), value)?;
        }
    }
    for key in FINANCE_DECIMAL_ARRAY_KEYS {
        if let Some(Value::Array(items)) = arguments.get(*key) {
            for (index, value) in items.iter().enumerate() {
                reject_grouped_decimal(&format!("arguments.{key}[{index}]"), value)?;
            }
        }
    }
    Ok(())
}

fn validate_finance_calculate_arguments(
    operation: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<(), BrokerError> {
    let allowed = finance_calculate_worker_keys(operation);
    let unknown = arguments
        .keys()
        .filter(|key| {
            !allowed.contains(&key.as_str()) && !FINANCE_HOST_ARGUMENT_KEYS.contains(&key.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(finance_execution(format!(
            "finance_calculate {operation} arguments contain unsupported fields: {}",
            unknown.join(", ")
        )));
    }
    reject_grouped_decimals(arguments)
}

fn parse_finance_calculate_params(params: &Value) -> Result<(&str, Value), BrokerError> {
    let object = params.as_object().ok_or_else(|| {
        finance_execution("finance_calculate requires an object with operation and arguments")
    })?;
    let extra = object
        .keys()
        .filter(|key| key.as_str() != "operation" && key.as_str() != "arguments")
        .cloned()
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        return Err(finance_execution(format!(
            "finance_calculate does not accept fields: {}",
            extra.join(", ")
        )));
    }
    let operation = object
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            finance_execution("finance_calculate.operation must be a known calculation name")
        })?;
    if !FINANCE_CALCULATE_OPERATIONS.contains(&operation) {
        return Err(BrokerError::MethodDenied);
    }
    let arguments = object
        .get("arguments")
        .ok_or_else(|| finance_execution("finance_calculate.arguments must be an object"))?;
    let argument_object = arguments
        .as_object()
        .ok_or_else(|| finance_execution("finance_calculate.arguments must be an object"))?;
    validate_finance_calculate_arguments(operation, argument_object)?;
    Ok((operation, arguments.clone()))
}

impl AppToolExecutor {
    pub(super) async fn finance_calculate(
        &self,
        policy: &ToolPolicy,
        params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.require_capability("guruterminal.finance-core")?;
        let (operation, mut arguments) = parse_finance_calculate_params(&params)?;
        let (context, resolved) = self
            .resolve_finance_calculation(policy, operation, &mut arguments)
            .await?;
        self.finance_worker_call_with_context(operation, resolved, context)
            .await
    }

    async fn resolve_finance_calculation(
        &self,
        policy: &ToolPolicy,
        operation: &str,
        arguments: &mut Value,
    ) -> Result<(Value, Value), BrokerError> {
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| finance_execution("finance_calculate.arguments must be an object"))?;
        let argument_as_of =
            parse_finance_as_of(object.remove("as_of").as_ref().and_then(Value::as_str))?;
        let cutoff = match (
            parse_finance_as_of(policy.as_of.as_deref())?,
            argument_as_of,
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        let now = Utc::now();
        let data_cutoff = cutoff
            .and_then(|date| date.and_hms_opt(23, 59, 59))
            .map(|naive| naive.and_utc())
            .unwrap_or(now);
        let sources = vec![json!({
            "source_id": "agent-authored-input",
            "provider": "agent-authored-input",
            "available_at": data_cutoff.to_rfc3339_opts(SecondsFormat::Secs, true),
            "retrieved_at": data_cutoff.to_rfc3339_opts(SecondsFormat::Secs, true)
        })];
        if operation == "point_in_time_filter"
            && !arguments.get("rows").is_some_and(Value::is_array)
        {
            return Err(BrokerError::Execution(
                "point_in_time_filter requires rows".into(),
            ));
        }
        Ok((
            finance_context_from_sources(data_cutoff, sources),
            arguments.clone(),
        ))
    }

    pub(super) async fn finance_resolve_entity(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["query"], &["limit"])?;
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 200)
            .ok_or(BrokerError::Malformed)?;
        let limit = object.get("limit").and_then(Value::as_u64).unwrap_or(8);
        if !(1..=20).contains(&limit) {
            return Err(BrokerError::Malformed);
        }
        let dart = if self.capability_enabled("opendart.disclosures") {
            Some(self.provider_credential("opendart.disclosures").await?)
        } else {
            None
        };
        if dart.is_none() {
            return Err(BrokerError::MethodDenied);
        }
        self.state
            .finance_data
            .resolve_entity(query, limit as usize, dart.as_deref())
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))
    }

    pub(super) async fn run_results_list(&self, params: Value) -> Result<Value, BrokerError> {
        exact_object(&params, &[], &[])?;
        let registry = self.capture.run_results.lock().await;
        Ok(json!({
            "results": registry.values().map(|result| json!({
                "result_ref": result.result_ref,
                "runtime_id": result.producer.runtime_id,
                "tool_name": result.producer.tool_name,
                "provider": result.producer.provider,
                "request_digest": result.request_digest,
                "response_digest": result.response_digest,
                "retrieved_at": result.retrieved_at,
                "warnings": result.warnings,
                "upstream_result_refs": result.upstream_result_refs,
            })).collect::<Vec<_>>()
        }))
    }

    pub(super) async fn provider_credential_bundle(
        &self,
        entry_id: &'static str,
    ) -> Result<crate::finance_credentials::ActiveCredentialBundle, BrokerError> {
        self.require_capability(entry_id)?;
        let credential =
            tokio::task::spawn_blocking(move || crate::finance_credentials::get(entry_id))
                .await
                .map_err(|_| BrokerError::Execution("credential lookup failed".into()))?
                .map_err(|_| BrokerError::Execution("credential lookup failed".into()))?;
        credential.ok_or_else(|| {
            BrokerError::Execution("the enabled finance connector needs a credential".into())
        })
    }

    pub(super) async fn provider_credential(
        &self,
        entry_id: &'static str,
    ) -> Result<String, BrokerError> {
        let credential = self.provider_credential_bundle(entry_id).await?;
        credential
            .get("api_key")
            .map(str::to_owned)
            .ok_or_else(|| BrokerError::Execution("credential lookup failed".into()))
    }

    pub(super) fn kis_account_profile(
        credential: &crate::finance_credentials::ActiveCredentialBundle,
    ) -> Result<Option<crate::finance_data::KisAccountProfile>, BrokerError> {
        let values = [
            "account_number",
            "account_product_code",
            "account_password",
            "customer_identity_number",
            "home_net_id",
            "hts_id",
        ]
        .into_iter()
        .filter_map(|key| {
            credential
                .get(key)
                .map(|value| (key.to_owned(), value.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
        if values.is_empty() {
            return Ok(None);
        }
        crate::finance_data::KisAccountProfile::from_values(values)
            .map(Some)
            .map_err(|_| BrokerError::Execution("KIS account profile is unavailable".into()))
    }

    pub(super) async fn finance_macro_data(
        &self,
        policy: &ToolPolicy,
        mut params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let _ = take_as_of(&mut params)?;
        let provider = params
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(BrokerError::Malformed)?;
        let (output, source_id) = match provider.as_str() {
            "world-bank.indicators" => {
                self.require_capability("world-bank.indicators")?;
                (
                    self.state
                        .finance_data
                        .macro_data(params)
                        .await
                        .map_err(|error| BrokerError::Execution(error.to_string()))?,
                    "world-bank.indicators",
                )
            }
            _ => return Err(BrokerError::MethodDenied),
        };
        let mut output = output;
        if let Some(cutoff) = cutoff {
            apply_as_of_cutoff(&mut output, cutoff)?;
        }
        self.validate_provider_tool_output(output, "finance_macro_data", source_id)
    }

    pub(super) async fn finance_company_data(
        &self,
        policy: &ToolPolicy,
        mut params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let _ = take_as_of(&mut params)?;
        if params.get("operation").and_then(Value::as_str) != Some("company.facts") {
            return Err(BrokerError::MethodDenied);
        }
        let provider = params
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(BrokerError::Malformed)?;
        let query = without_route_fields(params, &["provider", "operation"])?;
        let output = match provider.as_str() {
            "opendart.disclosures" => {
                let credential = self.provider_credential("opendart.disclosures").await?;
                let output = self
                    .state
                    .finance_data
                    .opendart_company_facts(query, &credential)
                    .await
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                drop(credential);
                output
            }
            _ => return Err(BrokerError::MethodDenied),
        };
        let mut output = output;
        if let Some(cutoff) = cutoff {
            apply_as_of_cutoff(&mut output, cutoff)?;
        }
        self.validate_provider_tool_output(output, "finance_company_data", &provider)
    }

    pub(super) async fn finance_filings(
        &self,
        policy: &ToolPolicy,
        mut params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let _ = take_as_of(&mut params)?;
        let provider = params
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(BrokerError::Malformed)?;
        let operation = params
            .get("operation")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "search" | "read"))
            .map(str::to_owned)
            .ok_or(BrokerError::Malformed)?;
        let query = without_route_fields(params, &["provider", "operation"])?;
        let output = match (provider.as_str(), operation.as_str()) {
            ("opendart.disclosures", "search") => {
                let credential = self.provider_credential("opendart.disclosures").await?;
                let output = self
                    .state
                    .finance_data
                    .opendart_filing_search(query, &credential)
                    .await
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                drop(credential);
                output
            }
            ("opendart.disclosures", "read") => {
                let credential = self.provider_credential("opendart.disclosures").await?;
                let output = self
                    .state
                    .finance_data
                    .opendart_filing_read(query, &credential)
                    .await
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                drop(credential);
                output
            }
            _ => return Err(BrokerError::MethodDenied),
        };
        let mut output = output;
        if let Some(cutoff) = cutoff {
            apply_as_of_cutoff(&mut output, cutoff)?;
        }
        self.validate_provider_tool_output(output, "finance_filings", &provider)
    }

    pub(super) async fn finance_market_data(
        &self,
        policy: &ToolPolicy,
        mut params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let _ = take_as_of(&mut params)?;
        let provider = params
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(BrokerError::Malformed)?;
        if provider == crate::finance_data::KIS_SOURCE_ID {
            self.require_capability(crate::finance_data::KIS_SOURCE_ID)?;
            let object =
                exact_object(&params, &["provider", "operation_id", "params"], &["as_of"])?;
            let operation_id = object
                .get("operation_id")
                .and_then(Value::as_str)
                .ok_or(BrokerError::Malformed)?;
            let operation_params = object
                .get("params")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or(BrokerError::Malformed)?;
            let credential = self
                .provider_credential_bundle(crate::finance_data::KIS_SOURCE_ID)
                .await?;
            let profile = Self::kis_account_profile(&credential)?;
            if operation_id == "catalog.search" {
                return self
                    .state
                    .finance_data
                    .kis_operation_search(operation_params, profile.as_ref())
                    .map_err(|error| BrokerError::Execution(error.to_string()));
            }
            let environment = crate::marketplace::connector_config_value(
                &self.state,
                crate::finance_data::KIS_SOURCE_ID,
                "environment",
            )
            .map_err(|_| BrokerError::Execution("KIS configuration is unavailable".into()))?
            .ok_or_else(|| {
                BrokerError::Execution("the KIS connector needs an environment".into())
            })?;
            let app_key = credential
                .get("app_key")
                .map(str::to_owned)
                .ok_or_else(|| BrokerError::Execution("credential lookup failed".into()))?;
            let app_secret = credential
                .get("app_secret")
                .map(str::to_owned)
                .ok_or_else(|| BrokerError::Execution("credential lookup failed".into()))?;
            let query = json!({
                "operation_id": operation_id,
                "params": operation_params
            });
            let output = self
                .state
                .finance_data
                .kis_read_data(query, &environment, &app_key, &app_secret, profile.as_ref())
                .await
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
            drop((credential, app_key, app_secret));
            return Ok(output);
        }
        if provider == "krx.market-data" {
            let credential = self.provider_credential("krx.market-data").await?;
            let query = without_route_fields(params, &["provider"])?;
            let mut output = self
                .state
                .finance_data
                .krx_market_data(query, &credential)
                .await
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
            drop(credential);
            if let Some(cutoff) = cutoff {
                apply_as_of_cutoff(&mut output, cutoff)?;
            }
            return self.validate_provider_tool_output(
                output,
                "finance_market_data",
                "krx.market-data",
            );
        }
        Err(BrokerError::MethodDenied)
    }

    pub(super) async fn finance_worker_call_with_context(
        &self,
        operation: &str,
        arguments: Value,
        context: Value,
    ) -> Result<Value, BrokerError> {
        let executable = self
            .state
            .artifacts
            .finance_executable
            .clone()
            .ok_or_else(|| BrokerError::Execution("finance worker is unavailable".into()))?;
        let worker_id = new_id("finance");
        let _run_scratch = crate::run_scratch::RunScratch::create(
            self.state.artifacts.deletion_root.clone(),
            &self.guru_id,
            &worker_id,
        )
        .map_err(|error| BrokerError::Execution(error.message))?;
        let config = FinanceLaunchConfig {
            executable,
            private_working_dir: _run_scratch.path().to_owned(),
            artifact_dir: _run_scratch.path().join("finance-artifacts"),
            lease_dir: self.state.artifacts.process_lease_dir.clone(),
        };
        let (worker, handshake) = FinanceWorker::spawn(config)
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if !handshake.tools.iter().any(|name| name == operation) {
            let _ = worker.shutdown(Duration::from_secs(1)).await;
            return Err(BrokerError::MethodDenied);
        }
        let result = worker
            .call_tool(operation, arguments, context, Duration::from_secs(30))
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()));
        let shutdown = worker
            .shutdown(Duration::from_secs(1))
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()));
        match (result, shutdown) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }
}
