//! Detecting background work from the transcript. The lines copy the shape of a real Claude Code
//! 2.1.32x transcript (a real session with four background subagents and a
//! ScheduleWakeup, then a stop). Prompts, replies and tool inputs are dummies.

use decidle::home::background::{self, Pending};
use serde_json::{json, Value};

fn ts(sec: u32) -> String {
    format!("2026-11-18T01:{:02}:{:02}.000Z", 50 + sec / 60, sec % 60)
}

fn human(sec: u32) -> Value {
    json!({"type":"user","isSidechain":false,"promptId":"p1","origin":{"kind":"human"},
           "message":{"role":"user","content":"dummy prompt"},"timestamp":ts(sec)})
}

fn tool_use(name: &str, id: &str, input: Value, sec: u32) -> Value {
    json!({"type":"assistant","isSidechain":false,
           "message":{"role":"assistant","content":[{"type":"tool_use","id":id,"name":name,"input":input}]},
           "timestamp":ts(sec)})
}

fn tool_result(id: &str, result: Value, sec: u32) -> Value {
    json!({"type":"user","isSidechain":false,"promptId":"p1",
           "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":[{"type":"text","text":"dummy"}]}]},
           "toolUseResult":result,"timestamp":ts(sec)})
}

fn agent(id: &str, sec: u32) -> Value {
    tool_use("Agent", id, json!({"description":"d","model":"m","prompt":"dummy","subagent_type":"general-purpose"}), sec)
}

fn launched(id: &str, agent_id: &str, sec: u32) -> Value {
    tool_result(
        id,
        json!({"agentId":agent_id,"canContinueAgent":true,"canReadOutputFile":true,"description":"d","isAsync":true,
               "outputFile":"/tmp/x","prompt":"dummy","resolvedModel":"m","status":"async_launched"}),
        sec,
    )
}

fn wakeup(id: &str, sec: u32) -> Value {
    tool_use("ScheduleWakeup", id, json!({"delaySeconds":1200,"noop":false,"prompt":"dummy","reason":"dummy"}), sec)
}

fn scheduled(id: &str, sec: u32) -> Value {
    tool_result(id, json!({"scheduledFor":1800000000000u64,"clampedDelaySeconds":1200,"wasClamped":false}), sec)
}

fn notification(task: &str, tool_use_id: &str) -> String {
    format!(
        "<task-notification>\n<task-id>{task}</task-id>\n<tool-use-id>{tool_use_id}</tool-use-id>\n<output-file>/tmp/x</output-file>\n<status>completed</status>\n<summary>dummy</summary>\n<note>dummy</note>\n</task-notification>"
    )
}

fn enqueue(content: &str, sec: u32) -> Value {
    json!({"type":"queue-operation","operation":"enqueue","content":content,"sessionId":"s","timestamp":ts(sec)})
}

fn removed(content: &str, sec: u32) -> Value {
    json!({"type":"queue-operation","operation":"remove","reason":"absorbed_mid_turn","commandUuid":"c","deliveryId":"d",
           "content":content,"sessionId":"s","timestamp":ts(sec)})
}

fn queued(prompt: &str, sec: u32) -> Value {
    json!({"type":"attachment","attachment":{"type":"queued_command","commandMode":"task-notification",
           "origin":{"kind":"task-notification","producer":"session-task"},"prompt":prompt,"delivery_id":"d"},
           "timestamp":ts(sec)})
}

/// The subagent's report delivered as a meta user line. It names a different tool use id than
/// the Agent call, so it is not taken as the result.
fn peer(from: &str, sec: u32) -> Value {
    json!({"type":"user","isSidechain":false,"isMeta":true,
           "origin":{"kind":"peer","from":from,"handback":true,"handbackToolUseId":"toolu_other"},
           "message":{"role":"user","content":"dummy report"},"timestamp":ts(sec)})
}

