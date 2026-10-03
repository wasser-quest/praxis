//! A bounded representation of one decision, lowered to the normal gateway
//! dispatcher. Workflow mappings are trusted; operands carry no authority.
use super::{
    action_contracts,
    llm::provider::{FunctionCall, ToolCall},
    task_control,
};
use crate::{db::Database, plugins::PluginRegistry};
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};

const MAX_IR_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn valid_mapping(op: &str, target: &str) -> bool {
    op.len() == 1
        && op.as_bytes()[0].is_ascii_uppercase()
        && (matches!(target, "inspect_file" | "agent_complete" | "agent_next" | "agent_back")
            || target.split_once('/').is_some_and(|(plugin, tool)| {
                crate::plugins::contracts::identifier(plugin)
                    && crate::plugins::contracts::identifier(tool)
            }))
}

// serde_json::Value normally accepts duplicate keys. Reject ambiguous operands
// and keep the IR v1 payload flat, like the supported capability schema subset.
struct Operands(Map<String, Value>);
impl<'de> Deserialize<'de> for Operands {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Operands;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("one flat JSON object with unique operand keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Operands, M::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if values.len() >= 32
                        || key == "_output"
                        || values.contains_key(&key)
                        || !(value.is_string() || value.is_boolean() || value.is_number())
                    {
                        return Err(serde::de::Error::custom("Invalid or duplicate IR operand"));
                    }
                    values.insert(key, value);
                }
                Ok(Operands(values))
            }
        }
        deserializer.deserialize_map(Object)
    }
}

pub(crate) fn parse(text: &str) -> anyhow::Result<(String, Value)> {
    anyhow::ensure!(
        text.len() <= MAX_IR_BYTES && !text.contains(['\n', '\r', '\0']),
        "IR must be one bounded instruction; encode newlines inside JSON strings"
    );
    let mut parts = text.trim().splitn(3, ' ');
    anyhow::ensure!(
        parts.next() == Some("1"),
        "Unsupported Decision IR version; use 1 OPCODE {{operands}}"
    );
    let op = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing IR opcode"))?;
    anyhow::ensure!(
        op.len() == 1 && op.as_bytes()[0].is_ascii_uppercase(),
        "IR opcode must be A..Z"
    );
    let args = match parts.next() {
        Some(payload) => Value::Object(serde_json::from_str::<Operands>(payload)?.0),
        None => Value::Object(Map::new()),
    };
    Ok((op.into(), args))
}

