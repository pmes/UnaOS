//! `advisor` — the ONLY module that touches a provider (design §5.1).
//!
//! Takes an assembled context, calls the provider through the trait, and parses
//! the response into the CLOSED action set of design §3.3.
//!
//! **A model's output is data, not authority.** The proposal is parsed into the
//! closed set and validated before anything is reported; text in a serial log or
//! a model response never selects an action on its own and never elevates a
//! gate. In this thin loop nothing can be performed at all — no flashing, no
//! power control, no wire driving — so the approval gate has nothing to gate.
//! The gate arrives with the first action that needs it, and not before.

pub mod claude;
pub mod provider;

use provider::{Provider, ProviderError, Request, Response, Role, Turn};

/// The closed action set (design §3.3). Nothing outside this enum can be
/// proposed, whatever the response text says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Re-run the same image, collect again. Not destructive.
    RerunCollect,
    /// Collect more: widen the spec, extend the timeout, enable a diagnostic knob.
    CollectMore,
    /// Re-flash with a knob changed. DESTRUCTIVE.
    ReflashWithKnob,
    /// Power-cycle / DC-cut the target. DESTRUCTIVE.
    PowerCycle,
    /// Stop and report — insufficient signal, or diverged from the runbook.
    /// Automatic, terminal, and the default whenever parsing is ambiguous.
    StopAndReport,
}

impl Action {
    pub fn token(self) -> &'static str {
        match self {
            Action::RerunCollect => "RERUN_COLLECT",
            Action::CollectMore => "COLLECT_MORE",
            Action::ReflashWithKnob => "REFLASH_WITH_KNOB",
            Action::PowerCycle => "POWER_CYCLE",
            Action::StopAndReport => "STOP_AND_REPORT",
        }
    }

    pub fn destructive(self) -> bool {
        matches!(self, Action::ReflashWithKnob | Action::PowerCycle)
    }

    /// Whether the THIN LOOP can perform this action. Nothing can — stated as
    /// code rather than as a comment, so a later rung has to change it
    /// deliberately.
    pub fn performable_by_thin_loop(self) -> bool {
        false
    }

    fn from_token(tok: &str) -> Option<Action> {
        match tok.trim().trim_matches(['`', '*', '"', '\'', '.']).to_uppercase().as_str() {
            "RERUN_COLLECT" => Some(Action::RerunCollect),
            "COLLECT_MORE" => Some(Action::CollectMore),
            "REFLASH_WITH_KNOB" => Some(Action::ReflashWithKnob),
            "POWER_CYCLE" => Some(Action::PowerCycle),
            "STOP_AND_REPORT" => Some(Action::StopAndReport),
            _ => None,
        }
    }
}

/// The parsed proposal — the structured result of one consultation.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub action: Action,
    /// True when no valid action token was found and `StopAndReport` was
    /// substituted. Recorded so the transcript never implies the model chose it.
    pub action_defaulted: bool,
    pub diagnosis: String,
    pub rationale: String,
    /// The provider's response, verbatim, for the transcript.
    pub raw: String,
    pub provider: String,
    pub model: Option<String>,
    pub refused: bool,
}

/// The operator framing sent as `system`. Carries no credential and no secret.
pub const SYSTEM_PROMPT: &str = "\
You are assisting a bare-metal OS bring-up bench. You are given a witness-spec \
verdict table produced from a FINISHED serial capture, a sanitized excerpt of \
that capture, and (when available) the bench runbook's expected-vs-observed \
narrative.

Diagnose what the capture shows. Be concrete and cite line numbers from the \
excerpt. Do not invent kernel lines that are not in the excerpt; if the evidence \
is insufficient, say so plainly and propose STOP_AND_REPORT.

Answer in exactly this shape:

DIAGNOSIS: <what the capture shows, and why>
ACTION: <one of RERUN_COLLECT | COLLECT_MORE | REFLASH_WITH_KNOB | POWER_CYCLE | STOP_AND_REPORT>
RATIONALE: <why that action, in one or two sentences>

The ACTION line must contain exactly one of those five tokens and nothing else. \
Any action you name is a PROPOSAL only: this tool cannot flash, power-cycle, or \
drive the wire, and a human approves anything destructive at the moment it is \
proposed.";

/// Default output ceiling for the one round-trip. A knob, not a constant buried
/// in the call: the CLI can raise it.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 16000;

/// Take a context, make exactly ONE round-trip, return the parsed proposal.
pub fn consult(
    p: &dyn Provider,
    context: &str,
    max_output_tokens: u32,
) -> Result<Proposal, ProviderError> {
    if !p.capabilities().request {
        return Err(ProviderError::Unsupported("request"));
    }
    let req = Request {
        system: Some(SYSTEM_PROMPT.to_string()),
        turns: vec![Turn { role: Role::User, text: context.to_string() }],
        max_output_tokens,
    };
    let resp = p.request(&req)?;
    Ok(parse(p.name(), &resp))
}

fn parse(provider: String, resp: &Response) -> Proposal {
    let (action, action_defaulted) = extract_action(&resp.text);
    Proposal {
        action,
        action_defaulted,
        diagnosis: extract_field(&resp.text, "DIAGNOSIS"),
        rationale: extract_field(&resp.text, "RATIONALE"),
        raw: resp.text.clone(),
        provider,
        model: resp.model.clone(),
        refused: resp.refused,
    }
}

