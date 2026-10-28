use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

const SYSTEM_PROMPT: &str = r#"You are a local desktop agent. Reply ONLY with a JSON object: {"message":"brief explanation","actions":[]}.
Use at most 8 actions. Available actions and exact JSON forms:
{"type":"list_files","path":"."}
{"type":"read_file","path":"relative/path.txt"}
{"type":"write_file","path":"relative/path.txt","content":"complete new file content"}
{"type":"create_dir","path":"relative/directory"}
{"type":"delete_file","path":"relative/file.txt"}
{"type":"run_command","command":"command to run in the chosen workspace"}
{"type":"weather","city":"London"}
{"type":"cat_fact"}
{"type":"github_user","username":"octocat"}
Use only relative paths under the selected workspace. A request to create test.txt means a FILE: use write_file directly, never create_dir for a filename. Use create_dir only when the user explicitly wants a folder. For weather, cat facts, and GitHub profile questions, use the matching API action rather than answering from memory. If a request needs a tool result before another action (for example weather before writing a file), call the first tool and then use its actual result in the next action plan. Do not stop after fetching data when the user asked to save it to a file. Never claim an action already happened. Read files before proposing edits if their contents are unknown. Do not include markdown fences. If the user only asks a question, use an empty actions array."#;

struct AppState {
    client: reqwest::Client,
    workspace: Mutex<Option<PathBuf>>,
    plans: Mutex<HashMap<String, StoredPlan>>,
}

#[derive(Serialize)]
struct OllamaStatus {
    running: bool,
    models: Vec<String>,
    error: String,
}

#[derive(Deserialize)]
struct OllamaTags {
    models: Vec<ModelTag>,
}
#[derive(Deserialize)]
struct ModelTag {
    name: String,
}

#[derive(Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AgentReply {
    message: String,
    actions: Vec<Action>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    ListFiles { path: String },
    ReadFile { path: String },
    WriteFile { path: String, content: String },
    CreateDir { path: String },
    DeleteFile { path: String },
    RunCommand { command: String },
    Weather { city: String },
    CatFact,
    GithubUser { username: String },
}

impl Action {
    fn label(&self) -> &'static str {
        match self {
            Self::ListFiles { .. } => "List files",
            Self::ReadFile { .. } => "Read file",
            Self::WriteFile { .. } => "Write file",
            Self::CreateDir { .. } => "Create directory",
            Self::DeleteFile { .. } => "Delete file",
            Self::RunCommand { .. } => "Run command",
            Self::Weather { .. } => "Weather",
            Self::CatFact => "Cat fact",
            Self::GithubUser { .. } => "GitHub profile",
        }
    }
    fn detail(&self) -> String {
        match self {
            Self::ListFiles { path }
            | Self::ReadFile { path }
            | Self::CreateDir { path }
            | Self::DeleteFile { path } => path.clone(),
            Self::WriteFile { path, content } => format!("{} ({} bytes)", path, content.len()),
            Self::RunCommand { command } => command.clone(),
            Self::Weather { city } => city.clone(),
            Self::CatFact => "Random fact".into(),
            Self::GithubUser { username } => username.clone(),
        }
    }
    fn audit_detail(&self) -> String {
        match self {
            Self::WriteFile { path, content } => format!("{} ({} bytes)", path, content.len()),
            _ => self.detail(),
        }
    }
    fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Self::WriteFile { .. }
                | Self::CreateDir { .. }
                | Self::DeleteFile { .. }
                | Self::RunCommand { .. }
        )
    }
}

#[derive(Serialize)]
struct ActionView {
    label: String,
    detail: String,
    needs_confirmation: bool,
}
#[derive(Serialize)]
struct PlanView {
    id: String,
    message: String,
    actions: Vec<ActionView>,
    needs_confirmation: bool,
}
struct StoredPlan {
    workspace: PathBuf,
    actions: Vec<Action>,
}

#[derive(Serialize)]
struct ActionResult {
    action: String,
    detail: String,
    status: String,
    output: String,
}

#[derive(Serialize, Deserialize)]
struct AuditEntry {
    time: u64,
    action: String,
    detail: String,
    status: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn log_action(app: &AppHandle, action: &str, detail: &str, status: &str) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("actions.jsonl"))
    else {
        return;
    };
    let entry = AuditEntry {
        time: now(),
        action: action.into(),
        detail: detail.into(),
        status: status.into(),
    };
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = writeln!(file, "{line}");
    }
}

fn workspace(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .workspace
        .lock()
        .map_err(|_| "Workspace is unavailable".to_string())?
        .clone()
        .ok_or_else(|| "Choose a workspace first".into())
}

