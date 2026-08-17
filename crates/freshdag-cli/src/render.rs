//! Human-readable rendering of a certificate.
//!
//! This is `freshdag why` in embryo: the user must be able to answer
//! "why does it say that?" without opening JSON. Invariant #6 says
//! every decision is explainable to a user; this module is where the
//! machine-checkable explanation becomes an English one.
//!
//! # Two rules this module lives under
//!
//! 1. **Prose lives here, not in core.** ADR 0006 made
//!    `ValidityReason.reason` a closed [`ReasonCode`] enum precisely so
//!    that consumers stop substring-matching free text. `as_wire_str`
//!    is the wire form; the sentence a human reads is a presentation
//!    concern and belongs in the presentation layer. [`prose`] is an
//!    exhaustive match, so adding a code to the vocabulary fails to
//!    compile here until someone writes the sentence.
//! 2. **Never branch on `detail`.** Certificate contract §The `detail`
//!    field: `detail` is non-normative, and no consumer may parse it or
//!    treat its absence as meaningful. It is printed verbatim as
//!    supplementary context under a reason whose meaning was already
//!    fully determined by its code.

use std::fmt::Write as _;

use freshdag_core::certificate::{Certificate, CoverageEntry};
use freshdag_core::dependency::{Dependency, ReasonCode, TrustClass, ValidityStatus};
use freshdag_core::ir::ProducerRole;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::exit::Exit;

/// Label column width, so the header block lines up. Wide enough that
/// the longest label (`Certificate`) is still followed by a separator —
/// the golden test's normalizer splits on it.
const LABEL: usize = 13;

