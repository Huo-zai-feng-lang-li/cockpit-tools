use crate::models::codex::CodexAccount;
#[cfg(target_os = "windows")]
use crate::modules::logger;
#[cfg(target_os = "windows")]
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use serde_json::json;
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::time::Duration;
#[cfg(target_os = "windows")]
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

#[cfg(target_os = "windows")]
const INSPECTOR_HTTP_TIMEOUT_SECS: u64 = 2;
#[cfg(target_os = "windows")]
const INSPECTOR_REQUEST_TIMEOUT_SECS: u64 = 8;
#[cfg(target_os = "windows")]
static LAST_INSPECTOR_ENDPOINT: Mutex<Option<InspectorEndpoint>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexHotSwitchRuntimeResult {
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<Value>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct RuntimeTokens {
    access_token: String,
    chatgpt_account_id: String,
    chatgpt_plan_type: Option<String>,
}

pub async fn hot_switch_account(
    account: &CodexAccount,
) -> Result<CodexHotSwitchRuntimeResult, String> {
    if account.is_api_key_auth() {
        return Err("Codex API Key 账号不支持运行时无感热切".to_string());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = account;
        Err("Codex 无感热切仅支持 Windows 上的 Antigravity 插件 Inspector".to_string())
    }

    #[cfg(target_os = "windows")]
    {
        let tokens = build_runtime_tokens(account)?;

        match hot_switch_via_antigravity_inspector(account, &tokens).await {
            Ok(result) => return Ok(result),
            Err(err) => logger::log_warn(&format!(
                "[Codex HotSwitch] Antigravity 插件 Inspector 热切不可用: {}",
                err
            )),
        }

        Err("未发现可热切的 Antigravity Codex 插件 Inspector 运行时".to_string())
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Deserialize)]
struct InspectorPort {
    pid: u32,
    port: u16,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct InspectorEndpoint {
    pid: u32,
    port: u16,
    ws_url: String,
}

#[cfg(target_os = "windows")]
struct CdpClient {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
}

#[cfg(target_os = "windows")]
async fn hot_switch_via_antigravity_inspector(
    account: &CodexAccount,
    tokens: &RuntimeTokens,
) -> Result<CodexHotSwitchRuntimeResult, String> {
    if let Some(endpoint) = cached_inspector_endpoint() {
        match hot_switch_with_inspector_endpoint(&endpoint, account, tokens).await {
            Ok(rate_limits) => return Ok(build_hot_switch_result(&endpoint, account, rate_limits)),
            Err(err) => logger::log_info(&format!(
                "[Codex HotSwitch] 缓存 Inspector 端点失效: pid={}, port={}, error={}",
                endpoint.pid, endpoint.port, err
            )),
        }
    }

    let endpoints = discover_antigravity_inspector_endpoints().await?;
    if endpoints.is_empty() {
        return Err("未发现 Antigravity 扩展宿主 Inspector 端口".to_string());
    }

    let mut errors = Vec::new();
    for endpoint in endpoints {
        match hot_switch_with_inspector_endpoint(&endpoint, account, tokens).await {
            Ok(rate_limits) => {
                remember_inspector_endpoint(&endpoint);
                return Ok(build_hot_switch_result(&endpoint, account, rate_limits));
            }
            Err(err) => errors.push(format!(
                "pid={},port={}: {}",
                endpoint.pid, endpoint.port, err
            )),
        }
    }

    Err(format!(
        "Antigravity Inspector 均未完成热切: {}",
        errors.join(" | ")
    ))
}

#[cfg(target_os = "windows")]
fn cached_inspector_endpoint() -> Option<InspectorEndpoint> {
    LAST_INSPECTOR_ENDPOINT.lock().ok()?.clone()
}

#[cfg(target_os = "windows")]
fn remember_inspector_endpoint(endpoint: &InspectorEndpoint) {
    if let Ok(mut cached) = LAST_INSPECTOR_ENDPOINT.lock() {
        *cached = Some(endpoint.clone());
    }
}

#[cfg(target_os = "windows")]
fn build_hot_switch_result(
    endpoint: &InspectorEndpoint,
    account: &CodexAccount,
    rate_limits: Option<Value>,
) -> CodexHotSwitchRuntimeResult {
    logger::log_info(&format!(
        "[Codex HotSwitch] Antigravity 插件运行时热切完成: pid={}, port={}, email={}",
        endpoint.pid, endpoint.port, account.email
    ));
    CodexHotSwitchRuntimeResult {
        runtime: format!(
            "antigravity-codex-extension-inspector:pid={},port={}",
            endpoint.pid, endpoint.port
        ),
        rate_limits,
    }
}

#[cfg(target_os = "windows")]
async fn discover_antigravity_inspector_endpoints() -> Result<Vec<InspectorEndpoint>, String> {
    let ports = tokio::task::spawn_blocking(discover_antigravity_inspector_ports)
        .await
        .map_err(|e| format!("Antigravity Inspector 端口探测任务失败: {}", e))??;
    if ports.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(INSPECTOR_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建 Inspector HTTP 客户端失败: {}", e))?;

    let mut endpoints = Vec::new();
    for port in ports {
        match read_inspector_ws_url(&client, port.port).await {
            Ok(Some(ws_url)) => endpoints.push(InspectorEndpoint {
                pid: port.pid,
                port: port.port,
                ws_url,
            }),
            Ok(None) => {}
            Err(err) => logger::log_info(&format!(
                "[Codex HotSwitch] 跳过非 Inspector 端口: pid={}, port={}, error={}",
                port.pid, port.port, err
            )),
        }
    }
    Ok(endpoints)
}

#[cfg(target_os = "windows")]
fn discover_antigravity_inspector_ports() -> Result<Vec<InspectorPort>, String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = r#"
$items = @()
$processes = Get-CimInstance Win32_Process -Filter "Name='Antigravity.exe'" |
  Where-Object {
    $_.CommandLine -match 'node\.mojom\.NodeService' -and
    $_.CommandLine -match '--inspect-port'
  }
foreach ($process in $processes) {
  $connections = Get-NetTCPConnection -State Listen -OwningProcess $process.ProcessId -ErrorAction SilentlyContinue |
    Where-Object { $_.LocalAddress -eq '127.0.0.1' -or $_.LocalAddress -eq '::1' }
  foreach ($connection in $connections) {
    $items += [pscustomobject]@{
      pid = [int]$process.ProcessId
      port = [int]$connection.LocalPort
    }
  }
}
$items | ConvertTo-Json -Compress
"#;

    let command_text = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; {}",
        script
    );
    let output = std::process::Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", &command_text])
        .output()
        .map_err(|e| format!("执行 Antigravity Inspector 端口探测失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Antigravity Inspector 端口探测失败: {}",
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }

    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("解析 Antigravity Inspector 端口列表失败: {}", e))?;
    if value.is_array() {
        serde_json::from_value(value)
            .map_err(|e| format!("解析 Antigravity Inspector 端口数组失败: {}", e))
    } else {
        serde_json::from_value::<InspectorPort>(value)
            .map(|item| vec![item])
            .map_err(|e| format!("解析 Antigravity Inspector 端口对象失败: {}", e))
    }
}

#[cfg(target_os = "windows")]
async fn read_inspector_ws_url(
    client: &reqwest::Client,
    port: u16,
) -> Result<Option<String>, String> {
    let url = format!("http://127.0.0.1:{}/json/list", port);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求 {} 失败: {}", url, e))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("解析 {} 响应失败: {}", url, e))?;
    let entries = value
        .as_array()
        .ok_or_else(|| format!("{} 响应不是数组", url))?;
    Ok(entries
        .iter()
        .find_map(|entry| entry.get("webSocketDebuggerUrl").and_then(Value::as_str))
        .map(ToString::to_string))
}

#[cfg(target_os = "windows")]
async fn hot_switch_with_inspector_endpoint(
    endpoint: &InspectorEndpoint,
    account: &CodexAccount,
    tokens: &RuntimeTokens,
) -> Result<Option<Value>, String> {
    let mut client = CdpClient::connect(&endpoint.ws_url).await?;
    let instance_object_id = client.find_codex_mcp_instance().await?;
    let payload = json!({
        "accessToken": tokens.access_token,
        "chatgptAccountId": tokens.chatgpt_account_id,
        "chatgptPlanType": tokens.chatgpt_plan_type,
        "expectedEmail": account.email,
    });
    let response = client
        .call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": instance_object_id,
                "functionDeclaration": HOT_SWITCH_FUNCTION,
                "arguments": [{ "value": payload }],
                "returnByValue": true,
                "awaitPromise": true
            }),
        )
        .await?;

    if let Some(exception) = response.get("exceptionDetails") {
        return Err(format!(
            "Inspector 执行热切异常: {}",
            format_cdp_exception(exception)
        ));
    }

    let result = response
        .get("result")
        .ok_or_else(|| "Inspector 热切响应缺少 result".to_string())?;
    if result.get("subtype").and_then(Value::as_str) == Some("error") {
        return Err(format!(
            "Inspector 热切返回错误对象: {}",
            result
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
        ));
    }

    let value = result
        .get("value")
        .cloned()
        .ok_or_else(|| "Inspector 热切响应缺少 value".to_string())?;
    let account_read = value
        .get("accountRead")
        .ok_or_else(|| "Inspector 热切响应缺少 accountRead".to_string())?;
    verify_runtime_account(account_read, account)?;

    if let Some(error) = value.get("rateLimitsError").and_then(Value::as_str) {
        logger::log_warn(&format!(
            "[Codex HotSwitch] Inspector 热切后读取 rateLimits 失败: {}",
            error
        ));
    }

    Ok(value
        .get("rateLimits")
        .cloned()
        .filter(|item| !item.is_null()))
}