fn system(subtype: &str, sec: u32) -> Value {
    json!({"type":"system","subtype":subtype,"isSidechain":false,"timestamp":ts(sec)})
}

fn scan(lines: &[Value]) -> Pending {
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    background::scan(text.iter().map(String::as_str))
}

fn pending(tasks: usize, wakeup: bool) -> Pending {
    Pending { tasks, wakeup }
}

/// The real session, cut at each of its three stops.
#[test]
fn real_session_shape() {
    let mut lines = vec![human(0)];
    for (k, id) in ["toolu_A", "toolu_B", "toolu_C", "toolu_D"].iter().enumerate() {
        lines.push(agent(id, 10 + k as u32));
    }
    // The results come back in another order than the calls.
    lines.push(launched("toolu_A", "agentA", 20));
    lines.push(launched("toolu_D", "agentD", 20));
    lines.push(launched("toolu_B", "agentB", 20));
    lines.push(launched("toolu_C", "agentC", 20));
    lines.push(tool_use("Bash", "toolu_fg", json!({"command":"dummy","description":"d"}), 25));
    lines.push(tool_result("toolu_fg", json!({"stdout":"","stderr":"","interrupted":false,"isImage":false,"noOutputExpected":false}), 26));
    lines.push(wakeup("toolu_W1", 30));
    lines.push(scheduled("toolu_W1", 31));
    lines.push(system("stop_hook_summary", 34));
    assert_eq!(scan(&lines), pending(4, true), "first stop");
    assert!(scan(&lines).waiting());

    // B reports first and starts a turn; the reports of B and A are absorbed mid-turn.
    let (nb, na) = (notification("agentB", "toolu_B"), notification("agentA", "toolu_A"));
    lines.push(enqueue("<agent-message from=\"agentB\">\ndummy\n</agent-message>", 84));
    lines.push(json!({"type":"queue-operation","operation":"dequeue","sessionId":"s","timestamp":ts(84)}));
    lines.push(peer("agentB", 84));
    lines.push(enqueue(&nb, 84));
    lines.push(enqueue(&na, 89));
    lines.push(removed(&nb, 90));
    lines.push(removed(&na, 90));
    lines.push(queued(&nb, 84));
    lines.push(queued(&na, 89));
    lines.push(wakeup("toolu_W2", 125));
    lines.push(scheduled("toolu_W2", 125));
    lines.push(system("stop_hook_summary", 128));
    assert_eq!(scan(&lines), pending(2, true), "second stop");

    let (nc, nd) = (notification("agentC", "toolu_C"), notification("agentD", "toolu_D"));
    lines.push(enqueue(&nc, 130));
    lines.push(enqueue(&nd, 132));
    lines.push(queued(&nc, 130));
    lines.push(queued(&nd, 132));
    lines.push(system("stop_hook_summary", 368));
    // Every subagent has reported, but the wakeup has not fired.
    assert_eq!(scan(&lines), pending(0, true), "third stop");
    assert!(scan(&lines).waiting());

    lines.push(system("scheduled_task_fire", 1200));
    assert_eq!(scan(&lines), pending(0, false), "after the wakeup fired");
    assert!(!scan(&lines).waiting());
}

#[test]
fn background_bash_waits_until_its_notification() {
    let mut lines = vec![
        human(0),
        tool_use("Bash", "toolu_bg", json!({"command":"dummy","description":"d","run_in_background":true}), 1),
        tool_result(
            "toolu_bg",
            json!({"backgroundCwdHint":"/tmp","backgroundTaskId":"bgtask1","interrupted":false,"isImage":false,
                   "noOutputExpected":false,"stderr":"","stdout":""}),
            2,
        ),
    ];
    assert_eq!(scan(&lines), pending(1, false));
    lines.push(enqueue(&notification("bgtask1", "toolu_bg"), 60));
    assert_eq!(scan(&lines), pending(0, false));
}