fn relative_path(root: &Path, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.len() > 500 || value.contains(':') {
        return Err("Invalid path".into());
    }
    let input = Path::new(value);
    if input
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err("Path must stay inside the selected workspace".into());
    }
    let mut resolved = root.to_path_buf();
    for part in input.components() {
        if let Component::Normal(name) = part {
            resolved.push(name);
            if let Ok(meta) = fs::symlink_metadata(&resolved) {
                if meta.file_type().is_symlink() {
                    return Err("Symlink paths are not allowed".into());
                }
            }
        }
    }
    Ok(resolved)
}

fn validate_action(action: &Action, root: &Path) -> Result<(), String> {
    match action {
        Action::ListFiles { path }
        | Action::ReadFile { path }
        | Action::CreateDir { path }
        | Action::DeleteFile { path }
        | Action::WriteFile { path, .. } => {
            let target = relative_path(root, path)?;
            if matches!(
                action,
                Action::WriteFile { .. } | Action::DeleteFile { .. } | Action::CreateDir { .. }
            ) && target == root
            {
                return Err("Cannot modify workspace root".into());
            }
            if matches!(action, Action::CreateDir { .. })
                && Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "txt" | "md" | "json" | "csv"
                        )
                    })
            {
                return Err(format!(
                    "{path} looks like a filename. Use write_file to create a file."
                ));
            }
        }
        Action::RunCommand { command } => {
            if command.trim().is_empty() || command.len() > 2000 {
                return Err("Command is empty or too long".into());
            }
        }
        Action::Weather { city } => {
            if city.trim().len() < 2 || city.len() > 100 {
                return Err("Invalid city".into());
            }
        }
        Action::GithubUser { username } => {
            if username.is_empty()
                || username.len() > 39
                || !username
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return Err("Invalid GitHub username".into());
            }
        }
        Action::CatFact => {}
    }
    if let Action::WriteFile { content, .. } = action {
        if content.len() > 100_000 {
            return Err("File content exceeds 100 KB".into());
        }
    }
    Ok(())
}

#[tauri::command]
async fn ollama_status(state: State<'_, AppState>) -> Result<OllamaStatus, String> {
    Ok(
        match state
            .client
            .get("http://127.0.0.1:11434/api/tags")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                match response.json::<OllamaTags>().await {
                    Ok(tags) => OllamaStatus {
                        running: true,
                        models: tags.models.into_iter().map(|m| m.name).collect(),
                        error: String::new(),
                    },
                    Err(e) => OllamaStatus {
                        running: false,
                        models: vec![],
                        error: format!("Ollama returned invalid model data: {e}"),
                    },
                }
            }
            Ok(response) => OllamaStatus {
                running: false,
                models: vec![],
                error: format!("Ollama returned HTTP {}", response.status()),
            },
            Err(_) => OllamaStatus {
                running: false,
                models: vec![],
                error: "Could not reach Ollama at localhost:11434. Start Ollama and try again."
                    .into(),
            },
        },
    )
}

#[tauri::command]
fn set_workspace(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let root = fs::canonicalize(&path).map_err(|e| format!("Cannot open folder: {e}"))?;
    if !root.is_dir() {
        return Err("Choose a directory".into());
    }
    *state
        .workspace
        .lock()
        .map_err(|_| "Workspace is unavailable")? = Some(root.clone());
    state
        .plans
        .lock()
        .map_err(|_| "Plans are unavailable")?
        .clear();
    let display = root.to_string_lossy().into_owned();
    log_action(&app, "Workspace", &display, "selected");
    Ok(display)
}

