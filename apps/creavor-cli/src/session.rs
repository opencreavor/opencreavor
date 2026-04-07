/// Generate a session ID in the format: `<runtime>:<short-uuid>:<timestamp>`
///
/// Example: `claude:a1b2c3d4:20260407T1234`
pub fn generate_session_id(runtime_name: &str) -> String {
    let short_uuid = uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("unknown")
        .to_string();

    let now = time::OffsetDateTime::now_utc();
    let fmt = time::format_description::parse("[year][month][day]T[hour][minute]").unwrap();
    let timestamp = now.format(&fmt).unwrap_or_else(|_| "unknown".to_string());

    format!("{runtime_name}:{short_uuid}:{timestamp}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_has_correct_format() {
        let id = generate_session_id("claude");
        let parts: Vec<&str> = id.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "claude");
        assert_eq!(parts[1].len(), 8);
        assert!(parts[2].contains('T'));
    }
}
