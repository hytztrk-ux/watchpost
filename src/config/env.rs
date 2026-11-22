//! `${ENV_VAR}` substitution inside string values of the configuration.
//!
//! Supported forms:
//!
//! * `${NAME}`          – replaced by the value of `NAME`; an error if unset.
//! * `${NAME:-default}` – replaced by `NAME`, or by `default` if unset or empty.
//! * `$${`              – a literal `${` (escape).
//!
//! Substitution happens on parsed string values (not on the raw file), so
//! comments are never touched and a value can never break TOML quoting.

use super::ConfigError;

/// Lookup function used to resolve variables. Abstracted so tests do not have
/// to modify the process environment.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Replace every reference in `input`. `path` is used only for error messages.
pub fn substitute(input: &str, path: &str, env: EnvLookup<'_>) -> Result<String, ConfigError> {
    if !input.contains('$') {
        return Ok(input.to_owned());
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        if let Some(stripped) = after.strip_prefix("$${") {
            out.push_str("${");
            rest = stripped;
        } else if let Some(body) = after.strip_prefix("${") {
            let end = body.find('}').ok_or_else(|| ConfigError::Env {
                path: path.to_owned(),
                message: "unterminated `${` (missing `}`)".to_owned(),
            })?;
            let expr = &body[..end];
            let (name, default) = match expr.split_once(":-") {
                Some((n, d)) => (n, Some(d)),
                None => (expr, None),
            };
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(ConfigError::Env {
                    path: path.to_owned(),
                    message: format!("invalid variable name `{name}`"),
                });
            }
            match (env(name), default) {
                (Some(v), Some(d)) if v.is_empty() => out.push_str(d),
                (Some(v), _) => out.push_str(&v),
                (None, Some(d)) => out.push_str(d),
                (None, None) => {
                    return Err(ConfigError::Env {
                        path: path.to_owned(),
                        message: format!("environment variable `{name}` is not set"),
                    })
                }
            }
            rest = &body[end + 1..];
        } else {
            out.push('$');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Walk a TOML value tree and substitute inside every string.
pub fn substitute_value(
    value: &mut toml::Value,
    path: &str,
    env: EnvLookup<'_>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(s) => {
            *s = substitute(s, path, env)?;
        }
        toml::Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                substitute_value(item, &format!("{path}[{i}]"), env)?;
            }
        }
        toml::Value::Table(table) => {
            for (k, v) in table.iter_mut() {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                substitute_value(v, &child, env)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "TOKEN" => Some("abc123".into()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    #[test]
    fn plain_strings_are_untouched() {
        assert_eq!(substitute("hello", "x", &env).unwrap(), "hello");
        assert_eq!(substitute("cost: $5", "x", &env).unwrap(), "cost: $5");
    }

    #[test]
    fn replaces_variables() {
        assert_eq!(substitute("${TOKEN}", "x", &env).unwrap(), "abc123");
        assert_eq!(
            substitute("Bearer ${TOKEN}!", "x", &env).unwrap(),
            "Bearer abc123!"
        );
    }

    #[test]
    fn defaults_and_escapes() {
        assert_eq!(
            substitute("${NOPE:-fallback}", "x", &env).unwrap(),
            "fallback"
        );
        assert_eq!(
            substitute("${EMPTY:-fallback}", "x", &env).unwrap(),
            "fallback"
        );
        assert_eq!(
            substitute("${TOKEN:-fallback}", "x", &env).unwrap(),
            "abc123"
        );
        assert_eq!(substitute("$${TOKEN}", "x", &env).unwrap(), "${TOKEN}");
    }

    #[test]
    fn missing_variable_is_an_error() {
        let err = substitute("${NOPE}", "notifier[0].bot_token", &env).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("NOPE"), "{msg}");
        assert!(msg.contains("notifier[0].bot_token"), "{msg}");
    }

    #[test]
    fn malformed_references_are_errors() {
        assert!(substitute("${TOKEN", "x", &env).is_err());
        assert!(substitute("${}", "x", &env).is_err());
        assert!(substitute("${BAD-NAME}", "x", &env).is_err());
    }
}