#[test]
fn foreground_calls_and_plain_turns_do_not_wait() {
    let lines = vec![
        human(0),
        agent("toolu_sync", 1),
        tool_result("toolu_sync", json!({"status":"completed","agentId":"a","content":[]}), 9),
        system("stop_hook_summary", 10),
    ];
    assert_eq!(scan(&lines), pending(0, false));
}

#[test]
fn wakeup_stopped_by_a_later_call() {
    let lines = vec![
        human(0),
        wakeup("toolu_W", 1),
        scheduled("toolu_W", 2),
        tool_use("ScheduleWakeup", "toolu_S", json!({"stop":true}), 3),
        tool_result("toolu_S", json!({}), 4),
    ];
    assert_eq!(scan(&lines), pending(0, false));
}

#[test]
fn text_that_only_looks_like_a_notification_does_not_count() {
    let quoted = notification("agentA", "toolu_A");
    let lines = vec![
        human(0),
        agent("toolu_A", 1),
        launched("toolu_A", "agentA", 2),
        // The assistant writing the tag is not a result.
        json!({"type":"assistant","message":{"content":[{"type":"text","text":quoted}]},"timestamp":ts(3)}),
        // Neither is a line of a subagent's own conversation.
        json!({"type":"queue-operation","operation":"enqueue","isSidechain":true,"content":quoted,"timestamp":ts(4)}),
        // A scheduledFor on a result that is not a ScheduleWakeup call does not schedule.
        tool_result("toolu_other", json!({"scheduledFor":1}), 5),
    ];
    assert_eq!(scan(&lines), pending(1, false));
}

#[test]
fn reading_the_file_matches_scanning_the_lines() {
    let dir = std::env::temp_dir().join(format!("decidle-bg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.jsonl");
    let lines = [human(0), agent("toolu_A", 1), launched("toolu_A", "agentA", 2)];
    let mut text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    text.push_str("{cut off");
    std::fs::write(&path, text).unwrap();
    assert_eq!(background::pending(&path).unwrap(), pending(1, false));
    assert!(background::pending(&dir.join("missing.jsonl")).is_err());
}

/// TaskStop ends a background start without a task-notification.
#[test]
fn task_stop_ends_a_background_start() {
    let bash_started = |id: &str, task: &str, sec: u32| {
        tool_result(id, json!({"backgroundCwdHint":"/tmp","backgroundTaskId":task,"interrupted":false,"isImage":false,
                               "noOutputExpected":false,"stderr":"","stdout":""}), sec)
    };
    let mut lines = vec![
        human(0),
        tool_use("Bash", "toolu_bg1", json!({"command":"dummy","description":"d","run_in_background":true}), 1),
        bash_started("toolu_bg1", "bgtask2", 2),
        tool_use("Bash", "toolu_bg2", json!({"command":"dummy","description":"d","run_in_background":true}), 3),
        bash_started("toolu_bg2", "bother", 4),
        agent("toolu_A", 5),
        launched("toolu_A", "agentA", 6),
    ];
    assert_eq!(scan(&lines), pending(3, false));
    lines.push(tool_use("TaskStop", "toolu_ts", json!({"task_id":"bgtask2"}), 7));
    lines.push(tool_result("toolu_ts", json!({"message":"dummy","task_id":"bgtask2","task_type":"local_bash","command":"dummy"}), 8));
    assert_eq!(scan(&lines), pending(2, false));
    // A task_id on the result of another tool is not a stop.
    lines.push(tool_result("toolu_other", json!({"task_id":"agentA"}), 9));
    assert_eq!(scan(&lines), pending(2, false));
    lines.push(tool_use("TaskStop", "toolu_ts2", json!({"task_id":"agentA"}), 10));
    lines.push(tool_result("toolu_ts2", json!({"message":"dummy","task_id":"agentA","task_type":"local_agent"}), 11));
    assert_eq!(scan(&lines), pending(1, false));
}
