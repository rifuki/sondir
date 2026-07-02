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
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(15))
            .build();
        Self { url: url.into(), agent }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response: Value = self
            .agent
            .post(&self.url)
            .set("content-type", "application/json")
            .send_json(body)
            .with_context(|| format!("RPC {method} failed against {}", self.url))?
            .into_json()
            .context("RPC response was not JSON")?;
        if let Some(err) = response.get("error") {
            return Err(anyhow!("RPC {method} error: {err}"));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("RPC {method}: no result field"))
    }

    /// `None` when the account does not exist.
    pub fn account(&self, pubkey: &str) -> Result<Option<AccountInfo>> {
        let result = self.call(
            "getAccountInfo",
            json!([pubkey, {"encoding": "base64"}]),
        )?;
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