#[cfg(target_os = "windows")]
impl CdpClient {
    async fn connect(ws_url: &str) -> Result<Self, String> {
        let (socket, _) = connect_async(ws_url)
            .await
            .map_err(|e| format!("连接 Antigravity Inspector WebSocket 失败: {}", e))?;
        Ok(Self { socket, next_id: 1 })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({
            "id": id,
            "method": method,
            "params": params
        });
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .map_err(|e| format!("写入 Inspector WebSocket 失败: {}", e))?;

        let wait = async {
            loop {
                let Some(message) = self.socket.next().await else {
                    return Err(format!(
                        "Inspector WebSocket 已断开，未收到 {} 响应",
                        method
                    ));
                };
                let message =
                    message.map_err(|e| format!("读取 Inspector WebSocket 失败: {}", e))?;
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Message::Close(_) => {
                        return Err(format!(
                            "Inspector WebSocket 已关闭，未收到 {} 响应",
                            method
                        ));
                    }
                    _ => continue,
                };
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("解析 Inspector 响应失败: {}", e))?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(format!("Inspector {} 调用失败: {}", method, error));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        };

        tokio::time::timeout(Duration::from_secs(INSPECTOR_REQUEST_TIMEOUT_SECS), wait)
            .await
            .map_err(|_| format!("Inspector {} 调用超时", method))?
    }

    async fn evaluate_object(&mut self, expression: &str) -> Result<String, String> {
        let value = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": false,
                    "replMode": true
                }),
            )
            .await?;
        object_id_at(&value, &["result"])
            .map(ToString::to_string)
            .ok_or_else(|| "Inspector evaluate 未返回 objectId".to_string())
    }

    async fn get_properties(
        &mut self,
        object_id: &str,
        own_properties: bool,
    ) -> Result<Value, String> {
        self.call(
            "Runtime.getProperties",
            json!({
                "objectId": object_id,
                "ownProperties": own_properties,
                "generatePreview": false
            }),
        )
        .await
    }

    async fn find_codex_mcp_instance(&mut self) -> Result<String, String> {
        let activate_id = self.evaluate_object(ACTIVATE_FUNCTION_EXPRESSION).await?;
        let activate_props = self.get_properties(&activate_id, false).await?;
        let scopes_id = internal_object_id(&activate_props, "[[Scopes]]")
            .ok_or_else(|| "未找到 Codex 扩展 activate [[Scopes]]".to_string())?
            .to_string();
        let scopes = self.get_properties(&scopes_id, true).await?;
        let closure_scope_id = property_object_id(&scopes, "0")
            .ok_or_else(|| "未找到 Codex 扩展闭包 scope".to_string())?
            .to_string();
        let closure_props = self.get_properties(&closure_scope_id, true).await?;
        let class_id = property_object_id(&closure_props, "I_")
            .ok_or_else(|| "未找到 CodexMcpConnection 类".to_string())?
            .to_string();
        let class_props = self.get_properties(&class_id, false).await?;
        let prototype_id = property_object_id(&class_props, "prototype")
            .ok_or_else(|| "未找到 CodexMcpConnection prototype".to_string())?
            .to_string();
        let queried = self
            .call(
                "Runtime.queryObjects",
                json!({
                    "prototypeObjectId": prototype_id
                }),
            )
            .await?;
        let objects_id = object_id_at(&queried, &["objects"])
            .ok_or_else(|| "Inspector queryObjects 未返回对象数组".to_string())?
            .to_string();
        let objects = self.get_properties(&objects_id, true).await?;
        find_indexed_object_id_by_class(&objects, "I_")
            .ok_or_else(|| "当前 Antigravity 扩展宿主中未找到 CodexMcpConnection 实例".to_string())
    }
}