/// The sentence a human reads for a reason code.
///
/// Pre-wrapped at roughly 68 columns; [`indent`] adds the leading
/// whitespace. Hard-wrapping rather than wrapping at render time keeps
/// the output byte-identical across terminal widths, which is what
/// makes the golden test meaningful.
///
/// The match is exhaustive on purpose. A new `ReasonCode` is a contract
/// change (ADR 0006); this is the compile error that tells its author
/// the CLI owes the user a sentence.
fn prose(code: ReasonCode) -> &'static str {
    match code {
        ReasonCode::Drift => {
            "This dependency changed. A probe re-read it and got a fingerprint\n\
             that does not match the one recorded when the artifact was built."
        }
        ReasonCode::ProbeUnknown => {
            "A probe ran for this dependency but could not decide whether it\n\
             changed, so it contributed no evidence either way. Nothing here\n\
             is treated as proof the dependency is unchanged."
        }
        ReasonCode::NoProbeAvailable => {
            "No probe answered for this dependency, so it was never checked:\n\
             either nothing is registered for its scheme, or the probe that\n\
             recorded it has since been removed. What was not checked cannot\n\
             be called fresh."
        }
        ReasonCode::TrustClassHeuristicCapsAtLikelyValid => {
            "This dependency matched, but only on a heuristic signal (an mtime,\n\
             a weak validator) that can be wrong. That is enough to say\n\
             likely-valid and not enough to say valid."
        }
        ReasonCode::TrustClassVolatileCapsAtLikelyValid => {
            "This dependency has no trustworthy freshness signal at all. A probe\n\
             re-read it and agreed it is unchanged, but that signal cannot\n\
             support more than likely-valid. It is also inside its declared\n\
             TTL."
        }
        ReasonCode::VolatileWithinTtlUnprobed => {
            "NOTHING CHECKED THIS DEPENDENCY. It is being treated as fresh\n\
             purely because whoever recorded it declared a lifetime, and that\n\
             lifetime has not elapsed. No probe is registered for its scheme,\n\
             or the one that recorded it has since been removed. A declared\n\
             lifetime is a promise, not an observation, so --accept-likely-valid\n\
             will not treat this as reusable."
        }
        ReasonCode::DependencyChangedDuringComputation => {
            "This dependency changed WHILE the agent was reading it: the same\n\
             input was observed twice during one computation with two different\n\
             contents. Nothing records which of the two the agent actually\n\
             used, so the recorded fingerprint is not a statement about what\n\
             this artifact was built from. Re-run the computation to get an\n\
             artifact whose inputs held still."
        }
        ReasonCode::TtlExpired => {
            "This dependency's declared TTL elapsed without it being\n\
             re-observed, so nothing is currently known about its state."
        }
        ReasonCode::ProbeTrustDemoted => {
            "The signal this dependency's trust class rested on has gone away.\n\
             The recorded trust class was demoted and the dependency must be\n\
             re-observed before it can be trusted again."
        }
        // Names the condition, never an operating system. The sentence
        // is static and one certificate away from every platform this
        // binary runs on: `freshdag-observer` ships a real Linux
        // backend, so "v0 ships no observer backend for macOS" was
        // simply false there — and it was printed for every
        // coverage-deficit reason, including `uncovered-effect-kinds`
        // deficits with no `bash` or `task` anywhere near them. A
        // sentence that is false on some runs of the product is worse
        // than a vague one, and the honest-coverage story is the thing
        // this project is selling. So it points at the coverage list
        // the renderer has already printed, which is concrete, on
        // screen, and true everywhere.
        ReasonCode::CoverageDeficit => {
            "Something happened during this computation that no registered\n\
             producer claims to observe, so its silence cannot be read as\n\
             \"nothing happened\". Two shapes reach here: an effect kind that\n\
             nothing in the coverage list below declares it emits, or a\n\
             `bash` or `task` command run with no observer-role producer\n\
             covering the filesystem. Only a sub-agent-layer observer sees\n\
             what a subprocess reads and writes, and where no observer\n\
             backend covers this platform FreshDAG reports the gap rather\n\
             than papering over it. It will not call an artifact valid on\n\
             the strength of effects nobody watched, so this is reported as\n\
             unknown."
        }
        ReasonCode::ProducerMissingFromCoverage => {
            "An event here came from a producer that never registered a\n\
             coverage manifest. Without one, FreshDAG cannot tell what that\n\
             producer would have reported from what it never watches, so its\n\
             silence carries no information."
        }
        ReasonCode::NoDependenciesObserved => {
            "This computation produced an artifact without a single observed\n\
             dependency. Absence of evidence is not evidence of freshness."
        }
        ReasonCode::UnprovenDependency => {
            "Something this computation read could not be fingerprinted, so it\n\
             is an input whose state is unknown. It is not listed among the\n\
             dependencies below, because recording it as one would mean\n\
             inventing evidence about it — but it exists, and it was not\n\
             checked."
        }
        ReasonCode::RecipeIdentityUnavailable => {
            "This computation carries no recipe identity, so nothing can tie\n\
             the artifact to a reproducible way of making it. Its dependencies\n\
             may all have verified; what is missing is the identity of the\n\
             computation they belong to. Some runtimes cannot supply one at\n\
             all, in which case this caps every artifact they produce."
        }
    }
}

/// The closing line: what the status means and which exit code it maps
/// to.
fn verdict(status: ValidityStatus, exit: Exit) -> String {
    let code = exit.code();
    match status {
        ValidityStatus::Valid => format!(
            "Verdict: valid — every dependency was verified unchanged at exact\n\
             or versioned trust. Safe to reuse. (exit {code})"
        ),
        ValidityStatus::LikelyValid if exit == Exit::Valid => format!(
            "Verdict: likely-valid — nothing drifted, but at least one\n\
             dependency could only be verified heuristically, so this is not a\n\
             proof. Treated as reusable because --accept-likely-valid was\n\
             passed. (exit {code})"
        ),
        ValidityStatus::LikelyValid => format!(
            "Verdict: likely-valid — nothing drifted, but at least one\n\
             dependency could only be verified heuristically, so this is not a\n\
             proof. FreshDAG will not report it as reusable; exit {code} is\n\
             deliberate. Pass --accept-likely-valid to accept it anyway."
        ),
        ValidityStatus::Stale => format!(
            "Verdict: stale — at least one dependency changed since this\n\
             artifact was built. Recompute it. (exit {code})"
        ),
        ValidityStatus::Unknown => format!(
            "Verdict: unknown — FreshDAG could not prove this artifact is\n\
             still valid. Unknown is not fresh: do not reuse it on the\n\
             strength of this check. (exit {code})"
        ),
    }
}

