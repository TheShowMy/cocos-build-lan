use chrono::{DateTime, Local};
use reqwest::Client;
use serde_json::{Value, json};

use crate::models::FeishuBotConfig;

#[derive(Debug, Clone)]
pub struct NotificationResult {
    pub bot_name: String,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct FailedTaskNotification {
    pub task_name: String,
    pub project_name: String,
    pub branch: String,
    pub failed_at: String,
    pub error: String,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FinishedTaskSummary {
    pub task_name: String,
    pub project_name: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BuildFinishedNotification {
    pub started_at: DateTime<Local>,
    pub finished_at: DateTime<Local>,
    pub success_tasks: Vec<FinishedTaskSummary>,
    pub failed_tasks: Vec<FinishedTaskSummary>,
}

pub async fn send_build_started(
    bots: &[FeishuBotConfig],
    task_names: &[String],
    started_at: DateTime<Local>,
) -> Vec<NotificationResult> {
    let card = json!({
        "config": { "wide_screen_mode": true },
        "header": build_header("blue", "打包开始"),
        "elements": [
            build_fields(&[
                markdown_field("任务数量", &task_names.len().to_string(), true),
                markdown_field("开始时间", &started_at.format("%Y-%m-%d %H:%M:%S").to_string(), true),
            ]),
            json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("**任务列表**\n{}", task_names.iter().map(|name| format!("• {name}")).collect::<Vec<_>>().join("\n"))
                }
            }),
            build_note("当前通知由自定义机器人 webhook 发送。")
        ]
    });

    send_card_to_bots(bots, card).await
}

pub async fn send_task_failed(
    bots: &[FeishuBotConfig],
    data: &FailedTaskNotification,
) -> Vec<NotificationResult> {
    let card = json!({
        "config": { "wide_screen_mode": true },
        "header": build_header("red", "任务失败"),
        "elements": [
            build_fields(&[
                markdown_field("任务名", &data.task_name, true),
                markdown_field("项目", &data.project_name, true),
                markdown_field("分支", &data.branch, true),
                markdown_field("失败时间", &data.failed_at, true),
            ]),
            json!({ "tag": "hr" }),
            json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("**错误摘要**\n{}", truncate_text(&data.error, 400))
                }
            }),
            json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("**日志路径**\n{}", data.log_path.as_deref().unwrap_or("无"))
                }
            }),
            build_note("单任务失败通知，不影响其余任务继续执行。")
        ]
    });

    send_card_to_bots(bots, card).await
}

pub async fn send_build_finished(
    bots: &[FeishuBotConfig],
    data: &BuildFinishedNotification,
) -> Vec<NotificationResult> {
    let duration = data.finished_at - data.started_at;
    let success_list = if data.success_tasks.is_empty() {
        "无".to_string()
    } else {
        data.success_tasks
            .iter()
            .map(|task| format!("• {} / {}", task.project_name, task.task_name))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let failed_list = if data.failed_tasks.is_empty() {
        "无".to_string()
    } else {
        data.failed_tasks
            .iter()
            .map(|task| {
                let summary = task.error.as_deref().unwrap_or("无错误信息");
                format!(
                    "• {} / {}：{}",
                    task.project_name,
                    task.task_name,
                    truncate_text(summary, 120)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let card = json!({
        "config": { "wide_screen_mode": true },
        "header": build_header(if data.failed_tasks.is_empty() { "green" } else { "orange" }, "打包完成"),
        "elements": [
            build_fields(&[
                markdown_field("总任务数", &(data.success_tasks.len() + data.failed_tasks.len()).to_string(), true),
                markdown_field("成功数", &data.success_tasks.len().to_string(), true),
                markdown_field("失败数", &data.failed_tasks.len().to_string(), true),
                markdown_field("耗时", &format!("{} 秒", duration.num_seconds().max(0)), true),
            ]),
            json!({ "tag": "hr" }),
            json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("**成功任务**\n{}", success_list)
                }
            }),
            json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("**失败任务**\n{}", failed_list)
                }
            }),
            build_note(&format!(
                "开始：{}｜结束：{}",
                data.started_at.format("%Y-%m-%d %H:%M:%S"),
                data.finished_at.format("%Y-%m-%d %H:%M:%S"),
            ))
        ]
    });

    send_card_to_bots(bots, card).await
}

async fn send_card_to_bots(bots: &[FeishuBotConfig], card: Value) -> Vec<NotificationResult> {
    if bots.is_empty() {
        return vec![NotificationResult {
            bot_name: "无".to_string(),
            success: true,
            detail: "未配置飞书机器人，已跳过通知".to_string(),
        }];
    }

    let client = Client::new();
    let mut results = Vec::with_capacity(bots.len());
    for bot in bots {
        let payload = json!({
            "msg_type": "interactive",
            "card": card,
        });

        let response = client
            .post(&bot.api_key)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                results.push(NotificationResult {
                    bot_name: bot.name.clone(),
                    success: status.is_success(),
                    detail: if status.is_success() {
                        format!("HTTP {}", status.as_u16())
                    } else {
                        format!("HTTP {} {}", status.as_u16(), truncate_text(&body, 200))
                    },
                });
            }
            Err(error) => {
                results.push(NotificationResult {
                    bot_name: bot.name.clone(),
                    success: false,
                    detail: truncate_text(&error.to_string(), 200),
                });
            }
        }
    }

    results
}

fn build_header(template: &str, title: &str) -> Value {
    json!({
        "template": template,
        "title": {
            "tag": "plain_text",
            "content": title,
        }
    })
}

fn build_fields(fields: &[Value]) -> Value {
    json!({
        "tag": "div",
        "fields": fields,
    })
}

fn markdown_field(label: &str, value: &str, is_short: bool) -> Value {
    json!({
        "is_short": is_short,
        "text": {
            "tag": "lark_md",
            "content": format!("**{}**\n{}", label, value),
        }
    })
}

fn build_note(text: &str) -> Value {
    json!({
        "tag": "note",
        "elements": [
            {
                "tag": "plain_text",
                "content": text,
            }
        ]
    })
}

fn truncate_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let truncated = text.chars().take(limit).collect::<String>();
    format!("{truncated}...")
}
