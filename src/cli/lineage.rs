//! `--parent` and `--no-parent` for commands that create panes.
//!
//! By default a creating command names the calling pane (`HERDR_PANE_ID`) as
//! the new pane's parent. The field is only sent when the server advertises the
//! `pane_parent` capability, so older servers keep working; an explicit
//! `--parent` on such a server is an error instead of a silent no-op.

/// How a creating command chooses the new pane's parent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum ParentArg {
    /// The calling pane from `HERDR_PANE_ID`, when available and supported.
    #[default]
    Inherit,
    /// A pane id or live agent name given with `--parent`.
    Explicit(String),
    /// `--no-parent`: record no parent.
    Detached,
}

/// Remove `--parent <ID>`, `--parent=<ID>`, and `--no-parent` from `args`.
///
/// The remaining arguments are returned unchanged so existing parsers keep
/// their contracts. The last parent flag wins.
pub(super) fn take_parent_flags(args: &[String]) -> Result<(Vec<String>, ParentArg), String> {
    let mut remaining = Vec::with_capacity(args.len());
    let mut parent = ParentArg::Inherit;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            remaining.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "--no-parent" {
            parent = ParentArg::Detached;
            index += 1;
            continue;
        }
        let value = if arg == "--parent" {
            let Some(value) = args.get(index + 1) else {
                return Err("missing value for --parent".into());
            };
            index += 2;
            value.as_str()
        } else if let Some(value) = arg.strip_prefix("--parent=") {
            index += 1;
            value
        } else {
            remaining.push(args[index].clone());
            index += 1;
            continue;
        };
        if value.trim().is_empty() || value.starts_with('-') {
            return Err("--parent requires a pane id or a live agent name".into());
        }
        parent = ParentArg::Explicit(value.to_string());
    }
    Ok((remaining, parent))
}

/// The `caller_pane_id` to send for a creating request.
///
/// Implicit attribution degrades silently when the server lacks the
/// capability or cannot be probed; an explicit `--parent` fails loudly.
pub(super) fn caller_pane_for_request(parent: &ParentArg) -> std::io::Result<Option<String>> {
    let (candidate, explicit) = match parent {
        ParentArg::Detached => return Ok(None),
        ParentArg::Explicit(id) => (id.clone(), true),
        ParentArg::Inherit => match super::target::caller_pane_id() {
            Some(id) => (super::normalize_pane_id(&id), false),
            None => return Ok(None),
        },
    };
    match server_supports_pane_parent() {
        Ok(true) if explicit => {
            ensure_parent_exists(&candidate)?;
            Ok(Some(candidate))
        }
        Ok(true) => Ok(Some(candidate)),
        Ok(false) if explicit => Err(std::io::Error::other(
            "--parent needs a Herdr server that advertises the pane_parent capability; \
             update the server or drop --parent",
        )),
        Ok(false) => Ok(None),
        Err(err) if explicit => Err(err),
        Err(_) => Ok(None),
    }
}

/// The server ignores unknown callers, so an explicit `--parent` is checked
/// here to fail loudly on a typo or a pane from another session.
fn ensure_parent_exists(target: &str) -> std::io::Result<()> {
    use crate::api::schema::{AgentTarget, Method, PaneTarget, Request};
    let as_pane = Request {
        id: "cli:lineage:parent-pane".into(),
        method: Method::PaneGet(PaneTarget {
            pane_id: target.to_owned(),
        }),
    };
    if super::send_request(&as_pane).is_ok_and(|value| response_is_success(&value)) {
        return Ok(());
    }
    let as_agent = Request {
        id: "cli:lineage:parent-agent".into(),
        method: Method::AgentGet(AgentTarget {
            target: target.to_owned(),
        }),
    };
    if super::send_request(&as_agent).is_ok_and(|value| response_is_success(&value)) {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "--parent {target}: no pane or live agent with that id on this server"
    )))
}

/// `send_request` returns error responses as ordinary JSON values; only a body
/// without an `error` member means the lookup found the target.
fn response_is_success(value: &serde_json::Value) -> bool {
    value.get("error").is_none() && value.get("result").is_some()
}

fn server_supports_pane_parent() -> std::io::Result<bool> {
    let client = super::target::api_client()?;
    let status = super::target::server_status(&client).map_err(super::api_client_error_to_io)?;
    Ok(status
        .capabilities
        .is_some_and(|capabilities| capabilities.pane_parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn take_parent_flags_defaults_to_inherit_and_keeps_other_args() {
        let (remaining, parent) =
            take_parent_flags(&args(&["--direction", "right", "--no-focus"])).unwrap();
        assert_eq!(remaining, args(&["--direction", "right", "--no-focus"]));
        assert_eq!(parent, ParentArg::Inherit);
    }

    #[test]
    fn take_parent_flags_extracts_explicit_parent_in_both_forms() {
        let (remaining, parent) =
            take_parent_flags(&args(&["--parent", "w1:p2", "--direction", "down"])).unwrap();
        assert_eq!(remaining, args(&["--direction", "down"]));
        assert_eq!(parent, ParentArg::Explicit("w1:p2".into()));

        let (remaining, parent) =
            take_parent_flags(&args(&["--cwd", "/tmp", "--parent=reviewer"])).unwrap();
        assert_eq!(remaining, args(&["--cwd", "/tmp"]));
        assert_eq!(parent, ParentArg::Explicit("reviewer".into()));
    }

    #[test]
    fn take_parent_flags_no_parent_detaches_and_last_flag_wins() {
        let (remaining, parent) =
            take_parent_flags(&args(&["--parent", "w1:p2", "--no-parent"])).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(parent, ParentArg::Detached);

        let (_, parent) = take_parent_flags(&args(&["--no-parent", "--parent", "w1:p2"])).unwrap();
        assert_eq!(parent, ParentArg::Explicit("w1:p2".into()));
    }

    #[test]
    fn take_parent_flags_rejects_missing_or_flag_like_values() {
        assert!(take_parent_flags(&args(&["--parent"])).is_err());
        assert!(take_parent_flags(&args(&["--parent", "--direction"])).is_err());
        assert!(take_parent_flags(&args(&["--parent="])).is_err());
    }

    #[test]
    fn take_parent_flags_stops_at_double_dash() {
        let (remaining, parent) =
            take_parent_flags(&args(&["--no-parent", "--", "--parent", "x"])).unwrap();
        assert_eq!(remaining, args(&["--", "--parent", "x"]));
        assert_eq!(parent, ParentArg::Detached);
    }

    #[test]
    fn response_success_requires_a_result_and_no_error_member() {
        assert!(response_is_success(&serde_json::json!({
            "id": "x",
            "result": {"type": "pane_info"}
        })));
        assert!(!response_is_success(&serde_json::json!({
            "id": "x",
            "error": {"code": "pane_not_found", "message": "pane not found"}
        })));
        assert!(!response_is_success(&serde_json::json!({"id": "x"})));
    }

    #[test]
    fn detached_parent_never_probes_the_server() {
        assert_eq!(caller_pane_for_request(&ParentArg::Detached).unwrap(), None);
    }
}