pub(crate) fn resolve(
    db: &Database,
    user: &str,
    call: &ToolCall,
    plugins: &PluginRegistry,
) -> anyhow::Result<ToolCall> {
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("IR requires an active gateway task"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    let args: Value = serde_json::from_str(&call.function.arguments)?;
    let args = crate::tools::tool_output::execution_args(&args)?;
    anyhow::ensure!(
        args.as_object()
            .is_some_and(|obj| obj.len() == 1 && obj.contains_key("ir")),
        "Supply only ir (and optional outer _output)"
    );
    let (op, operands) = parse(
        args["ir"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("IR must be a string"))?,
    )?;
    let ctx = db.load_context(user)?;
    let mappings = action_contracts::decision_ir_mapping_for(user, ctx.active_state.as_deref().unwrap_or(""))?;
    let target = mappings
        .get(&op)
        .ok_or_else(|| anyhow::anyhow!("Opcode {op} is not declared by this workflow"))?;
    let tool = super::workflow_preflight::target(db, plugins, super::prompt::workflow_name(&ctx), ctx.active_state.as_deref().unwrap_or(""), target)?;
    let definitions = crate::tools::registry::build_tool_definitions_for_user(
        &ctx.settings,
        Some(&plugins.tool_definitions()),
        Some(db),
        user,
    );
    for name in ["execute_decision", tool] {
        anyhow::ensure!(
            definitions.iter().any(|d| d.function.name == name)
                && crate::tools::discovery::enabled(db, plugins, name),
            "IR tool {name} is disabled or unavailable in the current state"
        );
    }
    super::agent_loop::validate_tool_params(tool, &operands, &definitions)
        .map_err(anyhow::Error::msg)?;
    if tool == "agent_complete" {
        anyhow::ensure!(
            operands.as_object().is_some_and(|obj| obj.is_empty()),
            "Completion has no IR operands"
        );
    }
    Ok(ToolCall {
        id: call.id.clone(),
        function: FunctionCall {
            name: tool.into(),
            arguments: operands.to_string(),
        },
    })
}

pub fn definition() -> crate::db::tools::Tool {
    crate::db::tools::Tool {
        name:"execute_decision".into(),
        description:Some("Execute one Praxis Decision IR v1 instruction: 1 OPCODE {flat JSON operands}, or 1 OPCODE for no operands. Use only the active workflow's [DECISION IR] opcode table. Same state permissions, contracts, rollback and receipts as normal tools; each instruction consumes one tool call. No batches, shell or model-supplied mappings. Requires an owned gateway task and pinned workflow.".into()),
        parameters:serde_json::json!({"type":"object","properties":{"ir":{"type":"string","minLength":3,"maxLength":MAX_IR_BYTES}},"required":["ir"],"additionalProperties":false}),
        is_enabled:true,
    }
}

pub(crate) fn instructions(user: &str, state: &str) -> String {
    let Ok(mapping) = action_contracts::decision_ir_mapping_for(user, state) else {
        return String::new();
    };
    let mut entries: Vec<_> = mapping
        .into_iter()
        .map(|(op, target)| format!("{op}={target}"))
        .collect();
    entries.sort();
    format!("\n[DECISION IR]\nexecute_decision accepts {{\"ir\":\"1 OPCODE {{JSON operands}}\"}}. One instruction per call. Omit operands only for no-input actions. Pinned opcode table: {}. Targets must also be enabled and allowed in the current state. Normal capability calls remain available. Receipts and guards are identical. Source edits require inspect_file's expected_sha256 and replacement content; verification commands belong to the author contract, never the instruction.\n", entries.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decision_ir_v1_parses_one_opcode_and_json_operands() {
        let (op, args) =
            parse("1 M {\"path\":\"src/λ.rs\",\"content\":\"line\\nquote \\\"ok\\\"\"}").unwrap();
        assert_eq!(op, "M");
        assert_eq!(args["path"], "src/λ.rs");
        assert_eq!(args["content"], "line\nquote \"ok\"");
        assert_eq!(parse("1 C").unwrap(), ("C".into(), json!({})));
    }

    #[test]
    fn decision_ir_rejects_batches_unknown_versions_and_ambiguous_payloads() {
        for text in [
            "",
            "T {}",
            "2 T {}",
            "1 tests {}",
            "1 t {}",
            "1 T []",
            "1 T null",
            "1 T {}; 1 C",
            "1 T {}\n1 C",
            "1 T {\"_output\":{}}",
            "1 T {\"scope\":\"workspace\",\"scope\":\"other\"}",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert!(parse(&format!(
            "1 M {{\"content\":\"{}\"}}",
            "x".repeat(2 * 1024 * 1024)
        ))
        .is_err());
    }

    #[test]
    fn decision_ir_workflow_mapping_is_strict_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let sm = crate::sm::parse(
            "[state work]\n[decision_ir]\nR = inspect_file\nT = native/tests\nC = agent_complete",
        )
        .unwrap();
        assert_eq!(sm.decision_ir["T"], "native/tests");
        let user = "ir-policy";
        let _task = super::super::task_control::begin(user).unwrap();
        super::super::action_contracts::bind(user, "ir", &sm, dir.path()).unwrap();
        let mut changed = sm.clone();
        changed
            .decision_ir
            .insert("T".into(), "native/other".into());
        assert!(super::super::action_contracts::bind(user, "ir", &changed, dir.path()).is_err());
        for entry in [
            "T = execute_terminal",
            "t = native/tests",
            "T = native/tests\nT = native/tests",
            "T = native/../tests",
            "T = execute_decision",
        ] {
            assert!(crate::sm::parse(&format!("[state work]\n[decision_ir]\n{entry}")).is_err());
        }
    }
}
