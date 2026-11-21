use std::io::Read;

pub(crate) const CLEAR_SOURCE: &str = "clear";
const HOOK_INPUT_BYTES: usize = 1 << 20;

/// Whether `muster msg read --from-hook` should read this session's messages.
pub fn is_main_agent(input: &mut impl Read) -> Result<bool, String> {
    let mut bytes = Vec::new();
    input.take(HOOK_INPUT_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|error| {
        format!("could not read hook input ({error}), so messages were left unread.")
    })?;
    if bytes.len() > HOOK_INPUT_BYTES {
        return Err(format!(
            "hook input exceeds {HOOK_INPUT_BYTES} bytes, so messages were left unread."
        ));
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(true);
    }
    let event: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!("hook input is not valid JSON ({error}), so messages were left unread.")
    })?;
    let event = event
        .as_object()
        .ok_or("hook input must be a JSON object, so messages were left unread.")?;
    Ok(event.get("agent_id").is_none_or(|id| id.is_null() || id.as_str() == Some("")))
}
