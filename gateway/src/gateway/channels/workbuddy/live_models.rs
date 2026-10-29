//! Account-scoped official catalog. Never read desktop caches or merge snapshots.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::gateway::{
    channels::AccountSnapshot,
    errors::{ErrorKind, ErrorScope, GatewayError},
    timeutil::now_ms,
    transport,
};

use super::{
    catalog,
    identity::{self, Hosts, Realm},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveModelCatalog {
    pub realm: &'static str,
    pub fetched_at_ms: u64,
    pub models: Vec<LiveModel>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    pub capabilities: LiveCapabilities,
    /// Preserve the upstream wording. A multiplier is not a per-request price.
    pub credits: Option<String>,
    pub credit_multiplier: Option<f64>,
    pub charge_type: &'static str,
}

#[derive(Debug, Serialize)]
pub struct LiveCapabilities {
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
}

fn invalid_catalog() -> GatewayError {
    GatewayError::new(
        ErrorKind::Upstream5xx,
        ErrorScope::Request,
        502,
        "WorkBuddy 未返回有效的账号模型清单，请检查账号授权后重试",
        false,
    )
}

// Observed official forms: x0.00, x0.00 credits, 0.00; unknown text stays unknown.
fn multiplier(credits: &str) -> Option<f64> {
    let normalized = credits.trim().to_ascii_lowercase();
    let normalized = normalized
        .strip_suffix("credits")
        .unwrap_or(&normalized)
        .trim();
    let normalized = normalized.strip_prefix('x').unwrap_or(normalized).trim();
    normalized
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn model(id: String, metadata: Option<&Value>) -> LiveModel {
    let entry = metadata.cloned().unwrap_or(Value::Null);
    let credits = match entry.get("credits") {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    };
    let credit_multiplier = credits.as_deref().and_then(multiplier);
    let charge_type = match credit_multiplier {
        Some(value) if value == 0.0 => "free",
        Some(_) => "credits",
        None => "unknown",
    };
    let positive = |value: Option<&Value>| value.and_then(Value::as_u64).filter(|value| *value > 0);
    let flag = |key: &str| entry.get(key).and_then(Value::as_bool).unwrap_or(false);
    LiveModel {
        display_name: entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        context_window: positive(entry.get("maxInputTokens")).or_else(|| {
            positive(
                entry
                    .get("contextWindow")
                    .and_then(|value| value.get("defaultLength")),
            )
        }),
        max_output: positive(entry.get("maxOutputTokens")),
        capabilities: LiveCapabilities {
            tools: flag("supportsToolCall"),
            vision: flag("supportsImages"),
            reasoning: flag("supportsReasoning"),
        },
        id,
        credits,
        credit_multiplier,
        charge_type,
    }
}

pub fn parse(payload: &Value, realm: Realm) -> Result<LiveModelCatalog, GatewayError> {
    if payload
        .get("code")
        .is_some_and(|code| code.as_i64() != Some(0))
    {
        return Err(invalid_catalog());
    }
    let (ids, metadata) = catalog::parse_config(payload).ok_or_else(invalid_catalog)?;
    // Keep every chat model the selected account is offered, including paid variants.
    let models = ids
        .into_iter()
        .filter(|id| catalog::is_chat_model(id))
        .filter(|id| {
            metadata
                .get(id)
                .and_then(|entry| entry.get("disabled"))
                .and_then(Value::as_bool)
                != Some(true)
        })
        .map(|id| model(id.clone(), metadata.get(&id)))
        .collect();
    Ok(LiveModelCatalog {
        realm: match realm {
            Realm::Cn => "cn",
            Realm::Intl => "intl",
        },
        fetched_at_ms: now_ms(),
        models,
    })
}

pub async fn fetch(
    client: &reqwest::Client,
    hosts: &Hosts,
    realm: Realm,
    account: &AccountSnapshot,
) -> Result<LiveModelCatalog, GatewayError> {
    fetch_url(
        client,
        hosts,
        realm,
        account,
        &hosts.chat_url(catalog::CONFIG_PATH),
    )
    .await
}

async fn fetch_url(
    client: &reqwest::Client,
    hosts: &Hosts,
    realm: Realm,
    account: &AccountSnapshot,
    url: &str,
) -> Result<LiveModelCatalog, GatewayError> {
    let token = account
        .access_token
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            GatewayError::new(
                ErrorKind::AuthExpired,
                ErrorScope::Request,
                502,
                "账号缺少 WorkBuddy 授权，请重新授权",
                false,
            )
        })?;
    let mut headers = identity::with_user_agent(
        identity::base_headers(hosts),
        &identity::desktop_user_agent(realm),
    );
    headers.extend([
        ("Authorization".into(), format!("Bearer {token}")),
        ("X-User-Id".into(), account.uid.clone()),
        (
            "Cache-Control".into(),
            "no-cache, no-store, max-age=0".into(),
        ),
        ("Pragma".into(), "no-cache".into()),
    ]);
    let url = format!("{url}?_relayai_ts={}", now_ms());
    let payload = tokio::time::timeout(
        Duration::from_secs(10),
        transport::get_json(client, &url, &headers),
    )
    .await
    .map_err(|_| GatewayError::network("WorkBuddy 模型查询超时，请重试"))?
    .map_err(|error| {
        // Upstream authorization is independent of the management session.
        if matches!(error.status, 401 | 403) {
            GatewayError::new(
                ErrorKind::AuthExpired,
                ErrorScope::Request,
                502,
                "WorkBuddy 拒绝了该账号的授权，请刷新账号凭据或重新授权后重试",
                false,
            )
        } else {
            error
        }
    })?;
    parse(&payload, realm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_live_models_and_live_credits_are_used() {
        let payload = json!({"code":0,"data":{
            "agents":[{"name":"cli","models":["hy3","hy3-x","glm-5.3","new-model","lite"]}],
            "models":[{"id":"hy3","credits":"x0.00 credits"},
                {"id":"hy3-x","credits":"x0.05 credits"},
                {"id":"glm-5.3","credits":"x0.12","maxInputTokens":1000000}]
        }});
        let catalog = parse(&payload, Realm::Cn).unwrap();
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["hy3", "hy3-x", "glm-5.3", "new-model"]
        );
        assert_eq!(catalog.models[0].charge_type, "free");
        assert_eq!(catalog.models[1].credit_multiplier, Some(0.05));
        assert_eq!(catalog.models[2].credit_multiplier, Some(0.12));
        assert_eq!(catalog.models[3].charge_type, "unknown");
        assert!(catalog.models[3].context_window.is_none());
        assert!(!catalog
            .models
            .iter()
            .any(|model| model.id == "hy4-preview-f"));
        let without_price = parse(
            &json!({"agents":[{"name":"cli","models":["hy3"]}]}),
            Realm::Intl,
        )
        .unwrap();
        assert_eq!(
            without_price.models[0].charge_type, "unknown",
            "snapshot free price must not leak in"
        );
        assert_eq!(without_price.realm, "intl");
    }

    #[test]
    fn invalid_or_missing_prices_are_never_free() {
        for value in [
            "free",
            "",
            "NaN",
            "inf",
            "-1",
            "x0.00 / x0.12",
            "x0 at night",
        ] {
            assert_eq!(multiplier(value), None, "{value}");
        }
        for value in ["x0", "x0.00 credits", "0.00", " X0.00 CREDITS "] {
            assert_eq!(multiplier(value), Some(0.0));
        }
        assert!(parse(&json!({"agents":[]}), Realm::Cn).is_err());
        assert!(parse(
            &json!({"code":401,"agents":[{"name":"cli","models":["hy3"]}]}),
            Realm::Cn
        )
        .is_err());
    }

    #[tokio::test]
    async fn fetch_reaches_upstream_on_every_call_and_does_not_fallback() {
        use axum::{
            extract::State,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::get,
            Json, Router,
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route("/v3/config", get(|State(calls): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
            assert_eq!(headers["cache-control"], "no-cache, no-store, max-age=0");
            assert_eq!(headers["authorization"], "Bearer test-token");
            assert_eq!(headers["x-user-id"], "selected-account");
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 3 {
                return (StatusCode::UNAUTHORIZED, Json(json!({"error":"expired account"}))).into_response();
            }
            Json(match call {
                0 => json!({"agents":[{"name":"cli","models":["hy3"]}],"models":[{"id":"hy3","credits":"x0.00 credits"}]}),
                1 => json!({"agents":[{"name":"cli","models":["hy3"]}],"models":[{"id":"hy3","credits":"x0.20 credits"}]}),
                _ => json!({"code":0,"agents":[]}),
            }).into_response()
        })).with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let hosts = identity::hosts(Realm::Cn, identity::Outbound::Desktop);
        let url = format!("http://{address}/v3/config");
        let account = AccountSnapshot {
            uid: "selected-account".into(),
            access_token: Some("test-token".into()),
            ..Default::default()
        };
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            fetch_url(&client, &hosts, Realm::Cn, &account, &url)
                .await
                .unwrap()
                .models[0]
                .charge_type,
            "free"
        );
        assert_eq!(
            fetch_url(&client, &hosts, Realm::Cn, &account, &url)
                .await
                .unwrap()
                .models[0]
                .credit_multiplier,
            Some(0.2)
        );
        assert!(fetch_url(&client, &hosts, Realm::Cn, &account, &url)
            .await
            .is_err());
        let unauthorized = fetch_url(&client, &hosts, Realm::Cn, &account, &url)
            .await
            .unwrap_err();
        assert_eq!(
            unauthorized.status, 502,
            "upstream 401 must not invalidate the management login"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();
    }

    // Opt in with selected accounts supplied only through the process environment.
    #[tokio::test]
    #[ignore = "requires TEST_WORKBUDDY_LIVE_ACCOUNTS with authorized account credentials"]
    async fn official_cn_and_intl_live_query() {
        let accounts: Value =
            serde_json::from_str(&std::env::var("TEST_WORKBUDDY_LIVE_ACCOUNTS").unwrap()).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for entry in accounts.as_array().unwrap() {
            let realm = Realm::parse(entry["realm"].as_str().unwrap());
            let hosts = identity::hosts(realm, identity::Outbound::Desktop);
            let account = AccountSnapshot {
                uid: entry["uid"].as_str().unwrap().into(),
                access_token: Some(entry["token"].as_str().unwrap().into()),
                ..Default::default()
            };
            let catalog = fetch(&client, &hosts, realm, &account).await.unwrap();
            assert!(!catalog.models.is_empty());
            assert!(catalog
                .models
                .iter()
                .any(|model| model.credit_multiplier.is_some()));
            println!(
                "official {}: {} chat models, {} free",
                catalog.realm,
                catalog.models.len(),
                catalog
                    .models
                    .iter()
                    .filter(|model| model.charge_type == "free")
                    .count()
            );
        }
    }
}