#[tauri::command]
async fn ask_agent(
    prompt: String,
    model: String,
    history: Vec<ChatMessage>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<PlanView, String> {
    let root = workspace(&state)?;
    if prompt.trim().is_empty() || prompt.len() > 8000 {
        return Err("Message is empty or too long".into());
    }
    if model.is_empty() || model.len() > 120 {
        return Err("Choose a model".into());
    }
    let mut messages = vec![json!({"role":"system", "content": SYSTEM_PROMPT})];
    for item in history
        .into_iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        if matches!(item.role.as_str(), "user" | "assistant") && item.content.len() <= 8000 {
            messages.push(json!({"role": item.role, "content": item.content}));
        }
    }
    messages.push(json!({"role":"user", "content":prompt}));
    let (mut reply, mut raw_reply) = request_agent_reply(&state.client, &model, &messages).await?;
    for attempt in 0..2 {
        if reply.actions.len() > 8 || reply.message.len() > 8000 {
            return Err("Model response exceeded limits".into());
        }
        let invalid = reply
            .actions
            .iter()
            .find_map(|action| validate_action(action, &root).err());
        if let Some(error) = invalid {
            if attempt == 1 {
                return Err(format!("Model proposed an invalid action: {error}"));
            }
            messages.push(json!({"role":"assistant", "content":raw_reply}));
            messages.push(json!({"role":"user", "content":format!("Your action plan is invalid: {error}. Correct the plan and return only JSON. A filename such as test.txt must be written with write_file, never create_dir.")}));
            (reply, raw_reply) = request_agent_reply(&state.client, &model, &messages).await?;
        } else {
            break;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let views: Vec<ActionView> = reply
        .actions
        .iter()
        .map(|a| ActionView {
            label: a.label().into(),
            detail: a.detail(),
            needs_confirmation: a.needs_confirmation(),
        })
        .collect();
    let needs_confirmation = reply.actions.iter().any(Action::needs_confirmation);
    if !reply.actions.is_empty() {
        state
            .plans
            .lock()
            .map_err(|_| "Plans are unavailable")?
            .insert(
                id.clone(),
                StoredPlan {
                    workspace: root,
                    actions: reply.actions,
                },
            );
        log_action(
            &app,
            "Plan",
            &format!("{} action(s)", views.len()),
            "proposed",
        );
    }
    Ok(PlanView {
        id,
        message: reply.message,
        actions: views,
        needs_confirmation,
    })
}

async fn request_agent_reply(
    client: &reqwest::Client,
    model: &str,
    messages: &[Value],
) -> Result<(AgentReply, String), String> {
    let mut last_error = String::new();
    for attempt in 0..2 {
        let mut request_messages = messages.to_vec();
        if attempt == 1 {
            request_messages.push(json!({"role":"user", "content":"Your previous final answer was empty or invalid. Return exactly one non-empty JSON object with message and actions."}));
        }
        let mut request =
            json!({"model":model,"messages":request_messages,"format":"json","stream":false});
        if model.starts_with("gpt-oss") {
            request["think"] = json!(if attempt == 0 { "medium" } else { "high" });
        }
        let response = client
            .post("http://127.0.0.1:11434/api/chat")
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Ollama request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("Ollama returned HTTP {}", response.status()));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Invalid Ollama response: {e}"))?;
        let content = body
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or("");
        if content.trim().is_empty() {
            let reason = body["done_reason"].as_str().unwrap_or("unknown");
            last_error = format!("Ollama returned an empty final answer (reason: {reason}).");
            continue;
        }
        match serde_json::from_str::<AgentReply>(content) {
            Ok(reply) => return Ok((reply, content.to_string())),
            Err(error) => last_error = format!("Model did not return a valid action plan: {error}"),
        }
    }
    Err(last_error)
}

#[tauri::command]
fn discard_plan(id: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let removed = state
        .plans
        .lock()
        .map_err(|_| "Plans are unavailable")?
        .remove(&id);
    if removed.is_some() {
        log_action(&app, "Plan", &id, "cancelled");
    }
    Ok(())
}

#[tauri::command]
fn propose_command(
    command: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<PlanView, String> {
    let root = workspace(&state)?;
    let action = Action::RunCommand { command };
    validate_action(&action, &root)?;
    let id = uuid::Uuid::new_v4().to_string();
    let view = ActionView {
        label: action.label().into(),
        detail: action.detail(),
        needs_confirmation: true,
    };
    state
        .plans
        .lock()
        .map_err(|_| "Plans are unavailable")?
        .insert(
            id.clone(),
            StoredPlan {
                workspace: root,
                actions: vec![action],
            },
        );
    log_action(&app, "Plan", "Manual command", "proposed");
    Ok(PlanView {
        id,
        message: "Review the command before running it.".into(),
        actions: vec![view],
        needs_confirmation: true,
    })
}

#[tauri::command]
async fn execute_plan(
    id: String,
    confirm_destructive: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Vec<ActionResult>, String> {
    let current = workspace(&state)?;
    let plan = state
        .plans
        .lock()
        .map_err(|_| "Plans are unavailable")?
        .remove(&id)
        .ok_or("Plan expired or was already run")?;
    if plan.workspace != current {
        return Err("Workspace changed. Request a new plan".into());
    }
    if plan.actions.iter().any(Action::needs_confirmation) && !confirm_destructive {
        return Err("Confirmation is required for file changes and commands".into());
    }
    let mut results = Vec::new();
    for action in plan.actions {
        let label = action.label().to_string();
        let detail = action.detail();
        let outcome = match validate_action(&action, &current) {
            Ok(()) => run_action(&state.client, &current, &action).await,
            Err(e) => Err(e),
        };
        let (status, output) = match outcome {
            Ok(value) => ("success".to_string(), value),
            Err(e) => ("error".to_string(), e),
        };
        log_action(&app, &label, &action.audit_detail(), &status);
        results.push(ActionResult {
            action: label,
            detail,
            status,
            output,
        });
        if results
            .last()
            .is_some_and(|result| result.status == "error")
        {
            break;
        }
    }
    Ok(results)
}

async fn run_action(
    client: &reqwest::Client,
    root: &Path,
    action: &Action,
) -> Result<String, String> {
    match action {
        Action::ListFiles { path } => {
            let target = relative_path(root, path)?;
            let mut names = fs::read_dir(target)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok())
                .take(200)
                .map(|e| {
                    let suffix = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        "/"
                    } else {
                        ""
                    };
                    format!("{}{}", e.file_name().to_string_lossy(), suffix)
                })
                .collect::<Vec<_>>();
            names.sort();
            Ok(if names.is_empty() {
                "Folder is empty".into()
            } else {
                names.join("\n")
            })
        }
        Action::ReadFile { path } => {
            let target = relative_path(root, path)?;
            let meta = fs::metadata(&target).map_err(|e| e.to_string())?;
            if !meta.is_file() || meta.len() > 100_000 {
                return Err("Only text files up to 100 KB can be read".into());
            }
            fs::read_to_string(target).map_err(|e| e.to_string())
        }
        Action::WriteFile { path, content } => write_text_file(root, path, content),
        Action::CreateDir { path } => {
            let target = relative_path(root, path)?;
            fs::create_dir_all(target).map_err(|e| e.to_string())?;
            Ok("Directory created".into())
        }
        Action::DeleteFile { path } => {
            let target = relative_path(root, path)?;
            if !target.is_file() {
                return Err("Only files can be deleted".into());
            }
            fs::remove_file(target).map_err(|e| e.to_string())?;
            Ok("File deleted".into())
        }
        Action::RunCommand { command } => {
            #[cfg(windows)]
            let mut child = {
                let mut c = tokio::process::Command::new("powershell");
                c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
                c
            };
            #[cfg(not(windows))]
            let mut child = {
                let mut c = tokio::process::Command::new("sh");
                c.args(["-c", command]);
                c
            };
            child.current_dir(root).kill_on_drop(true);
            let output = tokio::time::timeout(std::time::Duration::from_secs(20), child.output())
                .await
                .map_err(|_| "Command timed out after 20 seconds".to_string())?
                .map_err(|e| e.to_string())?;
            let text = format!(
                "Exit: {}\n{}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                return Err(text.chars().take(20_000).collect());
            }
            Ok(text.chars().take(20_000).collect())
        }
        Action::Weather { city } => weather(client, city).await,
        Action::CatFact => {
            let data: Value = client
                .get("https://catfact.ninja/fact")
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            Ok(data["fact"]
                .as_str()
                .unwrap_or("No fact returned")
                .to_string())
        }
        Action::GithubUser { username } => {
            let url = format!("https://api.github.com/users/{username}");
            let data: Value = client
                .get(url)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "{} (@{})\nBio: {}\nPublic repos: {} · Followers: {}\n{}",
                data["name"].as_str().unwrap_or("GitHub user"),
                data["login"].as_str().unwrap_or(username),
                data["bio"].as_str().unwrap_or("—"),
                data["public_repos"],
                data["followers"],
                data["html_url"].as_str().unwrap_or("")
            ))
        }
    }
}