/// Render the whole report.
pub fn certificate(cert: &Certificate, exit: Exit) -> String {
    let mut out = String::new();
    header(&mut out, cert);
    reasons(&mut out, cert);
    dependencies(&mut out, &cert.depends_on);
    coverage(&mut out, &cert.observation_coverage);
    out.push('\n');
    out.push_str(&verdict(cert.status.value, exit));
    out.push('\n');
    out
}

fn header(out: &mut String, cert: &Certificate) {
    field(out, "Artifact", &cert.artifact.id.0);
    if let Some(path) = &cert.artifact.path {
        field(out, "Path", path);
    }
    field(out, "Status", status_word(cert.status.value));
    field(out, "Checked", &timestamp(cert.status.checked));
    if let Some(recipe) = &cert.produced_by.recipe {
        field(out, "Recipe", recipe);
    }
    field(out, "Certificate", &cert.cert_id.to_string());
}

fn field(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "{label:<LABEL$}{value}");
}

fn reasons(out: &mut String, cert: &Certificate) {
    let reasons = &cert.status.reasons;
    if reasons.is_empty() {
        out.push_str("\nReasons: none. A valid certificate carries no reasons.\n");
        return;
    }
    let n = reasons.len();
    let plural = if n == 1 { "reason" } else { "reasons" };
    let _ = writeln!(
        out,
        "\nWhy ({n} {plural}, in the certificate's contractual order):\n"
    );
    for (i, reason) in reasons.iter().enumerate() {
        // Scope comes from the code, not from whether `dependency_key`
        // happens to be empty: the code is the normative field.
        let scope = if reason.reason.is_artifact_scoped() {
            "(this artifact as a whole)".to_string()
        } else {
            reason.dependency_key.clone()
        };
        let _ = writeln!(out, "  {}. {}  —  {scope}", i + 1, reason.reason);
        out.push_str(&indent(prose(reason.reason), 5));
        // `detail` is non-normative: shown, never interpreted.
        if let Some(detail) = &reason.detail {
            let _ = writeln!(out, "     context: {detail}");
        }
        if i + 1 < n {
            out.push('\n');
        }
    }
}

fn dependencies(out: &mut String, deps: &[Dependency]) {
    if deps.is_empty() {
        out.push_str("\nDependencies: none were observed.\n");
        return;
    }
    let _ = writeln!(out, "\nDependencies ({}):", deps.len());
    for dep in deps {
        let _ = writeln!(out, "  {:<10} {}", trust_word(dep.trust_class), dep.key);
    }
}

fn coverage(out: &mut String, entries: &[CoverageEntry]) {
    if entries.is_empty() {
        out.push_str("\nObserved by: nothing declared coverage for this computation.\n");
        return;
    }
    out.push_str("\nObserved by:\n");
    for entry in entries {
        let _ = writeln!(
            out,
            "  {} {} ({})",
            entry.producer,
            entry.version,
            role_word(entry.role)
        );
        for limitation in &entry.known_limitations {
            let _ = writeln!(out, "      known limitation: {limitation}");
        }
    }
}

/// Indent every line of a pre-wrapped block.
fn indent(block: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    let mut out = String::with_capacity(block.len() + block.lines().count() * spaces + 1);
    for line in block.lines() {
        let _ = writeln!(out, "{pad}{line}");
    }
    out
}

/// RFC 3339, or a marker. Never a silent empty string: a missing
/// timestamp should be visible.
fn timestamp(ts: OffsetDateTime) -> String {
    ts.format(&Rfc3339)
        .unwrap_or_else(|_| "<unformattable timestamp>".to_string())
}

