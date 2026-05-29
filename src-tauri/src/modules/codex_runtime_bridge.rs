use crate::models::codex::CodexAccount;
#[cfg(target_os = "windows")]
use crate::modules::{logger, websocket};
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
const PLUGIN_SWITCH_CONNECT_WAIT_MS: u64 = 1_500;
#[cfg(target_os = "windows")]
const PLUGIN_SWITCH_TIMEOUT_MS: u64 = 45_000;
#[cfg(target_os = "windows")]
const INSPECTOR_CONNECT_TIMEOUT_SECS: u64 = 2;
#[cfg(target_os = "windows")]
const INSPECTOR_REQUEST_TIMEOUT_SECS: u64 = 8;
#[cfg(target_os = "windows")]
const MAX_INSPECTOR_PORTS_TO_SCAN: usize = 12;
#[cfg(target_os = "windows")]
const MAX_INSPECTOR_ENDPOINTS_TO_TRY: usize = 3;
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
        match hot_switch_via_plugin_ws(account).await {
            Ok(result) => return Ok(result),
            Err(err) => logger::log_warn(&format!(
                "[Codex HotSwitch] Antigravity Cockpit 插件 WS 热切不可用，准备降级 Inspector: {}",
                err
            )),
        }

        let tokens = build_runtime_tokens(account)?;

        let err = match hot_switch_via_antigravity_inspector(account, &tokens).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                logger::log_warn(&format!(
                    "[Codex HotSwitch] Antigravity 插件 Inspector 热切不可用: {}",
                    err
                ));
                err
            }
        };

        Err(err)
    }
}

pub async fn warm_up_runtime() -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        Err("Codex runtime 预热仅支持 Windows 上的 Antigravity 插件 Inspector".to_string())
    }

    #[cfg(target_os = "windows")]
    {
        match warm_up_via_plugin_ws().await {
            Ok(runtime) => return Ok(runtime),
            Err(err) => logger::log_info(&format!(
                "[Codex HotSwitch] Antigravity Cockpit 插件 WS 预热不可用，准备降级 Inspector: {}",
                err
            )),
        }

        match warm_up_via_antigravity_inspector().await {
            Ok(runtime) => Ok(runtime),
            Err(err) => {
                logger::log_warn(&format!(
                    "[Codex HotSwitch] 预热 Antigravity 插件运行时失败: {}",
                    err
                ));
                Err(err)
            }
        }
    }
}

#[cfg(target_os = "windows")]
async fn hot_switch_via_plugin_ws(
    account: &CodexAccount,
) -> Result<CodexHotSwitchRuntimeResult, String> {
    let client_count = websocket::wait_for_connected_clients(PLUGIN_SWITCH_CONNECT_WAIT_MS).await?;
    let response = websocket::request_plugin_switch_account(
        &account.email,
        "seamless",
        "manual",
        "codex.hot_switch",
        "codex_account_page_hot_switch",
        PLUGIN_SWITCH_TIMEOUT_MS,
    )
    .await?;

    validate_plugin_switch_response(&response, account)?;
    logger::log_info(&format!(
        "[Codex HotSwitch] Antigravity Cockpit 插件 WS 热切完成: email={}, mode={}, execution_id={}, clients={}",
        account.email, response.effective_mode, response.execution_id, client_count
    ));

    Ok(CodexHotSwitchRuntimeResult {
        runtime: format_plugin_ws_runtime_name(client_count, &response),
        rate_limits: None,
    })
}

#[cfg(target_os = "windows")]
async fn warm_up_via_plugin_ws() -> Result<String, String> {
    let client_count = websocket::wait_for_connected_clients(PLUGIN_SWITCH_CONNECT_WAIT_MS).await?;
    Ok(format!(
        "antigravity-cockpit-plugin-ws:clients={}",
        client_count
    ))
}

