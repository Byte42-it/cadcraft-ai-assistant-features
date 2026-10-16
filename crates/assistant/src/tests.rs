//! The agent loop against a headless session, with scripted model replies.

use cadcraft_mcp::{Backend, Headless};
use serde_json::{Value, json};

use crate::{Agent, Config, Error, Event, MAX_STEPS, Observer, Scripted, Turn, text_reply, tool_reply};

fn count(b: &mut Headless, kind: Option<&str>) -> u64 {
    let params = kind.map_or(json!({}), |k| json!({"type": k}));
    let r = b.call("engine.execute", json!({"command": "entities", "params": params})).unwrap();
    r["count"].as_u64().unwrap()
}

/// Records events and answers confirmations with `yes`.
#[derive(Default)]
struct Log {
    yes: bool,
    asked: Vec<String>,
    events: Vec<String>,
}

impl Observer for Log {
    fn confirm(&mut self, name: &str, _: &Value) -> bool {
        self.asked.push(name.to_string());
        self.yes
    }
    fn event(&mut self, e: Event<'_>) {
        self.events.push(match e {
            Event::Text(t) => format!("text {t}"),
            Event::ToolCall { name, .. } => format!("call {name}"),
            Event::ToolResult { name, ok, .. } => format!("result {name} {ok}"),
            Event::Declined { name } => format!("declined {name}"),
        });
    }
}

#[test]
fn draws_a_plate_with_holes_and_reports() {
    let mut b = Headless::default();
    let mut p = Scripted::new([
        tool_reply(&[("a", "command_line", json!({"text": "rectang 0,0 100,60"}))]),
        tool_reply(&[
            ("b", "execute", json!({"command": "circle", "params": {"center": [10, 10], "radius": 4}})),
            ("c", "execute", json!({"command": "circle", "params": {"center": [90, 10], "radius": 4}})),
        ]),
        tool_reply(&[("d", "query_entities", json!({"type": "Circle"}))]),
        text_reply("Drew a 100×60 plate with two holes."),
    ]);
    let mut agent = Agent::new(Config::new("test-model"));
    let mut log = Log::default();
    let turn = agent.send(&mut p, &mut b, "draw a plate with two holes", &mut log).unwrap();
    assert_eq!(turn.reply, "Drew a 100×60 plate with two holes.");
    assert_eq!((turn.steps, turn.tool_calls), (4, 4));
    assert_eq!(count(&mut b, Some("Circle")), 2);
    assert_eq!(count(&mut b, None), 3);
    assert!(log.events.iter().all(|e| !e.ends_with("false")), "{:?}", log.events);
    // Every request carries the model, the history so far and the command-level tools only.
    let last = p.requests.last().unwrap();
    assert_eq!(last["model"], "test-model");
    let tools: Vec<&str> = last["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&"command_line") && tools.contains(&"execute"));
    assert!(!tools.contains(&"render") && !tools.contains(&"ui_click"));
    // Each tool call is answered by a tool message with its id.
    let msgs = agent.messages();
    assert_eq!(msgs[0]["role"], "system");
    let tool_ids: Vec<&str> = msgs.iter().filter(|m| m["role"] == "tool").map(|m| m["tool_call_id"].as_str().unwrap()).collect();
    assert_eq!(tool_ids, ["a", "b", "c", "d"]);
    assert!(msgs.iter().any(|m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("\"count\": 2")));
}

#[test]
fn the_conversation_carries_over_to_the_next_turn() {
    let mut b = Headless::default();
    let mut p = Scripted::new([text_reply("Hi."), text_reply("Still here.")]);
    let mut agent = Agent::new(Config::new("m"));
    agent.send(&mut p, &mut b, "hello", &mut ()).unwrap();
    agent.send(&mut p, &mut b, "again", &mut ()).unwrap();
    let roles: Vec<&str> = p.requests[1]["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
    agent.reset();
    assert_eq!(agent.messages().len(), 1);
}

#[test]
fn tools_that_replace_the_drawing_or_write_files_need_confirmation() {
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
    });
    let dir = std::env::temp_dir().join(format!("cadcraft-assistant-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.dxf");
    let path_s = path.to_string_lossy().to_string();
    let script = || {
        Scripted::new([
            tool_reply(&[("a", "command_line", json!({"text": "circle 0,0 5"}))]),
            tool_reply(&[("b", "save", json!({"path": path_s}))]),
            text_reply("done"),
        ])
    };

    // Declined: nothing is written and the model is told not to retry.
    let mut b = Headless::default();
    let mut log = Log::default();
    let mut p = script();
    Agent::new(Config::new("m")).send(&mut p, &mut b, "draw and save", &mut log).unwrap();
    assert_eq!(log.asked, ["save"]);
    assert!(log.events.contains(&"declined save".to_string()));
    assert!(!path.exists());
    let told = p.requests[2]["messages"].as_array().unwrap().iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("declined")));
    assert!(told);

    // The default observer declines too.
    let mut p = script();
    Agent::new(Config::new("m")).send(&mut p, &mut Headless::default(), "draw and save", &mut ()).unwrap();
    assert!(!path.exists());

    // Confirmed: the file is written.
    let mut log = Log { yes: true, ..Log::default() };
    let mut p = script();
    Agent::new(Config::new("m")).send(&mut p, &mut Headless::default(), "draw and save", &mut log).unwrap();
    assert!(path.exists(), "{:?}", log.events);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hostile_model_replies_never_panic() {
    let huge = "x".repeat(100_000);
    let many: Vec<(String, &str, Value)> = (0..50).map(|i| (format!("c{i}"), "cancel", json!({}))).collect();
    let many: Vec<(&str, &str, Value)> = many.iter().map(|(id, n, a)| (id.as_str(), *n, a.clone())).collect();
    let replies = vec![
        json!({"choices": [{"message": {"role": "assistant", "tool_calls": [
            {"id": "1", "function": {"name": "execute", "arguments": "{not json"}},
            {"id": "2", "function": {"name": "rm_rf", "arguments": "{}"}},
            {"function": {"name": "command_line", "arguments": {"text": "circle 0,0 5"}}},
            {"id": "4", "function": {"name": "execute", "arguments": "[1,2]"}},
            {"id": "5", "function": {"name": "query_entities", "arguments": "{\"limit\": -3}"}},
            {"id": "6", "function": {"name": "command_line", "arguments": json!({"text": huge}).to_string()}},
            {"id": "7", "function": {}},
            {"id": "8"},
            7,
            null,
        ]}}]}),
        tool_reply(&many),
        json!({"choices": [{"message": {"content": 42}}]}),
    ];
    let mut b = Headless::default();
    let mut p = Scripted::new(replies);
    let mut agent = Agent::new(Config::new("m"));
    let turn = agent.send(&mut p, &mut b, "go", &mut ()).unwrap();
    assert_eq!(turn.reply, "");
    // The well-formed call ran despite its neighbours.
    assert_eq!(count(&mut b, Some("Circle")), 1);
    // At most 16 calls per reply; every recorded call has its answer.
    let msgs = agent.messages();
    let asked: usize = msgs.iter().filter_map(|m| m["tool_calls"].as_array()).map(Vec::len).sum();
    let answered = msgs.iter().filter(|m| m["role"] == "tool").count();
    assert_eq!(asked, answered);
    assert!(answered <= 32);
    // Long results are cut.
    assert!(msgs.iter().all(|m| m["content"].as_str().is_none_or(|c| c.chars().count() < 17_000)));

    for bad in [json!({}), json!([]), json!({"choices": []}), json!({"choices": [{"message": null}]}), json!("text"), Value::Null] {
        let r = Agent::new(Config::new("m")).send(&mut Scripted::new([bad.clone()]), &mut Headless::default(), "go", &mut ());
        assert!(matches!(r, Err(Error::BadResponse(_))), "{bad}: {r:?}");
    }
    let r = Agent::new(Config::new("m")).send(
        &mut Scripted::new([json!({"error": {"message": "rate limited"}})]),
        &mut Headless::default(),
        "go",
        &mut (),
    );
    assert_eq!(r, Err(Error::Provider("rate limited".into())));
    let r = Agent::new(Config::new("m")).send(&mut Scripted::default(), &mut Headless::default(), "go", &mut ());
    assert!(matches!(r, Err(Error::Provider(_))));
    let r = Agent::new(Config::new("m")).send(&mut Scripted::default(), &mut Headless::default(), "  \n", &mut ());
    assert_eq!(r, Err(Error::EmptyPrompt));
}

