//! `openroad/build_info`: which build is this client.
//!
//! Evidence collected from a running game is unreadable later unless it names
//! the build it came from — the same screenshot means different things on two
//! commits. The closest answer available to a tool outside the process is the
//! commit of whatever checkout it ran in, which is a hint, not proof. This is
//! the proof: the commit is embedded at compile time by `build.rs`, so the
//! answer comes from the binary itself.
//!
//! An unknown commit is reported as unknown. A source tarball has no git data,
//! and a client that refused to answer at all would be worse than one that
//! says it does not know.

use bevy::prelude::In;
use bevy::remote::BrpResult;
use serde_json::{json, Value};

/// The commit `build.rs` stamped in, or `"unknown"` outside a git checkout.
const BUILD_COMMIT: &str = env!("OPENROAD_BUILD_COMMIT");
/// `"1"` when tracked files differed from the commit at build time.
const BUILD_DIRTY: &str = env!("OPENROAD_BUILD_DIRTY");

/// The payload, built from values rather than read from the environment, so
/// the shape is testable without rebuilding with other stamps.
pub fn build_info_value(commit: &str, dirty: bool, profile: &str) -> Value {
    let known = commit != "unknown" && !commit.is_empty();
    json!({
        // `null` rather than the string "unknown": a reader checking for a
        // commit must not get a value that looks like one.
        "commit": if known { Value::String(commit.to_string()) } else { Value::Null },
        "commit_known": known,
        // Separate from the commit on purpose: a dirty build is *not* the
        // commit it names, and a mark taken against it cannot be reproduced
        // from that commit alone.
        "dirty": dirty,
        "profile": profile,
        "package_version": env!("CARGO_PKG_VERSION"),
    })
}

/// `openroad/build_info`: the commit, whether the tree was dirty, and which
/// profile. Takes no parameters.
pub fn brp_build_info(In(_params): In<Option<Value>>) -> BrpResult {
    Ok(build_info_value(
        BUILD_COMMIT,
        BUILD_DIRTY == "1",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known commit is a string and says so; nothing else in the payload
    /// changes shape.
    #[test]
    fn a_known_commit_is_reported_as_a_string() {
        let value = build_info_value("0f1a037", false, "debug");
        assert_eq!(value["commit"], json!("0f1a037"));
        assert_eq!(value["commit_known"], json!(true));
        assert_eq!(value["dirty"], json!(false));
        assert_eq!(value["profile"], json!("debug"));
    }

    /// The point of the field: a reader looking for a commit must not be
    /// handed the word "unknown", which would pass a string check and then
    /// name a commit that does not exist.
    #[test]
    fn an_unknown_commit_is_null_and_not_the_word_unknown() {
        for raw in ["unknown", ""] {
            let value = build_info_value(raw, false, "release");
            assert_eq!(value["commit"], Value::Null, "raw was {raw:?}");
            assert_eq!(value["commit_known"], json!(false));
            assert!(
                !value.to_string().contains("\"unknown\""),
                "the word must not travel as a commit: {value}"
            );
        }
    }

    /// A dirty build is not the commit it names. Keeping the flag beside the
    /// commit, rather than folding it into the string, lets a reader decide
    /// what to do with it — and stops `commit == "abc-dirty"` comparisons.
    #[test]
    fn a_dirty_build_keeps_its_commit_and_flags_itself() {
        let value = build_info_value("0f1a037", true, "debug");
        assert_eq!(value["commit"], json!("0f1a037"));
        assert_eq!(value["dirty"], json!(true));
        assert_eq!(value["commit_known"], json!(true));
    }

    /// The stamp the build script wrote must be one of the two shapes the
    /// payload knows: a commit, or the literal `unknown`. This is the only
    /// test that looks at the real environment, and it is what catches a
    /// build script that silently stopped stamping.
    #[test]
    fn the_embedded_stamp_is_either_a_commit_or_unknown() {
        assert!(!BUILD_COMMIT.is_empty());
        assert!(
            BUILD_COMMIT == "unknown" || BUILD_COMMIT.chars().all(|c| c.is_ascii_hexdigit()),
            "unexpected stamp {BUILD_COMMIT:?}"
        );
        assert!(BUILD_DIRTY == "0" || BUILD_DIRTY == "1", "{BUILD_DIRTY:?}");
    }
}
