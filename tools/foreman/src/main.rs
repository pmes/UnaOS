//! `foreman` CLI — a thin `main` over the four modules.
//!
//!   foreman --log <LOG> --spec <SPEC> [--provider <claude|none>] [--transcript <FILE>]
//!
//! Exit codes follow mbench's contract exactly, so the two are interchangeable
//! at a bench gate: 0 PASS | 1 FAIL | 2 usage/spec error | 3 TRUNCATED.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use foreman::advisor::{self, provider::Provider};
use foreman::context::Budgets;
use foreman::transcript::{FileTranscript, NullTranscript, Transcript};
use foreman::verdict::{self, RC_ERROR};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ProviderKind {
    /// No provider. The verdict table still prints — the deterministic half of
    /// the tool never depends on the AI half.
    None,
    /// The Claude connector, credentialed from the host environment only.
    Claude,
}

#[derive(Parser, Debug)]
#[command(
    name = "foreman",
    about = "Replay a FINISHED serial capture against a witness spec, assemble a bounded \
             diagnostic context, and take one provider round-trip to a printed diagnosis.",
    long_about = None,
    after_help = "exit codes: 0 PASS | 1 FAIL (a genuine regression) | 2 usage/spec error | \
                  3 TRUNCATED — the capture stopped before the run finished, so the result is \
                  INCONCLUSIVE.\n\n\
                  No device is opened, no log is followed, nothing is injected, flashed, or \
                  power-cycled. Credentials come from the host environment only \
                  (ANTHROPIC_API_KEY / ANTHROPIC_AUTH_TOKEN) and are never written to the \
                  transcript."
)]
struct Cli {
    /// A FINISHED serial log (a bridge capture, or a QEMU target/serial*.log).
    #[arg(long, value_name = "LOG")]
    log: PathBuf,

    /// The witness spec (unaos/scripts/specs/*.spec).
    #[arg(long, value_name = "FILE")]
    spec: PathBuf,

    /// Which provider to consult. `none` runs the deterministic half only.
    #[arg(long, value_enum, default_value_t = ProviderKind::None)]
    provider: ProviderKind,

    /// Model id for the provider. Omit to use the connector's default.
    #[arg(long, value_name = "ID")]
    model: Option<String>,

    /// Append the assembled context, the response, and the parsed action here.
    #[arg(long, value_name = "FILE")]
    transcript: Option<PathBuf>,

    /// Bench runbook to excerpt for expected-vs-observed (design §3.2).
    #[arg(long, value_name = "FILE")]
    runbook: Option<PathBuf>,

    /// Budget: trailing capture lines included in the context.
    #[arg(long, default_value_t = 120)]
    tail_lines: usize,

    /// Budget: lines of context either side of a FORBID hit / last REQUIRE.
    #[arg(long, default_value_t = 12)]
    window: usize,

    /// Budget: max runbook lines excerpted.
    #[arg(long, default_value_t = 120)]
    runbook_lines: usize,

    /// Budget: absolute ceiling on the assembled context, in bytes.
    #[arg(long, default_value_t = 65536)]
    max_context_bytes: usize,

    /// Budget: max output tokens for the single provider round-trip.
    #[arg(long, default_value_t = advisor::DEFAULT_MAX_OUTPUT_TOKENS)]
    max_output_tokens: u32,

    /// Print the assembled context to stdout as well as the transcript.
    #[arg(long)]
    show_context: bool,

