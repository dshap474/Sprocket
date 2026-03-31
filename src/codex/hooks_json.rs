use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn merge_hooks_json(
    existing: Option<Value>,
    generated_groups: &[(String, Value)],
    marker: &str,
) -> Result<Value> {
    let mut root = existing.unwrap_or_else(|| json!({}));
    let hooks = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json root must be an object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let hook_map = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json `hooks` must be an object"))?;

    for (event_name, generated_group) in generated_groups {
        let groups = hook_map
            .entry(event_name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let array = groups
            .as_array_mut()
            .ok_or_else(|| anyhow!("hooks.json event must be an array"))?;
        array.retain(|group| !group_contains_marker(group, marker));
        array.push(generated_group.clone());
    }

    Ok(root)
}

pub fn group_contains_marker(group: &Value, marker: &str) -> bool {
    let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    hooks.iter().any(|hook| {
        hook.get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains(marker))
    })
}