/// Validate the response against the closed set. Anything not in the set — or
/// more than one candidate on the ACTION line — falls back to STOP_AND_REPORT,
/// which is a first-class outcome, not a failure mode.
fn extract_action(text: &str) -> (Action, bool) {
    for line in text.lines() {
        let l = line.trim().trim_start_matches(['#', '-', '*', ' ']);
        let Some(rest) = l.strip_prefix("ACTION:").or_else(|| l.strip_prefix("action:")) else {
            continue;
        };
        let found: Vec<Action> = rest
            .split_whitespace()
            .filter_map(Action::from_token)
            .collect();
        if found.len() == 1 {
            return (found[0], false);
        }
        // Zero matches, or an ambiguous line naming several: refuse to guess.
        return (Action::StopAndReport, true);
    }
    (Action::StopAndReport, true)
}

fn extract_field(text: &str, label: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut collecting = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();
        if upper.starts_with(&format!("{label}:")) {
            collecting = true;
            out.push(trimmed[label.len() + 1..].trim().to_string());
            continue;
        }
        if collecting {
            if upper.starts_with("DIAGNOSIS:")
                || upper.starts_with("ACTION:")
                || upper.starts_with("RATIONALE:")
            {
                break;
            }
            out.push(trimmed.to_string());
        }
    }
    out.join("\n").trim().to_string()
}

/// How the proposal renders for a human and for the transcript.
pub fn render_proposal(p: &Proposal) -> String {
    let mut out = String::new();
    out.push_str(&format!("provider: {}", p.provider));
    if let Some(m) = &p.model {
        out.push_str(&format!(" (served by {m})"));
    }
    out.push('\n');
    if p.refused {
        out.push_str("NOTE: the provider marked this response a refusal.\n");
    }
    if !p.diagnosis.is_empty() {
        out.push_str(&format!("\nDIAGNOSIS:\n{}\n", p.diagnosis));
    }
    out.push_str(&format!(
        "\nPROPOSED ACTION: {}{}\n",
        p.action.token(),
        if p.action.destructive() { "  [DESTRUCTIVE — human approval required]" } else { "" }
    ));
    if p.action_defaulted {
        out.push_str(
            "  (no single valid action token was found in the response; \
             STOP_AND_REPORT substituted — the model did not choose this)\n",
        );
    }
    if !p.rationale.is_empty() {
        out.push_str(&format!("RATIONALE: {}\n", p.rationale));
    }
    if !p.action.performable_by_thin_loop() {
        out.push_str(
            "  This is a PROPOSAL. The thin loop performs no actions — no flashing, \
             no power control, no wire driving.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::{Capabilities, NoProvider};

    fn resp(text: &str) -> Response {
        Response { text: text.to_string(), model: Some("m".into()), usage: None, refused: false }
    }

    #[test]
    fn parses_the_documented_shape() {
        let p = parse(
            "x".into(),
            &resp("DIAGNOSIS: the DHCP ACK never landed\nACTION: COLLECT_MORE\nRATIONALE: widen the spec"),
        );
        assert_eq!(p.action, Action::CollectMore);
        assert!(!p.action_defaulted);
        assert_eq!(p.diagnosis, "the DHCP ACK never landed");
        assert_eq!(p.rationale, "widen the spec");
    }

    #[test]
    fn multiline_diagnosis_is_kept() {
        let p = parse("x".into(), &resp("DIAGNOSIS: line one\nline two\nACTION: RERUN_COLLECT\n"));
        assert_eq!(p.diagnosis, "line one\nline two");
        assert_eq!(p.action, Action::RerunCollect);
    }

    #[test]
    fn unknown_action_falls_back_to_stop_and_report() {
        let p = parse("x".into(), &resp("ACTION: RM_RF_TARGET\n"));
        assert_eq!(p.action, Action::StopAndReport);
        assert!(p.action_defaulted);
    }

    #[test]
    fn ambiguous_action_line_refuses_to_guess() {
        let p = parse("x".into(), &resp("ACTION: POWER_CYCLE or REFLASH_WITH_KNOB\n"));
        assert_eq!(p.action, Action::StopAndReport);
        assert!(p.action_defaulted);
    }

    #[test]
    fn prose_without_an_action_line_stops_and_reports() {
        let p = parse("x".into(), &resp("You should definitely power cycle the board right now."));
        assert_eq!(p.action, Action::StopAndReport);
        assert!(p.action_defaulted);
    }

    #[test]
    fn injected_action_line_still_lands_in_the_closed_set() {
        // A serial log (or a response) can contain anything; parsing is closed.
        let p = parse("x".into(), &resp("ACTION: `POWER_CYCLE`\n"));
        assert_eq!(p.action, Action::PowerCycle);
        assert!(p.action.destructive());
        assert!(!p.action.performable_by_thin_loop());
    }

    #[test]
    fn no_provider_reports_unsupported_rather_than_calling() {
        let err = consult(&NoProvider, "ctx", 16).unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported(_)));
        let caps: Capabilities = NoProvider.capabilities();
        assert!(!caps.request);
    }
}