    /// Suppress the per-hit FORBID lines above the table.
    #[arg(long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("foreman: {e}");
            ExitCode::from(RC_ERROR as u8)
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<i32> {
    // 1. read a FINISHED serial log ------------------------------------------
    let capture = capture_or_explain(&cli.log)?;

    // 2. run the witness-spec verdicts ---------------------------------------
    // Preflight FIRST: compile every pattern and name every offender, so an
    // unsupported dialect stops the run with an actionable report instead of the
    // regex crate's line-less complaint mid-evaluation. Silent on a valid spec.
    verdict::preflight_spec(&cli.spec).map_err(|r| anyhow::anyhow!("{r}"))?;
    let directives = verdict::parse_spec(&cli.spec).map_err(|e| anyhow::anyhow!("spec error: {e}"))?;
    let ev = verdict::evaluate(directives, &capture, &cli.spec);

    if !cli.quiet {
        // mbench prints the first hit of each FORBID above the table.
        for d in ev.directives.iter().filter(|d| d.kind == verdict::Kind::Forbid && d.hits > 0) {
            println!(
                "  {} FORBID hit @ line {}: {}",
                verdict::glyph::FAIL,
                d.first_lineno.unwrap_or(0),
                d.first_text.as_deref().unwrap_or("")
            );
        }
    }
    let table = verdict::render_table(&ev);
    print!("{table}");
    let rc = ev.verdict().rc();

    // 3. assemble the bounded diagnostic context -----------------------------
    let runbook = match &cli.runbook {
        Some(p) => Some(std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?),
        None => None,
    };
    let budgets = Budgets {
        tail_lines: cli.tail_lines,
        window: cli.window,
        runbook_lines: cli.runbook_lines,
        max_bytes: cli.max_context_bytes,
    };
    let ctx = foreman::context::assemble(&ev, &capture, runbook.as_deref(), budgets);
    let ctx_text = ctx.render();
    if cli.show_context {
        println!("\n{ctx_text}");
    }

    let mut sink: Box<dyn Transcript> = match &cli.transcript {
        Some(p) => Box::new(FileTranscript::open(p)?),
        None => Box::new(NullTranscript),
    };
    sink.section(
        "RUN",
        &format!(
            "log: {}\nspec: {}\nverdict: {} (rc {rc})\nprovider: {:?}",
            cli.log.display(),
            cli.spec.display(),
            ev.verdict().label(),
            cli.provider
        ),
    )?;
    // The assembled context is written to the transcript BEFORE it is sent:
    // what the model saw is reconstructable (design §3.2).
    sink.section("ASSEMBLED CONTEXT (verbatim, written before any send)", &ctx_text)?;

    // 4. one provider round-trip ---------------------------------------------
    if cli.provider == ProviderKind::None {
        println!("\nforeman: no provider configured (--provider=none) — verdict table only.");
        sink.section("PROVIDER", "none — the AI half was not consulted")?;
        return Ok(rc);
    }

    let connector = match advisor::claude::ClaudeConnector::from_env(cli.model.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            // A build with no provider configured is a VALID build: report and
            // keep the deterministic verdict, do not fail the gate on it.
            println!("\nforeman: no provider available: {e}");
            sink.section("PROVIDER", &format!("unavailable: {e}"))?;
            return Ok(rc);
        }
    };
    sink.section(
        "REQUEST (credentials excluded by construction)",
        &format!(
            "provider: {}\nsystem prompt:\n{}\nuser turn: the assembled context above ({} bytes)\nmax_output_tokens: {}",
            connector.name(),
            advisor::SYSTEM_PROMPT,
            ctx_text.len(),
            cli.max_output_tokens
        ),
    )?;

    match advisor::consult(&connector, &ctx_text, cli.max_output_tokens) {
        Ok(proposal) => {
            sink.section("RESPONSE (verbatim)", &proposal.raw)?;
            let rendered = advisor::render_proposal(&proposal);
            println!("\n═══════════ FOREMAN DIAGNOSIS ═══════════");
            print!("{rendered}");
            sink.section("PARSED PROPOSAL (validated against the closed action set)", &rendered)?;
        }
        Err(e) => {
            println!("\nforeman: provider round-trip failed: {e}");
            sink.section("PROVIDER ERROR", &e.to_string())?;
        }
    }

    // The verdict is the deterministic half's; the AI half never changes it.
    Ok(rc)
}

fn capture_or_explain(path: &std::path::Path) -> anyhow::Result<foreman::capture::Capture> {
    let cap = foreman::capture::read(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    if cap.lines.is_empty() {
        anyhow::bail!(
            "{}: the capture is EMPTY — that is neither a pass, a regression, nor a truncation. \
             Re-capture before reading anything into it.",
            path.display()
        );
    }
    Ok(cap)
}