#[test]
fn a_model_that_never_stops_calling_tools_is_stopped() {
    let endless = (0..100).map(|i| tool_reply(&[("x", "list_commands", json!({"filter": format!("z{i}")}))]));
    let mut p = Scripted::new(endless);
    let mut cfg = Config::new("m");
    cfg.max_steps = 1000;
    let r = Agent::new(cfg).send(&mut p, &mut Headless::default(), "loop", &mut ());
    assert_eq!(r, Err(Error::StepLimit { steps: MAX_STEPS }));
    // The tool-calling steps, then one wrap-up call that the model wasted on another tool call.
    assert_eq!(p.requests.len(), MAX_STEPS + 1);
}

#[test]
fn out_of_steps_the_model_gets_one_call_without_tools_to_say_what_it_did() {
    let mut p = Scripted::new([
        tool_reply(&[("a", "list_commands", json!({"filter": "zz"}))]),
        tool_reply(&[("b", "list_commands", json!({"filter": "zz"}))]),
        text_reply("Ho elencato i comandi; non ho disegnato nulla."),
        text_reply("Ciao."),
    ]);
    let mut cfg = Config::new("m");
    cfg.max_steps = 2;
    let mut agent = Agent::new(cfg);
    let turn = agent.send(&mut p, &mut Headless::default(), "loop", &mut ()).unwrap();
    assert_eq!(turn, Turn { reply: "Ho elencato i comandi; non ho disegnato nulla.".into(), steps: 3, tool_calls: 2, stopped: true });
    // The wrap-up request offers no tools; the nudge stays out of the history, the answer goes in.
    assert!(p.requests[2].get("tools").is_none());
    assert!(agent.messages().iter().all(|m| m["content"].as_str().is_none_or(|c| !c.contains("steps allowed"))));
    assert_eq!(agent.messages().last().unwrap()["content"], "Ho elencato i comandi; non ho disegnato nulla.");
    // The conversation carries on.
    let turn = agent.send(&mut p, &mut Headless::default(), "ciao", &mut ()).unwrap();
    assert_eq!((turn.reply.as_str(), turn.stopped), ("Ciao.", false));
}

#[test]
fn old_turns_are_forgotten_in_whole() {
    let mut b = Headless::default();
    let mut agent = Agent::new(Config::new("m"));
    for i in 0..150 {
        let mut p = Scripted::new([tool_reply(&[("t", "list_commands", json!({"filter": "zzz"}))]), text_reply(&format!("turn {i}"))]);
        agent.send(&mut p, &mut b, &format!("prompt {i}"), &mut ()).unwrap();
    }
    let msgs = agent.messages();
    assert!(msgs.len() <= 404, "{}", msgs.len());
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs.last().unwrap()["content"], "turn 149");
}
