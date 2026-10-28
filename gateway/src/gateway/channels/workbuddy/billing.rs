//! 计费侧额度查询（国内版 = 积分 / 资源包）。
//!
//! 三步（与参考实现一致）：
//! ① `get-user-resource-summary` 拿资源包编号与是否付费用户；
//! ② `get-user-resource-free-packages` / `get-user-resource-paid-packages` 拿每个包的
//!    周期容量（Size / Remain / Used，都有 Precise 变体，两种都要兼容）；
//! ③ 汇总成渠道无关的 [`QuotaSnapshot`]，面板只按 label + unit 展示。
//!
//! 已知简化：企业账号在个人计费接口上拿不到资源包（全是 0），
//! 这里只标注一下，等真需要企业用量时再接 `get-enterprise-user-usage`。

use serde_json::{json, Value};

use crate::gateway::{
    channels::{AccountSnapshot, QuotaSnapshot},
    errors::GatewayError,
    timeutil::now_ms,
    transport,
};

use super::identity::{self, Hosts};

const SUMMARY_PATH: &str = "/billing/meter/get-user-resource-summary";
const FREE_PATH: &str = "/billing/meter/get-user-resource-free-packages";
const PAID_PATH: &str = "/billing/meter/get-user-resource-paid-packages";

pub async fn fetch(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Result<Option<QuotaSnapshot>, GatewayError> {
    // 计费头组比对话头更轻：带 X-Client-Platform: web，无企业身份时显式声明。
    let headers = identity::billing_headers(hosts, account, Some("web"));
    let summary = transport::post_json(client, &hosts.billing_url(SUMMARY_PATH), &headers, &json!({}))
        .await?;
    if summary.get("code").and_then(Value::as_i64) != Some(0) {
        return Ok(None);
    }
    let data = summary.get("data").cloned().unwrap_or(Value::Null);
    let codes: Vec<String> = data
        .get("Packages")
        .and_then(Value::as_array)
        .map(|packages| {
            packages
                .iter()
                .filter_map(|package| {
                    package
                        .get("PackageCode")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let paid_user = data
        .get("IsPaidUser")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // 上游把每个资源包的容量直接放在 summary 的 Packages[] 里：
    // CycleTotalCapacity / CycleRemainCapacity / CycleUsedCapacity（字符串数字）。
    // 这是**唯一可靠的来源** —— 明细接口（free/paid-packages）才用 CycleCapacity* 那套命名，
    // 而且没有 PackageCode 时根本不返回明细。早期版本只看明细，于是积分永远是 0。
    let mut total = 0.0f64;
    let mut remaining = 0.0f64;
    let mut used = 0.0f64;
    let mut summary_packages = 0usize;
    let mut unit = String::new();
    for package in data
        .get("Packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let pick = |key: &str| -> f64 {
            package
                .get(key)
                .map(|value| match value {
                    Value::Number(number) => number.as_f64().unwrap_or(0.0),
                    Value::String(text) => text.parse::<f64>().unwrap_or(0.0),
                    _ => 0.0,
                })
                .unwrap_or(0.0)
        };
        let size = pick("CycleTotalCapacity");
        let remain = pick("CycleRemainCapacity");
        let spent = pick("CycleUsedCapacity");
        if size <= 0.0 && remain <= 0.0 && spent <= 0.0 {
            continue;
        }
        summary_packages += 1;
        total += size;
        remaining += remain;
        used += spent;
        if unit.is_empty() {
            if let Some(value) = package.get("CapacityUnit").and_then(Value::as_str) {
                if !value.is_empty() {
                    unit = value.to_string();
                }
            }
        }
    }
    let mut free_count = 0usize;
    let mut paid_count = 0usize;

    for (path, status, need_renew, is_free) in [
        (FREE_PATH, json!([0]), false, true),
        (PAID_PATH, json!([0, 3]), true, false),
    ] {
        let body = json!({
            "PackageCodes": codes,
            "PageNumber": 1,
            "PageSize": 200,
            "Status": status,
            "NeedRenewInfo": need_renew,
        });
        let Ok(payload) = transport::post_json(client, &hosts.billing_url(path), &headers, &body).await
        else {
            // 某类包查不到不影响整体：有哪类就报哪类。
            continue;
        };
        for package in collect_packages(&payload) {
            let (size, remain, spent) = capacities(package);
            if size <= 0.0 && remain <= 0.0 {
                continue;
            }
            total += size;
            remaining += remain;
            used += spent;
            if unit.is_empty() {
                if let Some(value) = package.get("CapacityUnit").and_then(Value::as_str) {
                    if !value.is_empty() {
                        unit = value.to_string();
                    }
                }
            }
            if is_free {
                free_count += 1;
            } else {
                paid_count += 1;
            }
        }
    }

    let mut details = vec![
        ("资源包".to_string(), format!("{summary_packages} 个")),
        ("免费包明细".to_string(), format!("{free_count} 个")),
        ("付费包明细".to_string(), format!("{paid_count} 个")),
    ];
    if paid_user {
        details.push(("账号类型".to_string(), "付费用户".to_string()));
    }
    if summary_packages == 0 {
        details.push((
            "说明".to_string(),
            "个人计费接口没有资源包；企业账号请在企业用量里查看".to_string(),
        ));
    }

    Ok(Some(QuotaSnapshot {
        label: "积分".to_string(),
        unit: if unit.is_empty() {
            "credits".to_string()
        } else {
            unit
        },
        remaining: Some(round2(remaining)),
        total: (total > 0.0).then(|| round2(total)),
        used: (used > 0.0).then(|| round2(used)),
        details,
        fetched_at_ms: now_ms(),
    }))
}

/// 在响应里**递归**找资源包条目：免费/付费两个接口的容器层级不完全一致，
/// 按"含容量字段的对象"识别比猜字段名稳。
fn collect_packages(payload: &Value) -> Vec<&Value> {
    let mut found = Vec::new();
    fn walk<'a>(node: &'a Value, found: &mut Vec<&'a Value>) {
        match node {
            Value::Object(map) => {
                let looks_like_package = map.keys().any(|key| {
                    key.starts_with("CycleCapacity") || key.starts_with("Capacity")
                }) || map.contains_key("PackageCode");
                if looks_like_package && !map.is_empty() {
                    found.push(node);
                }
                for value in map.values() {
                    walk(value, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, found);
                }
            }
            _ => {}
        }
    }
    walk(payload, &mut found);
    found
}

/// 容量三件套：优先 Precise 变体；缺一个就用另外两个补（参考实现同款修正）。
fn capacities(package: &Value) -> (f64, f64, f64) {
    let value = |keys: [&str; 4]| -> f64 {
        for key in keys {
            if let Some(raw) = package.get(key) {
                let parsed = match raw {
                    Value::Number(number) => number.as_f64(),
                    Value::String(text) => text.parse::<f64>().ok(),
                    _ => None,
                };
                if let Some(parsed) = parsed {
                    return parsed;
                }
            }
        }
        0.0
    };
    let size = value([
        "CycleCapacitySizePrecise",
        "CycleCapacitySize",
        "CapacitySizePrecise",
        "CapacitySize",
    ]);
    let mut remain = value([
        "CycleCapacityRemainPrecise",
        "CycleCapacityRemain",
        "CapacityRemainPrecise",
        "CapacityRemain",
    ]);
    let mut used = value([
        "CycleCapacityUsedPrecise",
        "CycleCapacityUsed",
        "CapacityUsedPrecise",
        "CapacityUsed",
    ]);

    if size > 0.0 && used <= 0.0 && remain <= size {
        used = (size - remain).max(0.0);
    } else if size > 0.0 && remain <= 0.0 && used < size {
        remain = (size - used).max(0.0);
    }
    (size, remain, used)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_variants_and_fixups_are_handled() {
        // 只有 Size + Remain：Used 要能推出来。
        let package = json!({"CycleCapacitySizePrecise": "1000", "CycleCapacityRemain": 250});
        assert_eq!(capacities(&package), (1000.0, 250.0, 750.0));

        // 只有 Size + Used：Remain 要能推出来。
        let package = json!({"CapacitySize": 100, "CapacityUsedPrecise": 40});
        assert_eq!(capacities(&package), (100.0, 60.0, 40.0));

        // 数字与字符串都要认。
        let package = json!({"CycleCapacitySize": "1.5", "CycleCapacityRemain": "0.5"});
        assert_eq!(capacities(&package), (1.5, 0.5, 1.0));

        assert_eq!(capacities(&json!({})), (0.0, 0.0, 0.0));
    }

    #[test]
    fn summary_package_capacities_are_read_from_the_real_shape() {
        // 上游真实响应（2026-10-10 实测）：容量在 data.Packages[] 的
        // CycleTotalCapacity / CycleRemainCapacity / CycleUsedCapacity 上，
        // 值是**字符串数字**。早期版本只看明细接口 → 积分永远是 0。
        let data = json!({
            "IsPaidUser": false,
            "Packages": [
                {"CapacityUnit": "credits", "CycleTotalCapacity": "2200",
                 "CycleRemainCapacity": "847.79000074", "CycleUsedCapacity": "1352.20999926",
                 "PackageCode": "TCACA_code_007_x"},
                {"CapacityUnit": "credits", "CycleTotalCapacity": "500",
                 "CycleRemainCapacity": "500", "CycleUsedCapacity": "0",
                 "PackageCode": "TCACA_code_008_y"}
            ]
        });
        let mut total = 0.0;
        let mut remaining = 0.0;
        let mut used = 0.0;
        for package in data["Packages"].as_array().unwrap() {
            let pick = |key: &str| -> f64 {
                package
                    .get(key)
                    .map(|value| match value {
                        Value::Number(number) => number.as_f64().unwrap_or(0.0),
                        Value::String(text) => text.parse::<f64>().unwrap_or(0.0),
                        _ => 0.0,
                    })
                    .unwrap_or(0.0)
            };
            total += pick("CycleTotalCapacity");
            remaining += pick("CycleRemainCapacity");
            used += pick("CycleUsedCapacity");
        }
        assert!((total - 2700.0).abs() < 0.01, "总量应为 2700，实际 {total}");
        assert!((remaining - 1347.79).abs() < 0.01, "剩余应为 1347.79，实际 {remaining}");
        assert!((used - 1352.21).abs() < 0.01, "已用应为 1352.21，实际 {used}");
        // 关键：这三个字段名一旦写错（如写成 CycleCapacitySize），这里就会失败。
        assert!(remaining > 0.0, "剩余为 0 说明字段名对不上上游");
    }

    #[test]
    fn packages_are_found_at_any_depth() {
        let payload = json!({
            "code": 0,
            "data": {"Accounts": [
                {"PackageCode": "p1", "CycleCapacitySizePrecise": 10, "CycleCapacityRemainPrecise": 4}
            ]}
        });
        let packages = collect_packages(&payload);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0]["PackageCode"], "p1");

        // 容器换成 Packages 也一样能找到。
        let payload = json!({"data": {"Packages": [{"PackageCode": "p2", "CapacitySize": 5}]}});
        assert_eq!(collect_packages(&payload).len(), 1);
    }
}

/// 手动运行的探针：用真实账号打一次上游计费接口，打印原始响应。
///
/// 用途：排查"积分一直是 0"这类问题——只有看到上游真实键名才能确认解析对不对。
/// 运行：`cargo test probe_real_summary -- --ignored --nocapture`
///
/// 需要 `DATABASE_URL` 与 `RELAYAI_MASTER_KEY`（与生产同一条读取路径）。
#[cfg(test)]
mod probe {
    use crate::gateway::{channels::AccountSnapshot, storage::Storage};

    #[tokio::test]
    #[ignore = "需要真实数据库与 wb-cn 账号，手动执行"]
    async fn probe_real_summary() {
        let data_source = std::env::var("DATABASE_URL").expect("需要 DATABASE_URL");
        let master_key = crate::secrets::load_master_key().expect("需要 RELAYAI_MASTER_KEY");

        let storage = Storage::open(&data_source, master_key)
            .await
            .expect("打开库");
        let records = storage.list_accounts().await.expect("读账号");
        let Some(record) = records.into_iter().find(|r| r.channel_id == "wb-cn") else {
            panic!("库里没有 wb-cn 账号");
        };
        let credential = record.credential.expect("没有凭据");
        let account = AccountSnapshot {
            channel_id: record.channel_id.clone(),
            uid: record.uid.clone(),
            access_token: Some(credential.access_token.clone()),
            refresh_token: credential.refresh_token.clone(),
            enterprise_id: credential.enterprise_id.clone(),
            ..Default::default()
        };
        let hosts = super::super::identity::hosts(
            super::super::identity::Realm::Cn,
            super::super::identity::Outbound::Desktop,
        );
        let headers = super::super::identity::billing_headers(&hosts, &account, Some("web"));
        let client = reqwest::Client::new();
        let payload = crate::gateway::transport::post_json(
            &client,
            &hosts.billing_url("/billing/meter/get-user-resource-summary"),
            &headers,
            &serde_json::json!({}),
        )
        .await
        .expect("summary 请求");
        println!("=== summary 原始响应 ===");
        println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());

        // 再走完整解析链路，看最终快照。
        let snapshot = super::fetch(&client, &hosts, &account)
            .await
            .expect("额度查询");
        println!("=== 解析后的额度快照 ===");
        println!("{}", serde_json::to_string_pretty(&snapshot).unwrap_or_default());
        let snapshot = snapshot.expect("应有快照");
        assert!(
            snapshot.remaining.unwrap_or(0.0) > 0.0,
            "剩余积分不该是 0（字段名与上游不符时会失败）"
        );
    }
}