#[cfg(target_os = "windows")]
fn property_object_id<'a>(properties: &'a Value, name: &str) -> Option<&'a str> {
    properties
        .get("result")?
        .as_array()?
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")?
        .get("objectId")?
        .as_str()
}

#[cfg(target_os = "windows")]
fn internal_object_id<'a>(properties: &'a Value, name: &str) -> Option<&'a str> {
    properties
        .get("internalProperties")?
        .as_array()?
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")?
        .get("objectId")?
        .as_str()
}

#[cfg(target_os = "windows")]
fn object_id_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.get("objectId")?.as_str()
}

#[cfg(target_os = "windows")]
fn find_indexed_object_id_by_class(properties: &Value, class_name: &str) -> Option<String> {
    let mut entries = properties
        .get("result")?
        .as_array()?
        .iter()
        .collect::<Vec<_>>();
    entries.sort_by_key(|item| {
        item.get("name")
            .and_then(Value::as_str)
            .and_then(|name| name.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });

    entries.into_iter().find_map(|item| {
        let name = item.get("name").and_then(Value::as_str)?;
        name.parse::<usize>().ok()?;
        let value = item.get("value")?;
        if value.get("className").and_then(Value::as_str) == Some(class_name) {
            value
                .get("objectId")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        } else {
            None
        }
    })
}

#[cfg(target_os = "windows")]
fn format_cdp_exception(exception: &Value) -> String {
    exception
        .get("exception")
        .and_then(|item| item.get("description"))
        .and_then(Value::as_str)
        .or_else(|| exception.get("text").and_then(Value::as_str))
        .unwrap_or("<unknown>")
        .to_string()
}

#[cfg(target_os = "windows")]
const ACTIVATE_FUNCTION_EXPRESSION: &str = r#"(() => {
  const moduleBuiltin = process.getBuiltinModule('module');
  const req = moduleBuiltin.createRequire('file:///D:/Antigravity/resources/app/out/main.js');
  const key = Object.keys(req.cache).find((candidate) =>
    candidate.includes('openai.chatgpt-') && candidate.endsWith('extension.js')
  );
  if (!key) {
    throw new Error('Codex extension module is not loaded');
  }
  return req.cache[key].exports.activate;
})()"#;

