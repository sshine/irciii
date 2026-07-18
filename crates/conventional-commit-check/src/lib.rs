//! Validation of [conventional commits](https://www.conventionalcommits.org/) subject lines.
//!
//! The subject grammar checked here is:
//!
//! ```text
//! <type>[(<scope>)][!]: <description>
//! ```

use thiserror::Error;

/// The default set of allowed commit types (the well-known conventional-commits set).
pub const DEFAULT_TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// Policy for validating a commit subject line.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Allowed commit types, e.g. `feat`, `fix`.
    pub types: Vec<String>,
    /// Whether a scope (`feat(scope): ...`) is mandatory.
    pub require_scope: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            types: DEFAULT_TYPES.iter().map(|s| s.to_string()).collect(),
            require_scope: false,
        }
    }
}

/// Why a commit subject failed validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Violation {
    /// The message is empty (or only comments).
    #[error("commit message is empty")]
    Empty,
    /// The subject has no `type: description` shape at all.
    #[error("subject does not match '<type>[(scope)][!]: <description>'")]
    Malformed,
    /// The type is not in the allowed set.
    #[error("unknown commit type '{0}' (allowed: {1})")]
    UnknownType(String, String),
    /// A scope is required but missing.
    #[error("scope is required: '<type>(<scope>): <description>'")]
    MissingScope,
    /// The scope is present but empty (`feat(): ...`).
    #[error("scope must not be empty")]
    EmptyScope,
    /// Nothing after the colon.
    #[error("description must not be empty")]
    EmptyDescription,
}

/// Validate a full commit message against a policy.
///
/// Only the subject (first non-comment line) is checked; git commit comments
/// (`#`-prefixed lines) are ignored, so this works on `COMMIT_EDITMSG` files.
pub fn validate(message: &str, policy: &Policy) -> Result<(), Violation> {
    let subject = message
        .lines()
        .map(str::trim_end)
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .ok_or(Violation::Empty)?;

    // Split "<head>: <description>".
    let (head, description) = subject.split_once(':').ok_or(Violation::Malformed)?;
    if description.trim().is_empty() {
        return Err(Violation::EmptyDescription);
    }

    // Strip a trailing breaking-change marker from the head.
    let head = head.strip_suffix('!').unwrap_or(head);

    // Split "<type>(<scope>)" from the head.
    let (ty, scope) = match head.split_once('(') {
        Some((ty, rest)) => {
            let scope = rest.strip_suffix(')').ok_or(Violation::Malformed)?;
            (ty, Some(scope))
        }
        None => (head, None),
    };

    if ty.is_empty() || !ty.chars().all(|c| c.is_ascii_lowercase()) {
        return Err(Violation::Malformed);
    }
    if !policy.types.iter().any(|t| t == ty) {
        return Err(Violation::UnknownType(
            ty.to_string(),
            policy.types.join(", "),
        ));
    }
    match scope {
        Some("") => return Err(Violation::EmptyScope),
        Some(_) => {}
        None if policy.require_scope => return Err(Violation::MissingScope),
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default() -> Policy {
        Policy::default()
    }

    fn strict() -> Policy {
        Policy {
            require_scope: true,
            ..Policy::default()
        }
    }

    #[test]
    fn accepts_plain_type() {
        assert_eq!(validate("feat: add thing", &default()), Ok(()));
    }

    #[test]
    fn accepts_scoped_type() {
        assert_eq!(validate("fix(ui): clamp scroll", &default()), Ok(()));
    }

    #[test]
    fn accepts_breaking_marker() {
        assert_eq!(
            validate("feat(proto)!: new wire format", &default()),
            Ok(())
        );
        assert_eq!(validate("feat!: new wire format", &default()), Ok(()));
    }

    #[test]
    fn ignores_comments_and_body() {
        let msg = "# please enter the commit message\n\nfeat: subject after comments\n\nbody\n";
        assert_eq!(validate(msg, &default()), Ok(()));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(validate("", &default()), Err(Violation::Empty));
        assert_eq!(
            validate("# only comments\n", &default()),
            Err(Violation::Empty)
        );
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(
            validate("bad message", &default()),
            Err(Violation::Malformed)
        );
        assert_eq!(
            validate("Feat: capitalized type", &default()),
            Err(Violation::Malformed)
        );
        assert_eq!(
            validate("feat(unclosed: paren", &default()),
            Err(Violation::Malformed)
        );
    }

    #[test]
    fn rejects_unknown_type() {
        assert!(matches!(
            validate("feet: typo", &default()),
            Err(Violation::UnknownType(..))
        ));
    }

    #[test]
    fn restricted_type_set() {
        let policy = Policy {
            types: vec!["feat".into(), "fix".into()],
            require_scope: false,
        };
        assert_eq!(validate("feat: ok", &policy), Ok(()));
        assert!(matches!(
            validate("chore: not allowed", &policy),
            Err(Violation::UnknownType(..))
        ));
    }

    #[test]
    fn scope_policy() {
        assert_eq!(
            validate("feat: no scope", &strict()),
            Err(Violation::MissingScope)
        );
        assert_eq!(validate("feat(ui): scoped", &strict()), Ok(()));
        assert_eq!(
            validate("feat(): empty", &default()),
            Err(Violation::EmptyScope)
        );
    }

    #[test]
    fn rejects_empty_description() {
        assert_eq!(
            validate("feat: ", &default()),
            Err(Violation::EmptyDescription)
        );
        assert_eq!(
            validate("feat:", &default()),
            Err(Violation::EmptyDescription)
        );
    }
}
