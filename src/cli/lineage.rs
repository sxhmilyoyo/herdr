//! `--parent` and `--no-parent` for commands that create panes.
//!
//! By default a creating command names the calling pane (`HERDR_PANE_ID`) as
//! the new pane's parent. The field is only sent when the server advertises the
//! `pane_parent` capability, so older servers keep working; an explicit
//! `--parent` on such a server is an error instead of a silent no-op.

use std::fmt;

use clap::{parser::ValueSource, Arg, ArgAction, ArgMatches};

use super::projection::ParseError;

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

/// Appends `--parent <ID>` and `--no-parent` to a creating command's arguments.
///
/// The flags are read back from the matches by `caller_pane_id`, so the
/// `caller_pane_id` projection field declares no input of its own.
pub(super) fn with_parent_flags(mut arguments: Vec<Arg>) -> Vec<Arg> {
    arguments.push(
        Arg::new("parent")
            .long("parent")
            .value_name("ID")
            .action(ArgAction::Set)
            .value_parser(clap::builder::NonEmptyStringValueParser::new())
            .help(
                "Record this pane id or live agent name as the new pane's parent \
                 (default: the calling pane from HERDR_PANE_ID)",
            ),
    );
    arguments.push(
        Arg::new("no-parent")
            .long("no-parent")
            .action(ArgAction::SetTrue)
            .help("Do not record a parent pane"),
    );
    arguments
}

fn last_index(matches: &ArgMatches, id: &str) -> Option<usize> {
    if matches.value_source(id) != Some(ValueSource::CommandLine) {
        return None;
    }
    matches
        .indices_of(id)
        .and_then(|mut indices| indices.next_back())
}

/// The parent choice in `matches`. The last parent flag wins.
fn parent_arg(matches: &ArgMatches) -> ParentArg {
    let explicit = last_index(matches, "parent");
    let detached = last_index(matches, "no-parent");
    // `None` orders below `Some`, so a lone `--no-parent` also detaches.
    if detached > explicit {
        return ParentArg::Detached;
    }
    match explicit.and(matches.get_one::<String>("parent")) {
        Some(value) => ParentArg::Explicit(value.clone()),
        None => ParentArg::Inherit,
    }
}

/// The `caller_pane_id` for a creating request parsed into `matches`.
///
/// A `--parent` failure is marked so the command dispatcher prints its message
/// and exits 1; see `is_parent_error`.
pub(super) fn caller_pane_id(matches: &ArgMatches) -> Result<Option<String>, ParseError> {
    let parent = parent_arg(matches);
    if matches!(&parent, ParentArg::Explicit(value) if value.trim().is_empty()) {
        return Err(ParseError::Usage(
            "--parent requires a pane id or a live agent name".into(),
        ));
    }
    caller_pane_for_request(&parent)
        .map_err(|error| ParseError::Io(std::io::Error::other(ParentError(error))))
}

/// Marks a `--parent` failure. An ordinary I/O error from request building
/// reaches `main` and is printed in debug form; this one is reported as its
/// plain message with exit code 1.
#[derive(Debug)]
struct ParentError(std::io::Error);

impl fmt::Display for ParentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for ParentError {}

pub(super) fn is_parent_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<ParentError>())
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

    fn parent(values: &[&str]) -> Result<ParentArg, clap::Error> {
        let matches = super::super::projection::command("create", with_parent_flags(Vec::new()))
            .no_binary_name(true)
            .try_get_matches_from(values)?;
        Ok(parent_arg(&matches))
    }

    #[test]
    fn parent_flags_default_to_inherit() {
        assert_eq!(parent(&[]).unwrap(), ParentArg::Inherit);
    }

    #[test]
    fn parent_flags_accept_an_explicit_parent_in_both_forms() {
        assert_eq!(
            parent(&["--parent", "w1:p2"]).unwrap(),
            ParentArg::Explicit("w1:p2".into())
        );
        assert_eq!(
            parent(&["--parent=reviewer"]).unwrap(),
            ParentArg::Explicit("reviewer".into())
        );
    }

    #[test]
    fn parent_flags_last_flag_wins() {
        assert_eq!(parent(&["--no-parent"]).unwrap(), ParentArg::Detached);
        assert_eq!(
            parent(&["--parent", "w1:p2", "--no-parent"]).unwrap(),
            ParentArg::Detached
        );
        assert_eq!(
            parent(&["--no-parent", "--parent", "w1:p2"]).unwrap(),
            ParentArg::Explicit("w1:p2".into())
        );
        assert_eq!(
            parent(&["--parent", "first", "--parent", "last"]).unwrap(),
            ParentArg::Explicit("last".into())
        );
    }

    #[test]
    fn parent_flags_reject_missing_empty_or_flag_like_values() {
        assert!(parent(&["--parent"]).is_err());
        assert!(parent(&["--parent", ""]).is_err());
        assert!(parent(&["--parent", "--no-parent"]).is_err());
    }

    #[test]
    fn parent_errors_are_marked_and_keep_their_message() {
        let marked = std::io::Error::other(ParentError(std::io::Error::other("boom")));
        assert!(is_parent_error(&marked));
        assert_eq!(marked.to_string(), "boom");
        assert!(!is_parent_error(&std::io::Error::other("boom")));
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