#[cfg(target_os = "windows")]
fn validate_plugin_switch_response(
    response: &websocket::PluginSwitchAccountResponsePayload,
    account: &CodexAccount,
) -> Result<(), String> {
    if !response.success {
        let message = response
            .error_message
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("插件未返回错误详情");
        let code = response
            .error_code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown");
        return Err(format!(
            "Antigravity 插件无感切号失败: code={}, message={}",
            code, message
        ));
    }

    if !response.to_email.eq_ignore_ascii_case(&account.email) {
        return Err(format!(
            "Antigravity 插件回包目标账号不一致: expected={}, actual={}",
            account.email, response.to_email
        ));
    }

    if !response.effective_mode.eq_ignore_ascii_case("seamless") {
        return Err(format!(
            "Antigravity 插件未执行无感模式: expected=seamless, actual={}",
            response.effective_mode
        ));
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn format_plugin_ws_runtime_name(
    client_count: usize,
    response: &websocket::PluginSwitchAccountResponsePayload,
) -> String {
    format!(
        "antigravity-cockpit-plugin-ws:clients={},mode={},execution_id={}",
        client_count, response.effective_mode, response.execution_id
    )
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Deserialize)]
struct InspectorPort {
    pid: u32,
    port: u16,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct InspectorPortCandidate {
    pid: u32,
    port: u16,
    priority: u8,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct InspectorEndpoint {
    pid: u32,
    port: u16,
    ws_url: String,
    kind: InspectorTargetKind,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTargetKind {
    Node,
    CodexRenderer,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct InspectorWsTarget {
    ws_url: String,
    kind: InspectorTargetKind,
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
    for endpoint in endpoints.into_iter().take(MAX_INSPECTOR_ENDPOINTS_TO_TRY) {
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
async fn warm_up_via_antigravity_inspector() -> Result<String, String> {
    if let Some(endpoint) = cached_inspector_endpoint() {
        match warm_up_with_inspector_endpoint(&endpoint).await {
            Ok(runtime) => return Ok(runtime),
            Err(err) => logger::log_info(&format!(
                "[Codex HotSwitch] 缓存 Inspector 预热点失效: pid={}, port={}, error={}",
                endpoint.pid, endpoint.port, err
            )),
        }
    }

    let endpoints = discover_antigravity_inspector_endpoints().await?;
    if endpoints.is_empty() {
        return Err("未发现 Antigravity 扩展宿主 Inspector 端口".to_string());
    }

    let mut errors = Vec::new();
    for endpoint in endpoints.into_iter().take(MAX_INSPECTOR_ENDPOINTS_TO_TRY) {
        match warm_up_with_inspector_endpoint(&endpoint).await {
            Ok(runtime) => {
                remember_inspector_endpoint(&endpoint);
                return Ok(runtime);
            }
            Err(err) => errors.push(format!(
                "pid={},port={}: {}",
                endpoint.pid, endpoint.port, err
            )),
        }
    }

    Err(format!(
        "Antigravity Inspector 均未完成预热: {}",
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
        runtime: format_runtime_name(endpoint),
        rate_limits,
    }
}

#[cfg(target_os = "windows")]
fn format_runtime_name(endpoint: &InspectorEndpoint) -> String {
    format!(
        "antigravity-codex-extension-inspector:pid={},port={},kind={}",
        endpoint.pid,
        endpoint.port,
        endpoint.kind.label()
    )
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
    for port in ports.into_iter().take(MAX_INSPECTOR_PORTS_TO_SCAN) {
        match read_inspector_ws_urls(&client, port.port).await {
            Ok(ws_targets) => {
                for target in ws_targets {
                    endpoints.push(InspectorEndpoint {
                        pid: port.pid,
                        port: port.port,
                        ws_url: target.ws_url,
                        kind: target.kind,
                    });
                }
            }
            Err(err) => logger::log_info(&format!(
                "[Codex HotSwitch] 跳过不可用的 Inspector 端口: pid={}, port={}, error={}",
                port.pid, port.port, err
            )),
        }
    }
    endpoints.sort_by_key(|endpoint| (endpoint.kind.priority(), endpoint.port));
    Ok(endpoints)
}

#[cfg(target_os = "windows")]
fn discover_antigravity_inspector_ports() -> Result<Vec<InspectorPort>, String> {
    use std::collections::{HashMap, HashSet};
    use std::process::Command;
    use sysinfo::{ProcessRefreshKind, System, UpdateKind};

    let mut system = System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );

    let mut candidates = Vec::new();
    let mut antigravity_pids = HashSet::new();
    let mut node_service_pids = HashSet::new();

    for (pid, process) in system.processes() {
        if !is_antigravity_process(process) {
            continue;
        }

        let pid = pid.as_u32();
        let args: Vec<String> = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        let command_line = args.join(" ");
        let lower_command_line = command_line.to_ascii_lowercase();
        antigravity_pids.insert(pid);
        if lower_command_line.contains("node.mojom.nodeservice") {
            node_service_pids.insert(pid);
        }

        for port in collect_debug_flag_ports(&args, "--inspect-port") {
            push_port_candidate(&mut candidates, pid, port, 0);
        }
        for port in collect_debug_flag_ports(&args, "--inspect-extensions") {
            push_port_candidate(&mut candidates, pid, port, 0);
        }
        for port in collect_debug_flag_ports(&args, "--inspect-brk-extensions") {
            push_port_candidate(&mut candidates, pid, port, 0);
        }
        for port in collect_debug_flag_ports(&args, "--remote-debugging-port") {
            push_port_candidate(&mut candidates, pid, port, 5);
        }
    }

    // Extra pass: subprocesses of Antigravity (extension host / Node service).
    // Antigravity child processes (e.g. node.exe with --inspect) own the true
    // extension-host Inspector where require.cache is accessible.
    for (pid, process) in system.processes() {
        let p = pid.as_u32();
        if antigravity_pids.contains(&p) {
            continue;
        }
        let parent_pid = match process.parent() {
            Some(parent) => parent.as_u32(),
            None => continue,
        };
        if !antigravity_pids.contains(&parent_pid) && !node_service_pids.contains(&parent_pid) {
            continue;
        }
        let args = cmd_args(process);
        let lower_cmd = args.join(" ").to_ascii_lowercase();
        if lower_cmd.contains("node.mojom.nodeservice") {
            node_service_pids.insert(p);
        }
        push_ports_from_flag(&mut candidates, p, &args, "--inspect-port", 0);
        push_ports_from_flag(&mut candidates, p, &args, "--inspect-extensions", 0);
        push_ports_from_flag(&mut candidates, p, &args, "--inspect-brk-extensions", 0);
        push_ports_from_flag(&mut candidates, p, &args, "--inspect", 0);
        push_ports_from_flag(&mut candidates, p, &args, "--inspect-brk", 0);
        push_ports_from_flag(&mut candidates, p, &args, "--remote-debugging-port", 5);
        push_ports_from_flag(&mut candidates, p, &args, "--debug", 4);
    }

    if !antigravity_pids.is_empty() {
        let netstat_output = Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .map_err(|e| format!("执行 netstat 探测 Antigravity 监听端口失败: {}", e))?;
        if netstat_output.status.success() {
            let stdout = String::from_utf8_lossy(&netstat_output.stdout);
            for (pid, port) in parse_netstat_listeners(&stdout) {
                if node_service_pids.contains(&pid) {
                    push_port_candidate(&mut candidates, pid, port, 0);
                } else if antigravity_pids.contains(&pid) {
                    push_port_candidate(&mut candidates, pid, port, 5);
                }
            }
        } else {
            logger::log_warn(&format!(
                "[Codex HotSwitch] netstat 探测 Antigravity 监听端口失败: {}",
                String::from_utf8_lossy(&netstat_output.stderr).trim()
            ));
        }
    }

    candidates.sort_by_key(|item| (item.priority, item.pid, item.port));
    let mut unique = HashMap::<(u32, u16), u8>::new();
    let mut ports = Vec::new();
    for item in candidates {
        if unique
            .insert((item.pid, item.port), item.priority)
            .is_none()
        {
            ports.push(InspectorPort {
                pid: item.pid,
                port: item.port,
            });
        }
    }
    Ok(ports)
}

#[cfg(target_os = "windows")]
fn is_antigravity_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy().to_ascii_lowercase();
    let exe_path = process
        .exe()
        .and_then(|path| path.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name == "antigravity.exe" || exe_path.ends_with("\\antigravity.exe")
}

#[cfg(target_os = "windows")]
fn push_ports_from_flag(
    candidates: &mut Vec<InspectorPortCandidate>,
    pid: u32,
    args: &[String],
    flag: &str,
    priority: u8,
) {
    for port in collect_debug_flag_ports(args, flag) {
        push_port_candidate(candidates, pid, port, priority);
    }
}

#[cfg(target_os = "windows")]
fn cmd_args(process: &sysinfo::Process) -> Vec<String> {
    process
        .cmd()
        .iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect()
}

#[cfg(target_os = "windows")]
fn collect_debug_flag_ports(args: &[String], flag: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix(&format!("{}=", flag)) {
            if let Some(port) = parse_port_value(value) {
                ports.push(port);
            }
            continue;
        }
        if arg == flag {
            if let Some(next) = args
                .get(index + 1)
                .and_then(|value| parse_port_value(value))
            {
                ports.push(next);
            }
        }
    }
    ports
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn parse_port_value(value: &str) -> Option<u16> {
    let digits: String = value
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse::<u16>().ok().filter(|port| *port > 0)
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn push_port_candidate(
    candidates: &mut Vec<InspectorPortCandidate>,
    pid: u32,
    port: u16,
    priority: u8,
) {
    if port > 0 {
        candidates.push(InspectorPortCandidate {
            pid,
            port,
            priority,
        });
    }
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn parse_netstat_listeners(output: &str) -> Vec<(u32, u16)> {
    output.lines().filter_map(parse_netstat_line).collect()
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn parse_netstat_line(line: &str) -> Option<(u32, u16)> {
    let columns: Vec<&str> = line.split_whitespace().collect();
    if columns.len() < 5 || !columns[0].eq_ignore_ascii_case("TCP") {
        return None;
    }
    if !columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case("LISTENING"))
    {
        return None;
    }
    let pid = columns.last()?.parse::<u32>().ok()?;
    let port = parse_port_from_address(columns.get(1)?)?;
    Some((pid, port))
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn parse_port_from_address(address: &str) -> Option<u16> {
    address
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port > 0)
}

#[cfg(target_os = "windows")]
async fn read_inspector_ws_urls(
    client: &reqwest::Client,
    port: u16,
) -> Result<Vec<InspectorWsTarget>, String> {
    let mut targets = Vec::new();
    let json_paths = ["json/list", "json/version"];
    let hosts = ["127.0.0.1", "localhost"];

    for path in json_paths {
        for host in hosts {
            let url = format!("http://{}:{}/{}", host, port, path);
            let response = match client.get(&url).send().await {
                Ok(response) => response,
                Err(err) => {
                    logger::log_info(&format!(
                        "[Codex HotSwitch] Inspector {} 请求失败: {}",
                        url, err
                    ));
                    continue;
                }
            };
            if !response.status().is_success() {
                continue;
            }
            let value: Value = match response.json().await {
                Ok(value) => value,
                Err(err) => {
                    logger::log_info(&format!(
                        "[Codex HotSwitch] Inspector {} 响应解析失败: {}",
                        url, err
                    ));
                    continue;
                }
            };

            match path {
                "json/list" => {
                    let Some(entries) = value.as_array() else {
                        continue;
                    };
                    for entry in entries {
                        let Some(kind) = inspector_entry_kind(entry) else {
                            continue;
                        };
                        if let Some(ws_url) =
                            entry.get("webSocketDebuggerUrl").and_then(Value::as_str)
                        {
                            push_ws_target(&mut targets, ws_url, kind);
                        }
                    }
                }
                "json/version" => {
                    if !is_node_inspector_version(&value) {
                        continue;
                    }
                    if let Some(ws_url) = value.get("webSocketDebuggerUrl").and_then(Value::as_str)
                    {
                        push_ws_target(&mut targets, ws_url, InspectorTargetKind::Node);
                    }
                }
                _ => {}
            }
        }
        if !targets.is_empty() {
            break;
        }
    }

    if targets.is_empty() {
        Err(format!("端口 {} 未返回可用的 Inspector WebSocket", port))
    } else {
        targets.sort_by_key(|target| target.kind.priority());
        Ok(targets)
    }
}

#[cfg(target_os = "windows")]
fn push_ws_target(targets: &mut Vec<InspectorWsTarget>, ws_url: &str, kind: InspectorTargetKind) {
    let ws_url = normalize_inspector_ws_url(ws_url);
    if targets.iter().any(|target| target.ws_url == ws_url) {
        return;
    }
    targets.push(InspectorWsTarget { ws_url, kind });
}

#[cfg(target_os = "windows")]
fn inspector_entry_kind(entry: &Value) -> Option<InspectorTargetKind> {
    let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");
    if entry_type.eq_ignore_ascii_case("node") {
        return Some(InspectorTargetKind::Node);
    }

    let title = entry.get("title").and_then(Value::as_str).unwrap_or("");
    let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
    let description = entry
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let haystack = format!("{} {} {} {}", entry_type, title, url, description).to_ascii_lowercase();
    let looks_like_codex_extension = haystack.contains("openai.chatgpt")
        || haystack.contains("codexmcpconnection")
        || (haystack.contains("chatgpt") && haystack.contains("extension"))
        || (haystack.contains("codex") && haystack.contains("extension"));

    if looks_like_codex_extension {
        Some(InspectorTargetKind::CodexRenderer)
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn is_node_inspector_version(value: &Value) -> bool {
    value
        .get("Browser")
        .and_then(Value::as_str)
        .map(|browser| browser.to_ascii_lowercase().contains("node"))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn normalize_inspector_ws_url(ws_url: &str) -> String {
    ws_url
        .replace("ws://[::1]:", "ws://127.0.0.1:")
        .replace("wss://[::1]:", "wss://127.0.0.1:")
        .replace("ws://localhost:", "ws://127.0.0.1:")
        .replace("wss://localhost:", "wss://127.0.0.1:")
}

#[cfg(target_os = "windows")]
impl InspectorTargetKind {
    fn priority(self) -> u8 {
        match self {
            InspectorTargetKind::Node => 0,
            InspectorTargetKind::CodexRenderer => 10,
        }
    }

    fn label(self) -> &'static str {
        match self {
            InspectorTargetKind::Node => "node",
            InspectorTargetKind::CodexRenderer => "codex-renderer",
        }
    }
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
async fn warm_up_with_inspector_endpoint(endpoint: &InspectorEndpoint) -> Result<String, String> {
    let mut client = CdpClient::connect(&endpoint.ws_url).await?;
    let _ = client.find_codex_mcp_instance().await?;
    logger::log_info(&format!(
        "[Codex HotSwitch] Antigravity 插件运行时预热完成: pid={}, port={}",
        endpoint.pid, endpoint.port
    ));
    Ok(format_runtime_name(endpoint))
}

#[cfg(target_os = "windows")]
impl CdpClient {
    async fn connect(ws_url: &str) -> Result<Self, String> {
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(INSPECTOR_CONNECT_TIMEOUT_SECS),
            connect_async(ws_url),
        )
        .await
        .map_err(|_| {
            format!(
                "连接 Antigravity Inspector WebSocket 超时: timeout={}s",
                INSPECTOR_CONNECT_TIMEOUT_SECS
            )
        })?
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
            .map_err(|e| {
                format!(
                    "Inspector WebSocket 写入失败: method={}, id={}, error={}",
                    method, id, e
                )
            })?;

        let wait = async {
            loop {
                let Some(message) = self.socket.next().await else {
                    return Err(format!(
                        "Inspector WebSocket 已断开: method={}, id={}",
                        method, id
                    ));
                };
                let message = message.map_err(|e| {
                    format!(
                        "Inspector WebSocket 读取失败: method={}, id={}, error={}",
                        method, id, e
                    )
                })?;
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Message::Close(_) => {
                        return Err(format!(
                            "Inspector WebSocket 已关闭: method={}, id={}",
                            method, id
                        ));
                    }
                    _ => continue,
                };
                let value: Value = serde_json::from_str(&text).map_err(|e| {
                    format!(
                        "Inspector 响应解析失败: method={}, id={}, error={}",
                        method, id, e
                    )
                })?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(format!(
                        "Inspector 调用失败: method={}, id={}, error={}",
                        method, id, error
                    ));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        };

        tokio::time::timeout(Duration::from_secs(INSPECTOR_REQUEST_TIMEOUT_SECS), wait)
            .await
            .map_err(|_| {
                format!(
                    "Inspector WebSocket 超时: method={}, id={}, timeout={}s",
                    method, id, INSPECTOR_REQUEST_TIMEOUT_SECS
                )
            })?
    }

    async fn evaluate_object(&mut self, label: &str, expression: &str) -> Result<String, String> {
        let value = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": false,
                    "replMode": true
                }),
            )
            .await
            .map_err(|err| format!("evaluate_object({}) 失败: {}", label, err))?;
        if let Some(exception) = value.get("exceptionDetails") {
            return Err(format!(
                "evaluate_object({}) 异常: {}",
                label,
                format_cdp_exception(exception)
            ));
        }
        object_id_at(&value, &["result"])
            .map(ToString::to_string)
            .ok_or_else(|| format!("evaluate_object({}) 未返回 objectId", label))
    }

    async fn get_properties(
        &mut self,
        label: &str,
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
        .map_err(|err| format!("get_properties({}) 失败: {}", label, err))
    }

    async fn find_codex_mcp_instance(&mut self) -> Result<String, String> {
        let direct_scan_error = match self
            .evaluate_object("direct req.cache scan", CODEX_INSTANCE_EXPRESSION)
            .await
        {
            Ok(instance_id) => {
                logger::log_info(
                    "[Codex HotSwitch] 直接扫描 req.cache 命中 CodexMcpConnection 实例",
                );
                return Ok(instance_id);
            }
            Err(err) => {
                logger::log_info(&format!(
                    "[Codex HotSwitch] 直接扫描失败，回退 heap arrays: {}",
                    err
                ));
                err
            }
        };

        let array_scan_error = match self.find_codex_mcp_instance_in_arrays().await {
            Ok(Some(instance_id)) => {
                logger::log_info("[Codex HotSwitch] heap arrays 扫描命中 CodexMcpConnection 实例");
                return Ok(instance_id);
            }
            Ok(None) => {
                let err = "heap arrays 未找到 CodexMcpConnection 实例".to_string();
                logger::log_info(&format!(
                    "[Codex HotSwitch] heap arrays 扫描失败，回退 activate [[Scopes]]: {}",
                    err
                ));
                err
            }
            Err(err) => {
                logger::log_info(&format!(
                    "[Codex HotSwitch] heap arrays 扫描异常，回退 activate [[Scopes]]: {}",
                    err
                ));
                err
            }
        };
        let combined_scan_error = format!(
            "{}; heap arrays 扫描失败: {}",
            direct_scan_error, array_scan_error
        );

        let activate_id = self
            .evaluate_object("activate export lookup", ACTIVATE_FUNCTION_EXPRESSION)
            .await
            .map_err(|err| format_lookup_error("activate 失败", &combined_scan_error, err))?;
        let activate_props = self
            .get_properties("activate function", &activate_id, false)
            .await
            .map_err(|err| format_lookup_error("activate 失败", &combined_scan_error, err))?;
        let scopes_id = internal_object_id(&activate_props, "[[Scopes]]")
            .ok_or_else(|| {
                format_lookup_error(
                    "activate 失败",
                    &combined_scan_error,
                    "未找到 Codex 扩展 activate [[Scopes]]",
                )
            })?
            .to_string();
        let scopes = self
            .get_properties("activate [[Scopes]]", &scopes_id, true)
            .await
            .map_err(|err| format_lookup_error("activate 失败", &combined_scan_error, err))?;
        let closure_scope_id = property_object_id(&scopes, "0")
            .ok_or_else(|| {
                format_lookup_error(
                    "activate 失败",
                    &combined_scan_error,
                    "未找到 Codex 扩展闭包 scope",
                )
            })?
            .to_string();
        let closure_props = self
            .get_properties("activate closure scope", &closure_scope_id, true)
            .await
            .map_err(|err| format_lookup_error("activate 失败", &combined_scan_error, err))?;
        let class_ids = codex_mcp_class_candidates(&closure_props);
        if class_ids.is_empty() {
            return Err(format_lookup_error(
                "activate 失败",
                &combined_scan_error,
                "未找到 CodexMcpConnection 候选类",
            ));
        }

        let mut errors = Vec::new();
        let candidate_count = class_ids.len();
        for class_id in class_ids.into_iter().take(80) {
            match self.find_codex_mcp_instance_for_class(&class_id).await {
                Ok(Some(instance_id)) => {
                    logger::log_info(
                        "[Codex HotSwitch] activate [[Scopes]] + queryObjects 命中 CodexMcpConnection 实例",
                    );
                    return Ok(instance_id);
                }
                Ok(None) => {}
                Err(err) => errors.push(err),
            }
        }

        let detail = if errors.is_empty() {
            format!(
                "当前 Antigravity 扩展宿主中未找到 CodexMcpConnection 实例，candidate_count={}",
                candidate_count
            )
        } else {
            format!(
                "当前 Antigravity 扩展宿主中未找到 CodexMcpConnection 实例，candidate_count={}, errors={}",
                candidate_count,
                errors.into_iter().take(3).collect::<Vec<_>>().join(" | ")
            )
        };
        Err(format_lookup_error(
            "queryObjects 失败",
            &combined_scan_error,
            detail,
        ))
    }

    async fn find_codex_mcp_instance_in_arrays(&mut self) -> Result<Option<String>, String> {
        let array_id = self
            .evaluate_object("Array constructor", "Array")
            .await
            .map_err(|err| format!("获取 Array 构造函数失败: {}", err))?;
        let array_props = self
            .get_properties("Array constructor", &array_id, false)
            .await
            .map_err(|err| format!("读取 Array.prototype 失败: {}", err))?;
        let prototype_id = property_object_id(&array_props, "prototype")
            .ok_or_else(|| "Array.prototype 缺少 objectId".to_string())?;
        let queried = self
            .call(
                "Runtime.queryObjects",
                json!({
                    "prototypeObjectId": prototype_id
                }),
            )
            .await
            .map_err(|err| format!("queryObjects(Array.prototype) 失败: {}", err))?;
        let arrays_id = object_id_at(&queried, &["objects"])
            .ok_or_else(|| "Inspector queryObjects(Array) 未返回对象数组".to_string())?
            .to_string();
        let response = self
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": arrays_id,
                    "functionDeclaration": FIND_CODEX_INSTANCE_IN_ARRAYS_FUNCTION,
                    "returnByValue": false
                }),
            )
            .await
            .map_err(|err| format!("扫描 heap arrays 失败: {}", err))?;
        if let Some(exception) = response.get("exceptionDetails") {
            return Err(format!(
                "扫描 heap arrays 异常: {}",
                format_cdp_exception(exception)
            ));
        }
        let Some(result) = response.get("result") else {
            return Ok(None);
        };
        if result.get("subtype").and_then(Value::as_str) == Some("null")
            || result.get("type").and_then(Value::as_str) == Some("undefined")
        {
            return Ok(None);
        }
        Ok(object_id_at(&response, &["result"]).map(ToString::to_string))
    }

    async fn find_codex_mcp_instance_for_class(
        &mut self,
        class_id: &str,
    ) -> Result<Option<String>, String> {
        let class_props = self
            .get_properties("CodexMcpConnection class candidate", class_id, false)
            .await?;
        let Some(prototype_id) = property_object_id(&class_props, "prototype") else {
            return Ok(None);
        };
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
        let objects = self
            .get_properties("queryObjects result", &objects_id, true)
            .await?;

        for object_id in indexed_object_ids(&objects).into_iter().take(20) {
            if self.is_codex_mcp_instance(&object_id).await? {
                return Ok(Some(object_id));
            }
        }
        Ok(None)
    }

    async fn is_codex_mcp_instance(&mut self, object_id: &str) -> Result<bool, String> {
        let response = self
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function() { return Boolean(this && typeof this.sendRequest === 'function' && this.providers && typeof this.providers.get === 'function'); }",
                    "returnByValue": true
                }),
            )
            .await?;
        Ok(response
            .get("result")
            .and_then(|item| item.get("value"))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
}

#[cfg(target_os = "windows")]
fn format_lookup_error(
    stage: &str,
    direct_scan_error: &str,
    detail: impl std::fmt::Display,
) -> String {
    format!("直接扫描失败: {}; {}: {}", direct_scan_error, stage, detail)
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
fn indexed_object_ids(properties: &Value) -> Vec<String> {
    let mut entries = properties
        .get("result")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    entries.sort_by_key(|item| {
        item.get("name")
            .and_then(Value::as_str)
            .and_then(|name| name.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });

    entries
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?;
            name.parse::<usize>().ok()?;
            item.get("value")?
                .get("objectId")?
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn codex_mcp_class_candidates(properties: &Value) -> Vec<String> {
    properties
        .get("result")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let value = item.get("value")?;
                    if value.get("type").and_then(Value::as_str) != Some("function") {
                        return None;
                    }
                    value.get("objectId")?.as_str().map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
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
const CODEX_INSTANCE_EXPRESSION: &str = r#"(() => {
  const findModuleBuiltin = () => {
    const p = globalThis.vscode?.process ?? globalThis.process;
    // Strategy 1: Electron getBuiltinModule (works in extension host context)
    if (p?.getBuiltinModule) {
      const m = p.getBuiltinModule('module');
      if (m?.createRequire) return { module: m, source: 'getBuiltinModule' };
    }
    // Strategy 2: require in global scope (Node.js / Electron context)
    try {
      const r = typeof require !== 'undefined' ? require : (globalThis.require ?? false);
      if (r && typeof r.resolve === 'function' && r.cache) {
        return { module: r, source: 'require' };
      }
    } catch {}
    // Strategy 3: process.mainModule.require (deprecated Electron, sometimes works)
    try {
      if (p?.mainModule?.require && typeof p.mainModule.require.resolve === 'function') {
        return { module: p.mainModule.require, source: 'mainModule' };
      }
    } catch {}
    return null;
  };

  const builtin = findModuleBuiltin();
  if (builtin) {
    // Build require.cache walk from the acquired module API
    const createRequireFromModule = (moduleApi) => {
      if (moduleApi?.cache) return moduleApi;
      const anchors = [
        'D:/Antigravity/resources/app/out/vs/workbench/api/node/extensionHostProcess.js',
        'D:/Antigravity/resources/app/out/main.js',
        'file:///d:/Antigravity/resources/app/out/vs/workbench/api/node/extensionHostProcess.js'
      ];
      for (const anchor of anchors) {
        try {
          const req = moduleApi?.createRequire?.(anchor);
          if (req?.cache) return req;
        } catch {}
      }
      return null;
    };
    const req = builtin.source === 'require' ? builtin.module : createRequireFromModule(builtin.module);
    if (!req?.cache) throw new Error('Node require cache is unavailable');
    const cache = req.cache || {};
    const modulePriority = (key) => {
      const lower = String(key).toLowerCase();
      if (lower.includes('openai.chatgpt-') && lower.endsWith('extension.js')) return 3;
      if (lower.includes('chatgpt') || lower.includes('codex')) return 2;
      return lower.endsWith('extension.js') ? 1 : 0;
    };
    const moduleEntries = Object.entries(cache).filter(([, mod]) => Boolean(mod));
    const modules = moduleEntries
      .sort(([left], [right]) => modulePriority(right) - modulePriority(left))
      .map(([, mod]) => mod);
    if (modules.length > 0) {
      const visitedValues = new WeakSet();
      const visitedModules = new Set();
      const valuesOf = (value) => { try { return Object.values(value); } catch { return []; } };
      const isCandidate = (value) => {
        if (!value || (typeof value !== 'object' && typeof value !== 'function')) return false;
        if (typeof value.sendRequest !== 'function') return false;
        const providers = value.providers;
        if (!providers || typeof providers.get !== 'function') return false;
        return value.initialized === true;
      };
      const walkValue = (value, depth) => {
        if (!value || (typeof value !== 'object' && typeof value !== 'function')) return null;
        if (visitedValues.has(value)) return null;
        visitedValues.add(value);
        if (isCandidate(value)) return value;
        if (depth <= 0) return null;
        for (const child of valuesOf(value)) { const f = walkValue(child, depth - 1); if (f) return f; }
        return null;
      };
      const walkModule = (mod, depth) => {
        if (!mod || visitedModules.has(mod)) return null;
        visitedModules.add(mod);
        for (const exported of [mod.exports, mod.exports?.default]) { const f = walkValue(exported, depth); if (f) return f; }
        if (depth <= 0) return null;
        for (const child of mod.children || []) { const f = walkModule(child, depth - 1); if (f) return f; }
        return null;
      };
      for (const mod of modules) { const f = walkModule(mod, 4); if (f) return f; }
    }
  }

  // Renderer-context fallback: walk globalThis and vscode API to find CodexMcpConnection
  const walkGlobal = (root, maxDepth) => {
    const visited = new WeakSet();
    const deeper = (v, d) => {
      if (!v || (typeof v !== 'object' && typeof v !== 'function')) return null;
      if (visited.has(v)) return null;
      visited.add(v);
      if (typeof v.sendRequest === 'function' && v.providers && typeof v.providers.get === 'function' && v.initialized === true) return v;
      if (d <= 0) return null;
      try { for (const child of Object.values(v)) { const f = deeper(child, d - 1); if (f) return f; } } catch {}
      return null;
    };
    return deeper(root, maxDepth);
  };
  const fromGlobal = walkGlobal(globalThis, 5);
  if (fromGlobal) return fromGlobal;
  // Walk vscode extensions if available
  const vscodeApi = globalThis.vscode;
  if (vscodeApi?.extensions) {
    try {
      const exts = vscodeApi.extensions.all || [];
      for (const ext of exts) { const f = walkGlobal(ext, 4); if (f) return f; }
    } catch {}
  }
  // Walk chrome/webview known extension iframes
  try {
    if (globalThis.document?.querySelectorAll) {
      for (const frame of globalThis.document.querySelectorAll('webview, iframe')) { const f = walkGlobal(frame.contentWindow, 3); if (f) return f; }
    }
  } catch {}

  throw new Error(
    'CodexMcpConnection instance is not loaded' +
    (builtin ? '; require.cache walk exhausted' : '; Electron built-in module API is unavailable')
  );
})()"#;

#[cfg(target_os = "windows")]
const ACTIVATE_FUNCTION_EXPRESSION: &str = r#"(() => {
  const findModuleBuiltin = () => {
    const p = globalThis.vscode?.process ?? globalThis.process;
    if (p?.getBuiltinModule) {
      const m = p.getBuiltinModule('module');
      if (m?.createRequire) return m;
    }
    try {
      const r = typeof require !== 'undefined' ? require : (globalThis.require ?? false);
      if (r && typeof r.resolve === 'function' && r.cache) return r;
    } catch {}
    try {
      if (p?.mainModule?.require && typeof p.mainModule.require.resolve === 'function') return p.mainModule.require;
    } catch {}
    return null;
  };

  const mod = findModuleBuiltin();
  if (!mod) throw new Error('Electron built-in module API is unavailable');

  const createRequireFromModule = (moduleApi) => {
    if (moduleApi?.cache) return moduleApi;
    const anchors = [
      'D:/Antigravity/resources/app/out/vs/workbench/api/node/extensionHostProcess.js',
      'D:/Antigravity/resources/app/out/main.js',
      'file:///d:/Antigravity/resources/app/out/vs/workbench/api/node/extensionHostProcess.js'
    ];
    for (const anchor of anchors) {
      try {
        const req = moduleApi?.createRequire?.(anchor);
        if (req?.cache) return req;
      } catch {}
    }
    return null;
  };
  const req = typeof mod.cache !== 'undefined' ? mod : createRequireFromModule(mod);
  if (!req?.cache) throw new Error('Node require cache is unavailable');
  const cache = req.cache || {};
  const modulePriority = (key) => {
    const lower = String(key).toLowerCase();
    if (lower.includes('openai.chatgpt-') && lower.endsWith('extension.js')) return 3;
    if (lower.includes('chatgpt') || lower.includes('codex')) return 2;
    return lower.endsWith('extension.js') ? 1 : 0;
  };
  const moduleEntries = Object.entries(cache)
    .filter(([, mod]) => Boolean(mod))
    .sort(([left], [right]) => modulePriority(right) - modulePriority(left));
  if (moduleEntries.length === 0) throw new Error('Node module cache is empty');
  const exportCandidates = moduleEntries.flatMap(([key, mod]) => {
    const priority = modulePriority(key);
    const exports = cache[key]?.exports;
    const candidates = [exports?.activate, exports?.default?.activate];
    if (priority > 0) candidates.push(exports?.default, exports);
    return candidates;
  });
  const activate = exportCandidates.find((candidate) => typeof candidate === 'function');
  if (!activate) throw new Error(
    'Codex extension activate export is unavailable; moduleCacheSize=' + moduleEntries.length +
    '; prioritizedModules=' + moduleEntries.filter(([key]) => modulePriority(key) > 0).length
  );
  return activate;
})()"#;

#[cfg(target_os = "windows")]
const FIND_CODEX_INSTANCE_IN_ARRAYS_FUNCTION: &str = r#"function() {
  const isCandidate = (value) => {
    if (!value || (typeof value !== 'object' && typeof value !== 'function')) return false;
    if (typeof value.sendRequest !== 'function') return false;
    const providers = value.providers;
    if (!providers || typeof providers.get !== 'function') return false;
    try {
      if (typeof providers.has === 'function' && !providers.has('auth')) return false;
    } catch {}
    return true;
  };

  const scanValue = (value, depth, seen) => {
    if (isCandidate(value)) return value;
    if (!value || depth <= 0 || (typeof value !== 'object' && typeof value !== 'function')) return null;
    if (seen.has(value)) return null;
    seen.add(value);
    try {
      const directNames = ['_value', 'value', 'item', 'connection', 'client', 'disposable'];
      for (const name of directNames) {
        const found = scanValue(value[name], depth - 1, seen);
        if (found) return found;
      }
    } catch {}
    return null;
  };

  const seen = new WeakSet();
  let inspectedArrays = 0;
  let inspectedValues = 0;
  for (const array of this) {
    inspectedArrays += 1;
    if (!Array.isArray(array) || array.length === 0 || array.length > 5000) continue;
    for (let index = 0; index < array.length; index += 1) {
      inspectedValues += 1;
      const found = scanValue(array[index], 2, seen);
      if (found) return found;
      if (inspectedValues > 250000) return null;
    }
  }
  return null;
}"#;

#[cfg(target_os = "windows")]
const HOT_SWITCH_FUNCTION: &str = r#"async function(payload) {
  if (!this.initialized) {
    throw new Error('Codex app-server is not initialized');
  }

  const originalProvider = this.providers.get('auth');
  if (!originalProvider) {
    throw new Error('Original auth provider not found');
  }

  const originalOnResult = originalProvider.onResult;
  const originalOnNotification = originalProvider.onNotification;

  const pending = new Map();
  const notifications = [];
  let nextId = Math.floor(Math.random() * 1000000) + 1;

  try {
    // 劫持原始 provider
    originalProvider.onResult = (message) => {
      const key = String(message.id);
      const waiter = pending.get(key);
      if (waiter) {
        pending.delete(key);
        if (message.error) {
          const errorText = typeof message.error === 'string'
            ? message.error
            : JSON.stringify(message.error);
          waiter.reject(new Error(errorText));
        } else {
          waiter.resolve(message.result ?? null);
        }
      } else {
        if (typeof originalOnResult === 'function') {
          try { originalOnResult(message); } catch {}
        }
      }
    };

    originalProvider.onNotification = (message) => {
      notifications.push({ method: message.method, params: message.params });
      if (typeof originalOnNotification === 'function') {
        try { originalOnNotification(message); } catch {}
      }
    };

    const request = (method, params, timeoutMs = 8000) => new Promise((resolve, reject) => {
      const id = nextId++;
      const idStr = String(id);
      const timer = setTimeout(() => {
        pending.delete(idStr);
        reject(new Error(`${method} timeout`));
      }, timeoutMs);
      pending.set(idStr, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
      this.sendRequest('auth', id, method, params);
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
        if (Date.now() - startedAt > 2000) {
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

  } finally {
    // 恢复原始提供者
    originalProvider.onResult = originalOnResult;
    originalProvider.onNotification = originalOnNotification;
  }
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

#[cfg(target_os = "windows")]
pub fn try_inject_shortcut_debugging_port() -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let script = r#"
try {
    $shell = New-Object -ComObject WScript.Shell
    $paths = @("$env:USERPROFILE\Desktop", "C:\Users\Public\Desktop")
    $injected = 0
    foreach ($path in $paths) {
        if (Test-Path $path) {
            $shortcuts = Get-ChildItem -Path $path -Filter "*.lnk"
            foreach ($s in $shortcuts) {
                try {
                    $lnk = $shell.CreateShortcut($s.FullName)
                    if ($lnk.TargetPath -like "*Antigravity.exe*") {
                        $changed = $false
                        if ($lnk.Arguments -notlike "*--inspect-extensions*") {
                            $lnk.Arguments = ($lnk.Arguments + " --inspect-extensions=9333").Trim()
                            $changed = $true
                        }
                        if ($lnk.Arguments -notlike "*--remote-debugging-port*") {
                            $lnk.Arguments = ($lnk.Arguments + " --remote-debugging-port=9000").Trim()
                            $changed = $true
                        }
                        if ($changed) {
                            $lnk.Save()
                            $injected++
                        }
                    }
                } catch {}
            }
        }
    }
    Write-Output $injected
} catch {
    Write-Output 0
}
"#;

    let command_text = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; {}",
        script
    );

    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .arg("-NoProfile")
        .arg("-Command")
        .arg(&command_text)
        .output();

    match output {
        Ok(out) => {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let count = res.parse::<i32>().unwrap_or(0);
            logger::log_info(&format!(
                "[Shortcut Injector] 快捷方式注入调试参数运行完毕: res={}, count={}, err={}",
                res,
                count,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            count > 0
        }
        Err(e) => {
            logger::log_warn(&format!(
                "[Shortcut Injector] 执行 PowerShell 注入快捷方式失败: {}",
                e
            ));
            false
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn try_inject_shortcut_debugging_port() -> bool {
    false
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    fn codex_account(email: &str) -> CodexAccount {
        CodexAccount::new(
            "codex-test".to_string(),
            email.to_string(),
            crate::models::codex::CodexTokens {
                id_token: "id-token".to_string(),
                access_token: "access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
            },
        )
    }

    fn plugin_switch_response(
        success: bool,
        to_email: &str,
        effective_mode: &str,
    ) -> websocket::PluginSwitchAccountResponsePayload {
        websocket::PluginSwitchAccountResponsePayload {
            execution_id: "switch-test".to_string(),
            request_id: Some("request-test".to_string()),
            success,
            effective_mode: effective_mode.to_string(),
            from_email: Some("from@example.com".to_string()),
            to_email: to_email.to_string(),
            duration_ms: 42,
            error_code: None,
            error_message: None,
            finished_at: "2026-05-29T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn accepts_successful_seamless_plugin_response() {
        let account = codex_account("target@example.com");
        let response = plugin_switch_response(true, "Target@Example.com", "seamless");
        assert!(validate_plugin_switch_response(&response, &account).is_ok());
    }

    #[test]
    fn rejects_failed_or_non_seamless_plugin_response() {
        let account = codex_account("target@example.com");

        let mut failed = plugin_switch_response(false, "target@example.com", "seamless");
        failed.error_code = Some("switch_failed".to_string());
        failed.error_message = Some("failed".to_string());
        assert!(validate_plugin_switch_response(&failed, &account)
            .unwrap_err()
            .contains("switch_failed"));

        let wrong_mode = plugin_switch_response(true, "target@example.com", "default");
        assert!(validate_plugin_switch_response(&wrong_mode, &account)
            .unwrap_err()
            .contains("未执行无感模式"));

        let wrong_email = plugin_switch_response(true, "other@example.com", "seamless");
        assert!(validate_plugin_switch_response(&wrong_email, &account)
            .unwrap_err()
            .contains("目标账号不一致"));
    }

    #[test]
    fn parses_debug_ports_from_flags_and_netstat() {
        assert_eq!(parse_port_value("9229"), Some(9229));
        assert_eq!(parse_port_value("127.0.0.1:9333"), Some(9333));
        assert_eq!(
            parse_netstat_line("  TCP    127.0.0.1:9333    0.0.0.0:0    LISTENING    4242"),
            Some((4242, 9333))
        );
        assert_eq!(
            parse_netstat_line("  TCP    [::]:9444    [::]:0    LISTENING    5252"),
            Some((5252, 9444))
        );
    }

    #[test]
    fn filters_inspector_targets_to_node_or_codex_extension() {
        let node_entry = json!({
            "type": "node",
            "title": "Antigravity Extension Host",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9333/abc"
        });
        assert_eq!(
            inspector_entry_kind(&node_entry),
            Some(InspectorTargetKind::Node)
        );

        let generic_extension_host_page = json!({
            "type": "page",
            "title": "Extension Host",
            "url": "devtools://devtools/bundled/js_app.html"
        });
        assert_eq!(inspector_entry_kind(&generic_extension_host_page), None);

        let codex_extension_page = json!({
            "type": "page",
            "title": "openai.chatgpt extension",
            "url": "antigravity://extension/openai.chatgpt"
        });
        assert_eq!(
            inspector_entry_kind(&codex_extension_page),
            Some(InspectorTargetKind::CodexRenderer)
        );
    }
}