#[cfg(target_os = "windows")]
const HOT_SWITCH_FUNCTION: &str = r#"async function(payload) {
  if (!this.initialized) {
    throw new Error('Codex app-server is not initialized');
  }
  if (this.__cockpitHotSwitchProvider && typeof this.__cockpitHotSwitchProvider.dispose === 'function') {
    try { this.__cockpitHotSwitchProvider.dispose(); } catch {}
  }

  const namespace = 'cockpit-hot-switch';
  const pending = new Map();
  const notifications = [];
  const provider = {
    onResult: (message) => {
      const key = String(message.id);
      const waiter = pending.get(key);
      if (!waiter) return;
      pending.delete(key);
      if (message.error) {
        const errorText = typeof message.error === 'string'
          ? message.error
          : JSON.stringify(message.error);
        waiter.reject(new Error(errorText));
      } else {
        waiter.resolve(message.result ?? null);
      }
    },
    onRequest: (message) => {
      if (message.method !== 'account/chatgptAuthTokens/refresh') return;
      this.sendResponse(message.id, {
        accessToken: payload.accessToken,
        chatgptAccountId: payload.chatgptAccountId,
        chatgptPlanType: payload.chatgptPlanType ?? null
      });
    },
    onNotification: (message) => {
      notifications.push({ method: message.method, params: message.params });
    }
  };

  const disposable = this.registerProvider(namespace, provider);
  this.__cockpitHotSwitchProvider = disposable;

  const request = (method, params, timeoutMs = 4000) => new Promise((resolve, reject) => {
    const id = `${Date.now()}-${Math.random()}`;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timeout`));
    }, timeoutMs);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      }
    });
    this.sendRequest(namespace, id, method, params);
  });

  const waitForLoginSettle = () => new Promise((resolve, reject) => {
    const startedAt = Date.now();
    const tick = () => {
      const notification = notifications.find((item) =>
        item.method === 'account/login/completed' || item.method === 'account/updated'
      );
      if (notification) {
        if (
          notification.method === 'account/login/completed' &&
          notification.params &&
          notification.params.success === false
        ) {
          reject(new Error(notification.params.error || 'account/login/completed failed'));
        } else {
          resolve(notification);
        }
        return;
      }
      if (Date.now() - startedAt > 1500) {
        resolve(null);
        return;
      }
      setTimeout(tick, 50);
    };
    tick();
  });

  await request('account/login/start', {
    type: 'chatgptAuthTokens',
    accessToken: payload.accessToken,
    chatgptAccountId: payload.chatgptAccountId,
    chatgptPlanType: payload.chatgptPlanType ?? null
  });
  await waitForLoginSettle();

  const accountRead = await request('account/read', { refresh: false });
  const actualEmail = accountRead?.account?.email || '';
  if (actualEmail.toLowerCase() !== String(payload.expectedEmail).toLowerCase()) {
    throw new Error(`runtime account mismatch: ${actualEmail} != ${payload.expectedEmail}`);
  }

  let rateLimits = null;
  let rateLimitsError = null;
  try {
    rateLimits = await request('account/rateLimits/read', {});
  } catch (error) {
    rateLimitsError = String(error?.message || error);
  }

  return {
    ok: true,
    accountRead,
    rateLimits,
    rateLimitsError
  };
}"#;

#[cfg(target_os = "windows")]
fn build_runtime_tokens(account: &CodexAccount) -> Result<RuntimeTokens, String> {
    let access_token = account.tokens.access_token.trim();
    if access_token.is_empty() {
        return Err("Codex OAuth 账号缺少 access_token，无法运行时热切".to_string());
    }

    let chatgpt_account_id = account
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(account.id.as_str())
        .to_string();

    Ok(RuntimeTokens {
        access_token: access_token.to_string(),
        chatgpt_account_id,
        chatgpt_plan_type: normalize_plan_type(account.plan_type.as_deref()),
    })
}

#[cfg(target_os = "windows")]
fn normalize_plan_type(plan_type: Option<&str>) -> Option<String> {
    let normalized = plan_type?.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    let mapped = match normalized.as_str() {
        "basic" => "free",
        "plus"
        | "pro"
        | "team"
        | "business"
        | "enterprise"
        | "edu"
        | "go"
        | "prolite"
        | "self_serve_business_usage_based"
        | "enterprise_cbp_usage_based" => normalized.as_str(),
        _ => "unknown",
    };
    Some(mapped.to_string())
}

#[cfg(target_os = "windows")]
fn verify_runtime_account(value: &Value, target: &CodexAccount) -> Result<(), String> {
    let account = value
        .get("account")
        .ok_or_else(|| "Codex App Server account/read 缺少 account 字段".to_string())?;
    let runtime_type = account.get("type").and_then(Value::as_str).unwrap_or("");
    if runtime_type != "chatgpt" {
        return Err(format!(
            "运行时账号类型不是 ChatGPT: {}",
            if runtime_type.is_empty() {
                "<empty>"
            } else {
                runtime_type
            }
        ));
    }

    let runtime_email = account
        .get("email")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex App Server account/read 未返回 email".to_string())?;
    if !runtime_email.eq_ignore_ascii_case(&target.email) {
        return Err(format!(
            "运行时账号回读不一致: expected={}, actual={}",
            target.email, runtime_email
        ));
    }

    Ok(())
}
