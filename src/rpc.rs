//! Minimal JSON-RPC client — only the handful of read-only calls doctor needs.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

pub struct RpcClient {
    url: String,
    agent: ureq::Agent,
}

pub struct AccountInfo {
    pub data: Vec<u8>,
    pub owner: String,
}

impl RpcClient {
    pub fn new(url: impl Into<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .build()
            .into();
        Self {
            url: url.into(),
            agent,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        // One retry with a short backoff: public endpoints drop or rate-limit
        // often enough that a single transient failure shouldn't reach the user.
        let mut last_err = None;
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            match self
                .agent
                .post(&self.url)
                .header("content-type", "application/json")
                .send_json(&body)
            {
                Ok(mut response) => {
                    let response: Value = response
                        .body_mut()
                        .read_json()
                        .context("RPC response was not JSON")?;
                    if let Some(err) = response.get("error") {
                        return Err(anyhow!("RPC {method} error: {err}"));
                    }
                    return response
                        .get("result")
                        .cloned()
                        .ok_or_else(|| anyhow!("RPC {method}: no result field"));
                }
                Err(err) => last_err = Some(err),
            }
        }
        Err(anyhow!(
            "RPC {method} failed against {} after retry: {}",
            self.url,
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    /// `None` when the account does not exist.
    pub fn account(&self, pubkey: &str) -> Result<Option<AccountInfo>> {
        let result = self.call("getAccountInfo", json!([pubkey, {"encoding": "base64"}]))?;
        let value = &result["value"];
        if value.is_null() {
            return Ok(None);
        }
        let data_b64 = value["data"][0]
            .as_str()
            .ok_or_else(|| anyhow!("getAccountInfo: missing data"))?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .context("account data was not valid base64")?;
        Ok(Some(AccountInfo {
            data,
            owner: value["owner"].as_str().unwrap_or_default().to_owned(),
        }))
    }

    pub fn balance(&self, pubkey: &str) -> Result<u64> {
        let result = self.call("getBalance", json!([pubkey]))?;
        result["value"]
            .as_u64()
            .ok_or_else(|| anyhow!("getBalance: no value"))
    }

    pub fn min_rent(&self, data_len: u64) -> Result<u64> {
        let result = self.call("getMinimumBalanceForRentExemption", json!([data_len]))?;
        result
            .as_u64()
            .ok_or_else(|| anyhow!("getMinimumBalanceForRentExemption: not a number"))
    }

    /// Feature gate status: `None` = account absent (inactive / not scheduled),
    /// `Some(true)` = activated, `Some(false)` = pending activation.
    pub fn feature_active(&self, gate: &str) -> Result<Option<bool>> {
        let account = self.account(gate)?;
        Ok(account.map(|acc| acc.data.first() == Some(&1)))
    }
}

pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}