/// The status word, spelled exactly as the wire form so prose and JSON
/// cannot drift apart.
fn status_word(status: ValidityStatus) -> &'static str {
    match status {
        ValidityStatus::Valid => "valid",
        ValidityStatus::LikelyValid => "likely-valid",
        ValidityStatus::Stale => "stale",
        ValidityStatus::Unknown => "unknown",
    }
}

fn trust_word(trust: TrustClass) -> &'static str {
    match trust {
        TrustClass::Exact => "exact",
        TrustClass::Versioned => "versioned",
        TrustClass::Heuristic => "heuristic",
        TrustClass::Volatile => "volatile",
    }
}

fn role_word(role: ProducerRole) -> &'static str {
    match role {
        ProducerRole::Adapter => "adapter",
        ProducerRole::Observer => "observer",
        ProducerRole::Probe => "probe",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_CODES: &[ReasonCode] = &[
        ReasonCode::Drift,
        ReasonCode::ProbeUnknown,
        ReasonCode::NoProbeAvailable,
        ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
        ReasonCode::TrustClassVolatileCapsAtLikelyValid,
        ReasonCode::TtlExpired,
        ReasonCode::ProbeTrustDemoted,
        ReasonCode::CoverageDeficit,
        ReasonCode::ProducerMissingFromCoverage,
        ReasonCode::NoDependenciesObserved,
    ];

    #[test]
    fn every_reason_code_has_distinct_prose() {
        let mut seen: Vec<&str> = Vec::new();
        for code in ALL_CODES {
            let text = prose(*code);
            assert!(!text.is_empty(), "{code} has no prose");
            assert!(
                !seen.contains(&text),
                "{code} reuses another code's sentence"
            );
            seen.push(text);
        }
    }

    #[test]
    fn prose_lines_stay_within_the_wrap_width() {
        for code in ALL_CODES {
            for line in prose(*code).lines() {
                assert!(
                    line.chars().count() + 5 <= 79,
                    "prose for {code} exceeds the wrap width once indented: {line:?}"
                );
            }
        }
    }

    #[test]
    fn prose_never_states_the_wire_code() {
        // The wire form is already printed on the heading line; prose
        // that just re-spells it explains nothing.
        for code in ALL_CODES {
            assert!(
                !prose(*code).contains(code.as_wire_str()),
                "prose for {code} merely restates the wire code"
            );
        }
    }

    #[test]
    fn the_coverage_deficit_sentence_names_the_condition() {
        let text = prose(ReasonCode::CoverageDeficit);
        for needle in [
            "bash",
            "observer",
            "sub-agent-layer",
            "coverage list",
            "unknown",
        ] {
            assert!(
                text.contains(needle),
                "the coverage-deficit sentence must mention `{needle}`; \
                 it is the product's honest-coverage story and must stay \
                 concrete about what was not observed"
            );
        }
    }

    /// The sentence is static and the product is not single-platform:
    /// `freshdag-observer` ships a real Linux backend, so a hardcoded
    /// "no observer backend for macOS" was false on Linux — and it was
    /// printed for every coverage deficit, including effect-kind
    /// deficits with no `bash` or `task` involved. Name the condition,
    /// not an OS.
    #[test]
    fn no_prose_hardcodes_an_operating_system() {
        for code in ALL_CODES {
            let text = prose(*code);
            for os in ["macOS", "Linux", "Windows", "darwin", "linux", "win32"] {
                assert!(
                    !text.contains(os),
                    "prose for {code} names `{os}`; a static sentence cannot \
                     know which platform it is printed on"
                );
            }
        }
    }

    #[test]
    fn status_words_match_the_certificate_wire_form() {
        for (status, word) in [
            (ValidityStatus::Valid, "valid"),
            (ValidityStatus::LikelyValid, "likely-valid"),
            (ValidityStatus::Stale, "stale"),
            (ValidityStatus::Unknown, "unknown"),
        ] {
            assert_eq!(status_word(status), word);
            let json = serde_json::to_string(&status).expect("status serializes");
            assert_eq!(json, format!("\"{word}\""));
        }
    }

    #[test]
    fn indent_pads_every_line() {
        assert_eq!(indent("a\nb", 2), "  a\n  b\n");
    }
}
