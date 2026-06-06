use crate::markdown::MarkdownContent;
use leptos::prelude::*;
use oxide_agent_web_contracts::PersistedTaskEvent;
use serde_json::Value;

use super::payload::{
    field_i64, field_str, input_preview_field_str, input_preview_json, is_sub_agent_event,
    parse_output_json, payload_str_event, raw_output_preview, stream_text,
};

// ── Tool Card (groups call + result) ─────────────────────────────────────

#[component]
pub(super) fn ToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
) -> impl IntoView {
    let tool_name = call
        .as_ref()
        .or(result.as_ref())
        .and_then(|e| payload_str_event(e, "name"))
        .unwrap_or_default();

    let outcome = tool_outcome(result.as_ref());
    let is_sub_agent = tool_event_is_sub_agent(call.as_ref(), result.as_ref());

    // Parse the nested output JSON from output_preview.
    let output_json = result.as_ref().and_then(parse_output_json);

    let display = match tool_name.as_str() {
        "execute_command" => {
            view! { <ShellToolCard call=call result=result output=output_json /> }.into_any()
        }
        "web_search" | "tavily_search" | "duckduckgo_search" | "duckduckgo_news" => view! {
            <SearchToolCard label="Web search" preview_query_first=false call=call result=result output=output_json />
        }
        .into_any(),
        "brave_search" => view! {
            <SearchToolCard label="Brave Search" preview_query_first=true call=call result=result output=output_json />
        }
        .into_any(),
        "searxng_search" => view! {
            <SearchToolCard label="SearXNG" preview_query_first=false call=call result=result output=output_json />
        }
        .into_any(),
        "web_markdown" => {
            view! { <WebMarkdownToolCard call=call result=result output=output_json /> }.into_any()
        }
        "crawl4ai_markdown" => {
            view! { <CrawlToolCard call=call result=result output=output_json /> }.into_any()
        }
        "spawn_sub_agents" => {
            view! { <SpawnSubAgentsToolCard call=call result=result output=output_json /> }.into_any()
        }
        "wait_sub_agents" => {
            view! { <WaitSubAgentsToolCard call=call result=result output=output_json /> }.into_any()
        }
        "write_todos" => {
            view! { <WriteTodosToolCard call=call result=result output=output_json /> }.into_any()
        }
        _ => {
            view! { <GenericToolCard name=tool_name call=call result=result output=output_json /> }
                .into_any()
        }
    };

    let status_class = if is_sub_agent {
        format!("{} sub-agent", outcome.status_class())
    } else {
        outcome.status_class().to_string()
    };

    view! {
        <section class=status_class>
            {display}
        </section>
    }
}

fn tool_event_is_sub_agent(
    call: Option<&PersistedTaskEvent>,
    result: Option<&PersistedTaskEvent>,
) -> bool {
    call.is_some_and(is_sub_agent_event) || result.is_some_and(is_sub_agent_event)
}

// ── Shell Tool Card (execute_command) ────────────────────────────────────