fn write_text_file(root: &Path, path: &str, content: &str) -> Result<String, String> {
    let target = relative_path(root, path)?;
    if target == root {
        return Err("Cannot replace the workspace folder".into());
    }
    if !target.parent().is_some_and(Path::is_dir) {
        return Err("Parent directory does not exist".into());
    }
    if target.is_dir() {
        let file_name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid filename")?;
        let mut backup = None;
        for number in 1..=100 {
            let candidate = target.with_file_name(format!("{file_name}.folder-{number}"));
            if fs::symlink_metadata(&candidate).is_err() {
                backup = Some(candidate);
                break;
            }
        }
        let backup = backup.ok_or("Could not find a free name for the existing folder")?;
        fs::rename(&target, &backup)
            .map_err(|e| format!("Could not move the existing folder: {e}"))?;
        if let Err(error) = fs::write(&target, content) {
            let _ = fs::rename(&backup, &target);
            return Err(format!("Could not write file {path}: {error}"));
        }
        let moved_name = backup
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok(format!(
            "Wrote {} bytes. Existing folder was preserved as {}.",
            content.len(),
            moved_name
        ));
    }
    fs::write(&target, content).map_err(|e| format!("Could not write file {path}: {e}"))?;
    Ok(format!("Wrote {} bytes", content.len()))
}