#[component]
fn ShellToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;

    let status_text = if is_running {
        "running".to_string()
    } else if success {
        "ok".to_string()
    } else {
        output
            .as_ref()
            .and_then(|v| field_str(v, "status"))
            .unwrap_or_else(|| "failed".to_string())
    };

    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());
    let exit_code = output.as_ref().and_then(|v| field_i64(v, "exit_code"));
    let command = command_from_events(call.as_ref(), output.as_ref());
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let stderr = output.as_ref().and_then(|v| stream_text(v, "stderr"));
    let error_msg = output.as_ref().and_then(|v| field_str(v, "error_message"));
    let icon = outcome.icon();
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = vec![tool_meta(status_text)];
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(code) = exit_code {
        let meta = format!("exit {code}");
        if code != 0 {
            header_metas.push(tool_meta_danger(meta));
        } else {
            header_metas.push(tool_meta(meta));
        }
    }
    if let Some(message) = error_msg {
        header_metas.push(tool_meta_danger(message));
    }

    // Default open: running, failed, or has no stream content (nothing to collapse)
    let has_streams = stdout.is_some() || stderr.is_some();
    let default_open = is_running || !success || !has_streams;

    // Compact preview: command or first line of stdout
    let preview_text = command
        .clone()
        .or_else(|| stdout.as_ref().map(|t| first_line(t)));

    view! {
        {tool_card_header(icon, "Shell", header_metas)}
        {preview_text.map(|text| tool_preview(format!("$ {text}")))}
        <ToolDetails open=default_open>
            {command.map(tool_command)}
            {stdout.clone().map(|text| tool_pre_stream(Some("stdout"), text))}
            {stderr.clone().map(|text| tool_pre_stream(Some("stderr"), text))}
            {(stdout.is_none() && stderr.is_none() && !is_running)
                .then(|| tool_pre_stream(None, "No output".to_string()))}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

// ── Search Tool Card (web_search / tavily_search) ────────────────────────

#[component]
fn SearchToolCard(
    label: &'static str,
    preview_query_first: bool,
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;

    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());
    let query = call
        .as_ref()
        .and_then(|e| payload_str_event(e, "input_preview"));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    // Try to parse structured results from structured_payload.
    let search_results: Vec<SearchResult> = output
        .as_ref()
        .and_then(|sp| {
            sp.get("structured_payload")
                .and_then(|v| v.get("results"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            Some(SearchResult {
                                title: item.get("title")?.as_str()?.to_string(),
                                url: item.get("url")?.as_str()?.to_string(),
                                snippet: item
                                    .get("snippet")
                                    .or_else(|| item.get("description"))
                                    .or_else(|| item.get("content"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
        })
        .unwrap_or_default();

    let result_count = if search_results.is_empty() {
        None
    } else {
        Some(search_results.len())
    };

    let icon = outcome.icon();

    let default_open = is_running || !success;

    let preview_snippet = search_results
        .first()
        .filter(|sr| !sr.snippet.is_empty())
        .map(|sr| sr.snippet.clone());
    let stdout_headline = stdout.as_ref().map(|text| first_line(text));
    // For Brave Search, query is the compact preview (stable, like Crawl's URL host).
    let preview_text = if success && preview_query_first {
        query
            .clone()
            .or_else(|| preview_snippet.clone())
            .or_else(|| stdout_headline.clone())
    } else if success {
        preview_snippet.or_else(|| {
            search_results
                .first()
                .filter(|sr| !sr.title.is_empty())
                .map(|sr| sr.title.clone())
                .or_else(|| stdout_headline.clone())
                .or_else(|| query.clone())
        })
    } else {
        result_summary.clone().or(query.clone())
    };
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = Vec::new();
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(count) = result_count {
        header_metas.push(tool_meta(format!("{count} results")));
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, label, header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {query.map(|q| tool_query_row("Query", q))}
            {if !search_results.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {search_results.into_iter().take(8).map(|sr| view! {
                            <li class="search-result-item">
                                <div class="search-result-title">{sr.title}</div>
                                <div class="search-result-url">{sr.url}</div>
                                {(!sr.snippet.is_empty()).then(|| view! {
                                    <p class="search-result-snippet">{sr.snippet}</p>
                                })}
                            </li>
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout {
                tool_markdown_stream(Some("output"), text)
            } else {
                ().into_any()
            }}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

// ── Crawl Tool Card (crawl4ai_markdown) ───────────────────────────────────

/// Extract hostname (with port) from a URL string for compact preview.
fn host_from_url_str(raw: &str) -> Option<String> {
    let s = raw
        .strip_prefix("https://")
        .or(raw.strip_prefix("http://"))?;
    let host = s.split('?').next().unwrap_or(s);
    let host = host.split('/').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Human-readable byte/char count (e.g. "5.0K chars").
fn chars_label(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M chars", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K chars", count as f64 / 1_000.0)
    } else {
        format!("{count} chars")
    }
}

#[derive(Clone, Default)]
struct WebMarkdownOutput {
    url: Option<String>,
    content_type: Option<String>,
    fetched_bytes: Option<u64>,
    truncated: bool,
    markdown: Option<String>,
}

fn parse_web_markdown_stdout(text: &str) -> Option<WebMarkdownOutput> {
    let normalized = text.replace("\r\n", "\n");
    let rest = normalized.strip_prefix("## Web Markdown\n\n")?;
    let (metadata, body) = rest.split_once("\n\n").unwrap_or((rest, ""));

    let mut parsed = WebMarkdownOutput::default();
    for line in metadata.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();

        match key.trim() {
            "URL" if !value.is_empty() => parsed.url = Some(value.to_string()),
            "Content-Type" if !value.is_empty() => parsed.content_type = Some(value.to_string()),
            "Fetched-Bytes" => parsed.fetched_bytes = value.parse::<u64>().ok(),
            "Truncated" => {
                parsed.truncated = value.eq_ignore_ascii_case("yes")
                    || value.eq_ignore_ascii_case("true")
                    || value == "1";
            }
            _ => {}
        }
    }

    if !body.is_empty() {
        parsed.markdown = Some(body.to_string());
    }

    Some(parsed)
}

#[component]
fn WebMarkdownToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;

    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());

    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    let stdout_text = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let web_markdown = stdout_text
        .as_ref()
        .and_then(|text| parse_web_markdown_stdout(text));
    let parsed_header = web_markdown.is_some();

    let url = web_markdown
        .as_ref()
        .and_then(|doc| doc.url.clone())
        .or_else(|| tool_url_from_structured_or_input(output.as_ref(), call.as_ref()));
    let content_type = web_markdown
        .as_ref()
        .and_then(|doc| doc.content_type.clone());
    let fetched_bytes = web_markdown.as_ref().and_then(|doc| doc.fetched_bytes);
    let truncated = web_markdown
        .as_ref()
        .map(|doc| doc.truncated)
        .unwrap_or(false);
    let markdown = if parsed_header {
        web_markdown.as_ref().and_then(|doc| doc.markdown.clone())
    } else {
        stdout_text.clone()
    };

    let preview_host = url.as_ref().and_then(|u| host_from_url_str(u));

    let icon = outcome.icon();

    let chars_display = markdown
        .as_ref()
        .map(|text| chars_label(text.chars().count() as u64));
    let default_open = is_running || !success;

    let preview_text = if !success {
        result_summary.clone()
    } else {
        preview_host.clone()
    };
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = Vec::new();
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(chars) = chars_display {
        header_metas.push(tool_meta(chars));
    }
    if truncated {
        header_metas.push(tool_meta("truncated"));
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, "Web Markdown", header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {url.clone().map(|u| tool_query_row("URL", u))}
            {content_type.map(|value| tool_query_row("Content-Type", value))}
            {fetched_bytes.map(|bytes| tool_query_row("Fetched", format!("{bytes} bytes")))}
            {parsed_header.then(|| {
                tool_query_row(
                    "Truncated",
                    if truncated { "yes" } else { "no" }.to_string(),
                )
            })}
            {if let Some(md) = markdown.filter(|m| !m.is_empty()) {
                tool_markdown_stream(None, md)
            } else if !is_running {
                tool_pre_stream(None, "No content".to_string())
            } else {
                ().into_any()
            }}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

#[component]
fn CrawlToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;

    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());

    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    // Parse the inner JSON from stdout.text (crawl4ai success payload). Large
    // tool outputs are truncated in output_preview, so fall back to the compact
    // display_payload persisted by the web transport.
    let stdout_text = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let crawl_from_stdout: Option<Value> = stdout_text
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let crawl_from_display = result
        .as_ref()
        .and_then(|event| event.payload.get("display_payload"))
        .filter(|payload| {
            payload.get("provider").and_then(Value::as_str) == Some("crawl4ai_markdown")
        })
        .cloned();
    let crawl = crawl_from_stdout.or(crawl_from_display);
    let show_raw_stdout_fallback = success && crawl.is_none();
    let url: Option<String> = crawl.as_ref().and_then(|v| {
        v.get("final_url")
            .or_else(|| v.get("url"))
            .and_then(Value::as_str)
            .map(String::from)
    });
    let markdown: Option<String> = crawl
        .as_ref()
        .and_then(|v| v.get("markdown").and_then(Value::as_str).map(String::from));
    let chars = crawl
        .as_ref()
        .and_then(|v| v.get("chars").and_then(Value::as_u64));
    let status_code = crawl
        .as_ref()
        .and_then(|v| v.get("status_code").and_then(Value::as_u64));
    let markdown_kind = crawl.as_ref().and_then(|v| {
        v.get("markdown_kind")
            .and_then(Value::as_str)
            .map(String::from)
    });
    let fresh = crawl
        .as_ref()
        .and_then(|v| v.get("fresh").and_then(Value::as_bool));
    let truncated = crawl
        .as_ref()
        .and_then(|v| v.get("truncated").and_then(Value::as_bool))
        .unwrap_or(false)
        || crawl
            .as_ref()
            .and_then(|v| v.get("markdown_preview_truncated").and_then(Value::as_bool))
            .unwrap_or(false);

    // Fallback: try to extract URL from the ToolCall input_preview.
    let failure_payload = tool_structured_payload(output.as_ref());
    let url = url.or_else(|| tool_url_from_structured_or_input(output.as_ref(), call.as_ref()));

    let preview_host = url.as_ref().and_then(|u| host_from_url_str(u));

    let icon = outcome.icon();

    let chars_display = chars.map(chars_label);
    let default_open = is_running;
    let failure_error_kind = (!success)
        .then(|| {
            failure_payload
                .and_then(|payload| payload.get("error_kind"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .flatten();
    let failure_label = (!success)
        .then(|| {
            result_summary
                .clone()
                .or_else(|| failure_error_kind.clone())
        })
        .flatten();
    let failure_status_code = (!success)
        .then(|| {
            failure_payload
                .and_then(|payload| payload.get("status_code"))
                .and_then(Value::as_i64)
        })
        .flatten();
    let failure_message = (!success)
        .then(|| {
            failure_payload
                .and_then(|payload| payload.get("message"))
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| output.as_ref().and_then(|v| field_str(v, "error_message")))
        })
        .flatten();
    let failure_response_tail = (!success)
        .then(|| {
            failure_payload
                .and_then(|payload| payload.get("response_tail"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|tail| !tail.is_empty())
                .map(String::from)
        })
        .flatten();

    // Preview line: hostname or error summary
    let preview_text = if !success {
        preview_host.clone().or_else(|| failure_label.clone())
    } else {
        preview_host.clone()
    };
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = Vec::new();
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(chars) = chars_display {
        header_metas.push(tool_meta(chars));
    }
    if truncated {
        header_metas.push(tool_meta("truncated"));
    }
    if let Some(summary) = failure_label.clone() {
        header_metas.push(tool_meta_danger(summary));
    }

    view! {
        {tool_card_header(icon, "Crawl", header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {url.clone().map(|u| tool_query_row("URL", u))}
            {status_code.map(|code| tool_query_row("Status", code.to_string()))}
            {markdown_kind.map(|kind| tool_query_row("Markdown", kind))}
            {fresh.map(|fresh| tool_query_row("Fresh", if fresh { "yes" } else { "no" }.to_string()))}
            {failure_label.clone().map(|label| tool_query_row("Error", label))}
            {failure_status_code.map(|code| tool_query_row("Status", code.to_string()))}
            {failure_message.map(|message| tool_pre_stream(Some("message"), message))}
            {failure_response_tail.map(|tail| tool_pre_stream(Some("response tail"), tail))}
            {if let Some(md) = markdown.filter(|m| !m.is_empty()) {
                tool_markdown_stream(None, md.to_string())
            } else if success && !is_running {
                tool_pre_stream(None, "No content".to_string())
            } else {
                ().into_any()
            }}
            // If stdout was not parseable as crawl JSON, show raw stdout as fallback.
            {show_raw_stdout_fallback
                .then(|| stdout_text.clone())
                .flatten()
                .map(|text| tool_pre_stream(Some("output"), text))}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

// ── Delegation/Todos Tool Cards ───────────────────────────────────────────

#[derive(Clone, Default)]
struct SubAgentTaskView {
    id: Option<String>,
    task: String,
    status: String,
    tools: Vec<String>,
    context: Option<String>,
}

#[derive(Clone, Default)]
struct SubAgentStatusView {
    id: String,
    task: Option<String>,
    status: String,
    output: Option<String>,
    elapsed_ms: Option<u64>,
    completed: Option<bool>,
}

#[derive(Clone)]
pub(super) struct TodoToolItemView {
    description: String,
    status: String,
}

#[component]
fn SpawnSubAgentsToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let parsed = stdout
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());

    let requested = parse_sub_agent_tasks_from_call(call.as_ref());
    let mut tasks = parsed
        .as_ref()
        .and_then(parse_spawned_sub_agent_tasks)
        .unwrap_or_default();
    if tasks.is_empty() {
        tasks = requested.clone();
        for task in &mut tasks {
            if task.status.is_empty() {
                task.status = if is_running { "starting" } else { "requested" }.to_string();
            }
        }
    } else {
        for (idx, task) in tasks.iter_mut().enumerate() {
            if let Some(requested_task) = requested.get(idx) {
                task.tools = requested_task.tools.clone();
                task.context = requested_task.context.clone();
                if task.task.is_empty() {
                    task.task = requested_task.task.clone();
                }
            }
        }
    }

    let active_label = active_count_label(parsed.as_ref());

    let icon = outcome.icon();
    let started_count = tasks.len();
    let count_label = (started_count > 0).then(|| format!("{started_count} started"));
    let preview_text = if !success {
        result_summary.clone()
    } else {
        tasks
            .first()
            .map(|task| first_line(&task.task))
            .or_else(|| stdout.as_ref().map(|text| first_line(text)))
    };
    let default_open = is_running || !success || tasks.is_empty();
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = Vec::new();
    if let Some(label) = count_label {
        header_metas.push(tool_meta(label));
    }
    if let Some(label) = active_label {
        header_metas.push(tool_meta(label));
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, "Sub-agents", header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {if !tasks.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {tasks.into_iter().map(|task| {
                            let meta = sub_agent_task_meta(&task);
                            view! {
                                <li class="search-result-item">
                                    <div class="search-result-title">{task.task}</div>
                                    <div class="search-result-url">{meta}</div>
                                    {task.context.filter(|context| !context.is_empty()).map(|context| view! {
                                        <p class="search-result-snippet">{context}</p>
                                    })}
                                </li>
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout.clone() {
                tool_pre_stream(Some("output"), text)
            } else if !is_running {
                tool_pre_stream(None, "No sub-agents".to_string())
            } else {
                ().into_any()
            }}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

#[component]
fn WaitSubAgentsToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let parsed = stdout
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());

    let mut statuses = parsed
        .as_ref()
        .and_then(parse_sub_agent_statuses)
        .unwrap_or_default();
    if statuses.is_empty() && is_running {
        statuses = parse_sub_agent_wait_ids_from_call(call.as_ref());
    }

    let timed_out = parsed
        .as_ref()
        .and_then(|v| v.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let active_label = active_count_label(parsed.as_ref());

    let total = statuses.len();
    let completed = statuses
        .iter()
        .filter(|status| status.completed.unwrap_or(false) || status.status == "completed")
        .count();
    let failed = statuses
        .iter()
        .filter(|status| matches!(status.status.as_str(), "failed" | "timed_out" | "cancelled"))
        .count();
    let count_label = (total > 0).then(|| format!("{completed}/{total} done"));
    let failed_label = (failed > 0).then(|| format!("{failed} failed"));

    let icon = outcome.icon();
    let preview_text = if !success {
        result_summary.clone()
    } else {
        statuses
            .iter()
            .find_map(|status| status.output.as_ref().map(|output| first_line(output)))
            .or_else(|| statuses.first().map(sub_agent_status_preview))
            .or_else(|| stdout.as_ref().map(|text| first_line(text)))
    };
    let default_open = is_running || !success || timed_out || failed > 0;
    let raw_output = raw_output_preview(result.as_ref());

    let mut header_metas = Vec::new();
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(label) = count_label {
        header_metas.push(tool_meta(label));
    }
    if let Some(label) = active_label {
        header_metas.push(tool_meta(label));
    }
    if timed_out {
        header_metas.push(tool_meta_danger("timed out"));
    }
    if let Some(label) = failed_label {
        header_metas.push(tool_meta_danger(label));
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, "Sub-agent results", header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {if !statuses.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {statuses.into_iter().map(|status| {
                            let title = status.task.clone().unwrap_or_else(|| status.id.clone());
                            let meta = sub_agent_status_meta(&status);
                            view! {
                                <li class="search-result-item">
                                    <div class="search-result-title">{title}</div>
                                    <div class="search-result-url">{meta}</div>
                                    {status.output.filter(|output| !output.is_empty()).map(|output| view! {
                                        <div class="tool-stream-content">
                                            <MarkdownContent markdown=output />
                                        </div>
                                    })}
                                </li>
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout.clone() {
                tool_pre_stream(Some("output"), text)
            } else if !is_running {
                tool_pre_stream(None, "No sub-agent results".to_string())
            } else {
                ().into_any()
            }}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

#[component]
fn WriteTodosToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let todos = parse_todo_items_from_call(call.as_ref());

    let total = todos.len();
    let completed = todos
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let active = todos.iter().find(|item| item.status == "in_progress");
    let blocked = todos.iter().find(|item| item.status == "blocked_on_user");
    let count_label = (total > 0).then(|| format!("{completed}/{total} done"));
    let state_label = blocked
        .map(|_| "blocked")
        .or_else(|| active.map(|_| "doing"));
    let has_todos = !todos.is_empty();

    let icon = outcome.icon();
    let preview_text = (!success).then(|| result_summary.clone()).flatten();
    let raw_output = raw_output_preview(result.as_ref());
    let show_fallback = !has_todos && (stdout.is_some() || !is_running);
    let show_details = raw_output.is_some() || show_fallback;
    let default_open = !success || !has_todos;

    let mut header_metas = Vec::new();
    if let Some(label) = count_label {
        header_metas.push(tool_meta(label));
    }
    if let Some(label) = state_label {
        if label == "blocked" {
            header_metas.push(tool_meta_danger(label));
        } else {
            header_metas.push(tool_meta(label));
        }
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, "Todos", header_metas)}
        {preview_text.map(tool_preview)}
        {has_todos.then(|| render_todo_list(todos.clone(), false))}
        {show_details.then(|| view! {
            <ToolDetails open=default_open>
                {if show_fallback {
                    if let Some(text) = stdout.clone() {
                        tool_pre_stream(Some("output"), text)
                    } else {
                        tool_pre_stream(None, "No todos".to_string())
                    }
                } else {
                    ().into_any()
                }}
                {raw_output.map(tool_raw_details)}
            </ToolDetails>
        })}
    }
}

// ── Generic Tool Card (fallback) ─────────────────────────────────────────

#[component]
fn GenericToolCard(
    name: String,
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let outcome = tool_outcome(result.as_ref());
    let is_running = outcome.is_running;
    let success = outcome.success;

    let duration_label = tool_duration_label(output.as_ref(), result.as_ref());
    let exit_code = output.as_ref().and_then(|v| field_i64(v, "exit_code"));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let stderr = output.as_ref().and_then(|v| stream_text(v, "stderr"));
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    let icon = outcome.icon();

    let has_streams = stdout.is_some() || stderr.is_some();
    let default_open = is_running || !success || !has_streams;

    // Compact preview: command_preview or first line of stdout
    let command_preview = call
        .as_ref()
        .and_then(|e| payload_str_event(e, "command_preview"));
    let preview_text = command_preview
        .or_else(|| stdout.as_ref().map(|t| first_line(t)))
        .or_else(|| (!success).then(|| result_summary.clone()).flatten());
    let raw_output = raw_output_preview(result.as_ref());
    let command_detail = call
        .as_ref()
        .and_then(|e| payload_str_event(e, "command_preview"));

    let mut header_metas = Vec::new();
    if let Some(duration) = duration_label {
        header_metas.push(tool_meta(duration));
    }
    if let Some(code) = exit_code {
        let meta = format!("exit {code}");
        if code != 0 {
            header_metas.push(tool_meta_danger(meta));
        } else {
            header_metas.push(tool_meta(meta));
        }
    }
    if !success {
        if let Some(summary) = result_summary.clone() {
            header_metas.push(tool_meta_danger(summary));
        }
    }

    view! {
        {tool_card_header(icon, name, header_metas)}
        {preview_text.map(tool_preview)}
        <ToolDetails open=default_open>
            {command_detail.map(tool_command)}
            {stdout.map(|text| tool_pre_stream(Some("output"), text))}
            {stderr.map(|text| tool_pre_stream(Some("stderr"), text))}
            {raw_output.map(tool_raw_details)}
        </ToolDetails>
    }
}

/// Extract first line from text, truncated to max chars.
fn first_line(text: &str) -> String {
    const MAX_CHARS: usize = 120;

    let line = text.lines().next().unwrap_or("");
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

#[derive(Clone, Copy)]
struct ToolOutcome {
    is_running: bool,
    success: bool,
}

impl ToolOutcome {
    fn icon(self) -> &'static str {
        tool_status_icon(self.is_running, self.success)
    }

    fn status_class(self) -> &'static str {
        if self.is_running {
            "tool-card running"
        } else if self.success {
            "tool-card success"
        } else {
            "tool-card failure"
        }
    }
}

fn tool_outcome(result: Option<&PersistedTaskEvent>) -> ToolOutcome {
    ToolOutcome {
        is_running: result.is_none(),
        success: result
            .and_then(|e| e.payload.get("success"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

fn tool_duration_ms(output: Option<&Value>, result: Option<&PersistedTaskEvent>) -> Option<i64> {
    output
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        })
}

fn tool_duration_label(
    output: Option<&Value>,
    result: Option<&PersistedTaskEvent>,
) -> Option<String> {
    tool_duration_ms(output, result).map(format_duration_ms)
}

fn format_duration_ms(ms: i64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn tool_status_icon(is_running: bool, success: bool) -> &'static str {
    if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    }
}

fn tool_structured_payload(output: Option<&Value>) -> Option<&Value> {
    output.and_then(|v| v.get("structured_payload"))
}

fn tool_structured_payload_str(output: Option<&Value>, key: &str) -> Option<String> {
    tool_structured_payload(output)
        .and_then(|payload| payload.get(key))
        .and_then(Value::as_str)
        .map(String::from)
}

fn tool_url_from_structured_or_input(
    output: Option<&Value>,
    call: Option<&PersistedTaskEvent>,
) -> Option<String> {
    tool_structured_payload_str(output, "url").or_else(|| input_preview_field_str(call, "url"))
}

fn active_count_label(payload: Option<&Value>) -> Option<String> {
    let active_count = payload
        .and_then(|v| v.get("active_count"))
        .and_then(Value::as_u64)?;
    let max_active = payload
        .and_then(|v| v.get("max_active"))
        .and_then(Value::as_u64);

    Some(match max_active {
        Some(max) => format!("active {active_count}/{max}"),
        None => format!("active {active_count}"),
    })
}

#[derive(Clone)]
pub(super) struct ToolHeaderMeta {
    text: String,
    danger: bool,
}

pub(super) fn tool_meta(text: impl Into<String>) -> ToolHeaderMeta {
    ToolHeaderMeta {
        text: text.into(),
        danger: false,
    }
}

pub(super) fn tool_meta_danger(text: impl Into<String>) -> ToolHeaderMeta {
    ToolHeaderMeta {
        text: text.into(),
        danger: true,
    }
}

fn tool_card_header(
    icon: &'static str,
    name: impl Into<String>,
    metas: Vec<ToolHeaderMeta>,
) -> AnyView {
    tool_card_header_with_icon_class(icon, "tool-status-icon", name, metas)
}

pub(super) fn tool_card_header_with_icon_class(
    icon: &'static str,
    icon_class: &'static str,
    name: impl Into<String>,
    metas: Vec<ToolHeaderMeta>,
) -> AnyView {
    let name = name.into();

    view! {
        <div class="tool-card-header">
            <span class=icon_class>{icon}</span>
            <span class="tool-name">{name}</span>
            {metas.into_iter().map(|meta| {
                let class = if meta.danger { "tool-meta danger" } else { "tool-meta" };
                view! { <span class=class>{meta.text}</span> }
            }).collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

fn tool_preview(text: String) -> AnyView {
    tool_preview_with_class(text, "tool-preview")
}

pub(super) fn tool_preview_with_class(text: String, class: &'static str) -> AnyView {
    view! { <div class=class>{text}</div> }.into_any()
}

#[component]
fn ToolDetails(open: bool, children: Children) -> impl IntoView {
    view! {
        <ToolDetailsWithClass open=open class="tool-card-body">
            {children()}
        </ToolDetailsWithClass>
    }
}

#[component]
pub(super) fn ToolDetailsWithClass(
    open: bool,
    class: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <details class=class open=open>
            <summary class="tool-card-expand">"details"</summary>
            {children()}
        </details>
    }
}

fn tool_query_row(label: &'static str, value: String) -> AnyView {
    view! {
        <div class="tool-query">
            <span class="tool-label">{label}</span>
            <code>{value}</code>
        </div>
    }
    .into_any()
}

fn tool_command(command: String) -> AnyView {
    view! { <pre class="tool-command">{format!("$ {command}")}</pre> }.into_any()
}

pub(super) fn tool_pre_stream(label: Option<&'static str>, text: String) -> AnyView {
    view! {
        <div class="tool-stream">
            {label.map(|label| view! { <div class="tool-stream-label">{label}</div> })}
            <pre class="tool-stream-pre">{text}</pre>
        </div>
    }
    .into_any()
}

fn tool_markdown_stream(label: Option<&'static str>, markdown: String) -> AnyView {
    view! {
        <div class="tool-stream">
            {label.map(|label| view! { <div class="tool-stream-label">{label}</div> })}
            <div class="tool-stream-content">
                <MarkdownContent markdown=markdown />
            </div>
        </div>
    }
    .into_any()
}

fn tool_raw_details(raw: Value) -> AnyView {
    view! {
        <details class="tool-raw-details">
            <summary>"Raw"</summary>
            <pre class="tool-raw-json">{raw.to_string()}</pre>
        </details>
    }
    .into_any()
}

fn parse_sub_agent_tasks_from_call(call: Option<&PersistedTaskEvent>) -> Vec<SubAgentTaskView> {
    input_preview_json(call)
        .and_then(|payload| {
            payload.get("tasks").and_then(Value::as_array).map(|tasks| {
                tasks
                    .iter()
                    .map(|task| {
                        let tools = task
                            .get("tools")
                            .and_then(Value::as_array)
                            .map(|tools| {
                                tools
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(ToString::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();

                        SubAgentTaskView {
                            id: None,
                            task: task
                                .get("task")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            status: String::new(),
                            tools,
                            context: task
                                .get("context")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                        }
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn parse_spawned_sub_agent_tasks(payload: &Value) -> Option<Vec<SubAgentTaskView>> {
    payload
        .get("started")
        .and_then(Value::as_array)
        .map(|started| {
            started
                .iter()
                .map(|task| SubAgentTaskView {
                    id: task
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    task: task
                        .get("task")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    status: task
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("running")
                        .to_string(),
                    tools: Vec::new(),
                    context: None,
                })
                .collect()
        })
}

fn sub_agent_task_meta(task: &SubAgentTaskView) -> String {
    let mut parts = Vec::new();
    if let Some(id) = &task.id {
        if !id.is_empty() {
            parts.push(format!("id: {id}"));
        }
    }
    if !task.status.is_empty() {
        parts.push(format!("status: {}", task.status));
    }
    if !task.tools.is_empty() {
        parts.push(format!("tools: {}", task.tools.join(", ")));
    }
    if parts.is_empty() {
        "sub-agent".to_string()
    } else {
        parts.join(" · ")
    }
}

fn parse_sub_agent_statuses(payload: &Value) -> Option<Vec<SubAgentStatusView>> {
    payload
        .get("statuses")
        .and_then(Value::as_array)
        .map(|statuses| {
            statuses
                .iter()
                .map(|status| SubAgentStatusView {
                    id: status
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    task: status
                        .get("task")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    status: status
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    output: status
                        .get("output")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    elapsed_ms: status.get("elapsed_ms").and_then(Value::as_u64),
                    completed: status.get("completed").and_then(Value::as_bool),
                })
                .collect()
        })
}

fn parse_sub_agent_wait_ids_from_call(
    call: Option<&PersistedTaskEvent>,
) -> Vec<SubAgentStatusView> {
    input_preview_json(call)
        .and_then(|payload| {
            payload.get("ids").and_then(Value::as_array).map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| SubAgentStatusView {
                        id: id.to_string(),
                        task: None,
                        status: "waiting".to_string(),
                        output: None,
                        elapsed_ms: None,
                        completed: None,
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn sub_agent_status_preview(status: &SubAgentStatusView) -> String {
    status
        .task
        .as_ref()
        .map(|task| format!("{}: {}", status.status, first_line(task)))
        .unwrap_or_else(|| format!("{}: {}", status.status, status.id))
}

fn sub_agent_status_meta(status: &SubAgentStatusView) -> String {
    let mut parts = Vec::new();
    if !status.id.is_empty() {
        parts.push(format!("id: {}", status.id));
    }
    if !status.status.is_empty() {
        parts.push(format!("status: {}", status.status));
    }
    if let Some(elapsed_ms) = status.elapsed_ms {
        let elapsed_ms = elapsed_ms.min(i64::MAX as u64) as i64;
        parts.push(format!("elapsed: {}", format_duration_ms(elapsed_ms)));
    }
    if parts.is_empty() {
        "sub-agent".to_string()
    } else {
        parts.join(" · ")
    }
}

fn parse_todo_items_from_call(call: Option<&PersistedTaskEvent>) -> Vec<TodoToolItemView> {
    input_preview_json(call)
        .map(|payload| parse_todo_items_from_value(&payload))
        .unwrap_or_default()
}

pub(super) fn parse_todo_items_from_value(value: &Value) -> Vec<TodoToolItemView> {
    value
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| {
            value
                .get("todos")
                .and_then(|todos| todos.get("items"))
                .and_then(Value::as_array)
        })
        .or_else(|| value.get("todos").and_then(Value::as_array))
        .map(|items| {
            items
                .iter()
                .map(|item| TodoToolItemView {
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn render_todo_list(items: Vec<TodoToolItemView>, show_title: bool) -> AnyView {
    if items.is_empty() {
        return ().into_any();
    }

    view! {
        <section class="todos-card">
            {show_title.then(|| view! {
                <div class="todos-card-title">"Todos"</div>
            })}
            <ol class="todo-list">
                {items.into_iter().map(|item| {
                    let label = todo_status_label(&item.status);
                    let marker = todo_status_marker(label);

                    view! {
                        <li class=format!("todo-item {} {label}", item.status)>
                            <span class=format!("todo-check {label}")>{marker}</span>
                            <span class=format!("todo-status-badge {label}")>{label}</span>
                            <span class=format!("todo-description todo-text {label}")>{item.description}</span>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ol>
        </section>
    }
    .into_any()
}

fn tool_result_summary(event: &PersistedTaskEvent, output: Option<&Value>) -> Option<String> {
    payload_str_event(event, "result_summary").or_else(|| {
        let output = output?;
        let payload = output.get("structured_payload")?;
        let error_kind = payload.get("error_kind").and_then(Value::as_str)?;

        match payload.get("provider").and_then(Value::as_str) {
            Some("web_markdown") => {
                let host = payload.get("host").and_then(Value::as_str);
                let status_code = payload.get("status_code").and_then(Value::as_i64);

                Some(match error_kind {
                    "anti_bot" => host
                        .map(|host| format!("anti_bot at {host}"))
                        .unwrap_or_else(|| "anti_bot".to_string()),
                    "http_status" => status_code
                        .map(|status| format!("http_status {status}"))
                        .unwrap_or_else(|| "http_status".to_string()),
                    other => host
                        .map(|host| format!("{other} at {host}"))
                        .unwrap_or_else(|| other.to_string()),
                })
            }
            Some("crawl4ai_markdown") => {
                let host = payload.get("host").and_then(Value::as_str);
                let status_code = payload.get("status_code").and_then(Value::as_i64);

                Some(match error_kind {
                    "crawl4ai_http_status" => status_code
                        .map(|code| format!("http_status {code}"))
                        .unwrap_or_else(|| "http_status".to_string()),
                    "crawl4ai_unavailable" => "crawl4ai unavailable".to_string(),
                    "crawl4ai_auth_failed" => "auth_failed".to_string(),
                    "timeout" => host
                        .map(|host| format!("timeout at {host}"))
                        .unwrap_or_else(|| "timeout".to_string()),
                    "dns_failed" => host
                        .map(|host| format!("dns_failed at {host}"))
                        .unwrap_or_else(|| "dns_failed".to_string()),
                    "network" => host
                        .map(|host| format!("network at {host}"))
                        .unwrap_or_else(|| "network".to_string()),
                    other => other.to_string(),
                })
            }
            Some("duckduckgo") => Some(match error_kind {
                "rate_limited" => "rate_limited".to_string(),
                "blocked" => "blocked".to_string(),
                "parser_break" => "parser_break".to_string(),
                "timeout" => "timeout".to_string(),
                other => other.to_string(),
            }),
            _ => None,
        }
    })
}

/// Extract command string from ToolCall + ToolResult pair.
fn command_from_events(
    call: Option<&PersistedTaskEvent>,
    output: Option<&Value>,
) -> Option<String> {
    // 1. ToolCall.command_preview
    call.and_then(|e| payload_str_event(e, "command_preview"))
        .or_else(|| {
            // 2. structured_payload.command from result
            output
                .and_then(|o| o.get("structured_payload"))
                .and_then(|sp| sp.get("command"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        })
        .or_else(|| {
            // 3. ToolCall.input_preview
            call.and_then(|e| payload_str_event(e, "input_preview"))
        })
}

fn todo_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "done",
        "in_progress" => "doing",
        "blocked_on_user" => "blocked",
        "cancelled" => "cancelled",
        _ => "todo",
    }
}

fn todo_status_marker(label: &str) -> &'static str {
    match label {
        "done" => "✓",
        "doing" => "•",
        "blocked" => "!",
        "cancelled" => "×",
        _ => "",
    }
}

// ── Search result model ──────────────────────────────────────────────────

#[derive(Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}