async fn weather(client: &reqwest::Client, city: &str) -> Result<String, String> {
    let geo: Value = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", city), ("count", "1")])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let place = geo["results"]
        .as_array()
        .and_then(|r| r.first())
        .ok_or("City not found")?;
    let lat = place["latitude"].as_f64().ok_or("Invalid latitude")?;
    let lon = place["longitude"].as_f64().ok_or("Invalid longitude")?;
    let forecast: Value = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            (
                "current",
                "temperature_2m,relative_humidity_2m,wind_speed_10m".into(),
            ),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let current = &forecast["current"];
    Ok(format!(
        "{}, {}\nTemperature: {} °C · Humidity: {}% · Wind: {} km/h",
        place["name"].as_str().unwrap_or(city),
        place["country"].as_str().unwrap_or(""),
        current["temperature_2m"],
        current["relative_humidity_2m"],
        current["wind_speed_10m"]
    ))
}

#[tauri::command]
async fn demo_api(
    kind: String,
    input: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ActionResult, String> {
    let action = match kind.as_str() {
        "weather" => Action::Weather { city: input },
        "cat_fact" => Action::CatFact,
        "github_user" => Action::GithubUser { username: input },
        _ => return Err("Unknown demo API".into()),
    };
    validate_action(&action, Path::new("."))?;
    let label = action.label().to_string();
    let detail = action.detail();
    let outcome = run_action(&state.client, Path::new("."), &action).await;
    let (status, output) = match outcome {
        Ok(v) => ("success".to_string(), v),
        Err(e) => ("error".to_string(), e),
    };
    log_action(&app, &label, &detail, &status);
    Ok(ActionResult {
        action: label,
        detail,
        status,
        output,
    })
}

#[tauri::command]
fn get_audit(app: AppHandle) -> Vec<AuditEntry> {
    let Ok(dir) = app.path().app_data_dir() else {
        return vec![];
    };
    let Ok(content) = fs::read_to_string(dir.join("actions.jsonl")) else {
        return vec![];
    };
    content
        .lines()
        .rev()
        .take(30)
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("local-agent-demo/0.1")
        .build()
        .expect("HTTP client");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            client,
            workspace: Mutex::new(None),
            plans: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            ollama_status,
            set_workspace,
            ask_agent,
            execute_plan,
            discard_plan,
            propose_command,
            demo_api,
            get_audit
        ])
        .run(tauri::generate_context!())
        .expect("error while running Local Agent");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_parent_and_absolute_paths() {
        let root = Path::new("C:/workspace");
        assert!(relative_path(root, "../outside").is_err());
        assert!(relative_path(root, "C:/outside").is_err());
        assert!(relative_path(root, "notes/today.txt").is_ok());
    }
    #[test]
    fn rejects_bad_github_username() {
        assert!(validate_action(
            &Action::GithubUser {
                username: "a/b".into()
            },
            Path::new(".")
        )
        .is_err());
    }
    #[test]
    fn file_write_preserves_conflicting_folder() {
        let root = std::env::temp_dir().join(format!("local-agent-test-{}", uuid::Uuid::new_v4()));
        let old_folder = root.join("test.txt");
        fs::create_dir_all(&old_folder).unwrap();
        fs::write(old_folder.join("hello.txt"), "old content").unwrap();
        let message = write_text_file(&root, "test.txt", "hello").unwrap();
        assert_eq!(fs::read_to_string(root.join("test.txt")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(root.join("test.txt.folder-1/hello.txt")).unwrap(),
            "old content"
        );
        assert!(message.contains("preserved"));
        fs::remove_file(root.join("test.txt")).unwrap();
        fs::remove_file(root.join("test.txt.folder-1/hello.txt")).unwrap();
        fs::remove_dir(root.join("test.txt.folder-1")).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn rejects_directory_named_like_text_file() {
        assert!(validate_action(
            &Action::CreateDir {
                path: "test.txt".into()
            },
            Path::new(".")
        )
        .is_err());
    }
    #[test]
    fn rejects_workspace_root_as_file() {
        let root = Path::new("C:/workspace");
        assert!(validate_action(
            &Action::WriteFile {
                path: "./".into(),
                content: "hello".into()
            },
            root
        )
        .is_err());
    }
}
